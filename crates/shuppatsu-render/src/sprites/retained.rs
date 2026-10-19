//! The composed 2D canvas kept between composes ([`Composer`], docs/performance.md): each
//! compose compares the layer with the one drawn last and redraws only the canvas rectangles
//! whose pixels can differ, with the same pixels as a full [`SpriteLayer::compose`].
//!
//! A pixel's colour is the result of the draws that cover it, applied in order to a cleared
//! pixel; every draw only reads the pixel it writes. Draws are identified by their place in the
//! order (layer, slot index or widget/fan ordinal), and a draw counts as changed when what it
//! draws differs: its slot, widget or fan, its anchor, a palette it uses, or the bank's cached
//! picture for it. Outside the boxes of changed draws (old and new), the same draws cover a
//! pixel in the same order, so it keeps its colour. Inside them the canvas is cleared and every
//! draw reaching the box is applied again, clipped to it.
//!
//! The rest is global: the bank ([`SpriteBank::generation`]), the canvas size, the shake
//! offset and the backdrop colour; a change there redraws everything.

use super::{
    Canvas, Fan, Hires, LAYERS, PALETTE_COUNT, PaperFill, SLOT_COUNT, Slot, SlotDraw, SpriteBank,
    SpriteLayer, Widget, WidgetKind, fan_bbox, widget_bbox,
};
use shuppatsu_formats::cg::Palette;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A composed canvas and what it was composed from.
pub struct Composer {
    id: u64,
    rgba: Vec<u8>,
    size: (usize, usize),
    prev: Option<Prev>,
    pending: Pending,
    /// Debug aid (env `SHUPPATSU_VERIFY_COMPOSE=1`): every partial compose is checked against
    /// a full one.
    verify: bool,
}

/// The canvas rectangles changed since the last [`Composer::take_pending`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Pending {
    #[default]
    None,
    /// Everything.
    Full,
    /// `[x0, y0, x1, y1]` boxes, not overlapping.
    Rects(Vec<[usize; 4]>),
}

impl Pending {
    fn add(&mut self, rects: &[[usize; 4]], size: (usize, usize)) {
        if rects.is_empty() {
            return;
        }
        match self {
            Pending::Full => {}
            Pending::None => *self = Pending::Rects(rects.to_vec()),
            Pending::Rects(r) => {
                r.extend_from_slice(rects);
                *r = merge(std::mem::take(r));
                if r.len() > MAX_RECTS || area(r) * 2 > size.0 * size.1 {
                    *self = Pending::Full;
                }
            }
        }
    }
}

/// More rectangles than this are uploaded whole.
const MAX_RECTS: usize = 48;

struct Prev {
    ops: Vec<Op>,
    palettes: Vec<Option<Palette>>,
    bank: u64,
    shake: (i8, i8),
    size: (usize, usize),
    scale: usize,
    backdrop: Option<[u8; 3]>,
}

/// A draw's place in the order: layer rank (3 → 0, then the fans), slots before widgets,
/// then the index in draw order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct OpId {
    rank: u8,
    kind: u8,
    index: u16,
}

struct Op {
    id: OpId,
    key: Key,
    bbox: Option<[usize; 4]>,
    /// The palettes the draw reads.
    palettes: [Option<u8>; 3],
}

enum Key {
    Slot {
        slot: Slot,
        shift: i32,
        framed: bool,
        paper: Option<Arc<PaperFill>>,
        hires: Option<Arc<Hires>>,
        /// The translated text drawn over it (the bank's, unchanged while its generation is).
        text: Option<usize>,
    },
    Widget {
        widget: Widget,
        shift: i32,
    },
    Fan {
        fan: Fan,
        shift: i32,
    },
}

fn same_arc<T>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

fn same_widget(a: &Widget, b: &Widget) -> bool {
    match (&a.kind, &b.kind) {
        // Pictures compare by identity first: comparing their pixels costs.
        (WidgetKind::Image(x), WidgetKind::Image(y)) => {
            a.layer == b.layer
                && a.rect == b.rect
                && a.anchor == b.anchor
                && (Arc::ptr_eq(x, y) || x == y)
        }
        _ => a == b,
    }
}

impl Key {
    fn same(&self, other: &Key) -> bool {
        match (self, other) {
            (
                Key::Slot {
                    slot: a,
                    shift: sa,
                    framed: fa,
                    paper: pa,
                    hires: ha,
                    text: ta,
                },
                Key::Slot {
                    slot: b,
                    shift: sb,
                    framed: fb,
                    paper: pb,
                    hires: hb,
                    text: tb,
                },
            ) => a == b && sa == sb && fa == fb && ta == tb && same_arc(pa, pb) && same_arc(ha, hb),
            (
                Key::Widget {
                    widget: a,
                    shift: sa,
                },
                Key::Widget {
                    widget: b,
                    shift: sb,
                },
            ) => sa == sb && same_widget(a, b),
            (Key::Fan { fan: a, shift: sa }, Key::Fan { fan: b, shift: sb }) => a == b && sa == sb,
            _ => false,
        }
    }
}

/// The palettes a widget reads.
fn widget_palettes(w: &Widget) -> [Option<u8>; 3] {
    match &w.kind {
        WidgetKind::Fill(p) | WidgetKind::DownArrow(p) => [Some(p.palette), None, None],
        WidgetKind::Sprite { palette, .. } => [Some(*palette), None, None],
        WidgetKind::Image(_) => [None; 3],
        WidgetKind::Legend { style, .. } | WidgetKind::Text { style, .. } => [
            Some(style.paint.palette),
            style.backdrop.map(|p| p.palette),
            style.outline.map(|p| p.palette),
        ],
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl Default for Composer {
    fn default() -> Self {
        Self::new()
    }
}

impl Composer {
    pub fn new() -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            rgba: Vec::new(),
            size: (0, 0),
            prev: None,
            pending: Pending::Full,
            verify: std::env::var("SHUPPATSU_VERIFY_COMPOSE").is_ok_and(|v| v == "1"),
        }
    }

    /// A transparent canvas of `size` that was never composed (the first compose draws all).
    pub fn blank(size: (usize, usize)) -> Self {
        Self {
            rgba: vec![0; size.0 * size.1 * 4],
            size,
            ..Self::new()
        }
    }

    /// Identifies this canvas to the overlay it is uploaded to.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The composed pixels (RGBA8, [`Composer::size`]).
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub fn size(&self) -> (usize, usize) {
        self.size
    }

    /// What changed since the last call (for uploading it).
    pub fn take_pending(&mut self) -> Pending {
        std::mem::take(&mut self.pending)
    }

    /// Forgets what was drawn: the next compose draws everything.
    pub fn invalidate(&mut self) {
        self.prev = None;
    }

    /// Composes `layer` as [`SpriteLayer::compose`] does, then puts `backdrop` under it in the
    /// design rectangle ([`SpriteLayer::put_backdrop`]); only the changed rectangles are drawn
    /// again.
    pub fn compose(&mut self, layer: &SpriteLayer, bank: &SpriteBank, backdrop: Option<[u8; 3]>) {
        let _perf = crate::perf::scope(crate::perf::Section::Compose);
        crate::perf::count(crate::perf::Counter::Composes, 1);
        let size = layer.canvas_size();
        let c = layer.full_canvas();
        let slots = layer.prepare(bank, c);
        let ops = ops(layer, &slots, c);
        let dirty = self.prev.as_ref().and_then(|prev| {
            let same = prev.bank == bank.generation()
                && prev.shake == layer.shake
                && prev.size == size
                && prev.scale == layer.scale
                && prev.backdrop == backdrop;
            same.then(|| diff(prev, &ops, &layer.palettes))
        });
        let dirty = dirty
            .map(merge)
            .filter(|r| r.len() <= MAX_RECTS && area(r) * 2 <= size.0 * size.1);
        match dirty {
            Some(rects) => {
                for &r in &rects {
                    clear(&mut self.rgba, size.0, r);
                    let rc = Canvas { clip: r, ..c };
                    layer.draw(bank, &slots, rc, &mut self.rgba);
                    if let Some(rgb) = backdrop {
                        layer.put_backdrop(&mut self.rgba, rgb, r);
                    }
                }
                self.pending.add(&rects, size);
                if self.verify && !rects.is_empty() {
                    let mut full = vec![0; size.0 * size.1 * 4];
                    layer.draw(bank, &slots, c, &mut full);
                    if let Some(rgb) = backdrop {
                        layer.put_backdrop(&mut full, rgb, c.clip);
                    }
                    check(&full, &self.rgba, size, &rects);
                }
            }
            None => {
                crate::perf::count(crate::perf::Counter::FullComposes, 1);
                if self.size != size || self.rgba.len() != size.0 * size.1 * 4 {
                    self.rgba = vec![0; size.0 * size.1 * 4];
                    self.size = size;
                } else {
                    self.rgba.fill(0);
                }
                layer.draw(bank, &slots, c, &mut self.rgba);
                if let Some(rgb) = backdrop {
                    layer.put_backdrop(&mut self.rgba, rgb, c.clip);
                }
                self.pending = Pending::Full;
            }
        }
        // The palettes are kept for the next diff; only changed ones are copied.
        let mut palettes = match self.prev.take() {
            Some(p) => p.palettes,
            None => vec![None; PALETTE_COUNT],
        };
        palettes.resize(layer.palettes.len(), None);
        for (old, new) in palettes.iter_mut().zip(&layer.palettes) {
            if old != new {
                old.clone_from(new);
            }
        }
        self.prev = Some(Prev {
            ops,
            palettes,
            bank: bank.generation(),
            shake: layer.shake,
            size,
            scale: layer.scale,
            backdrop,
        });
    }
}

/// The draws of `layer` in draw order, with what decides their pixels.
fn ops(layer: &SpriteLayer, slots: &[Option<SlotDraw<'_>>], c: Canvas) -> Vec<Op> {
    let (sx, sy) = (i32::from(layer.shake.0), i32::from(layer.shake.1));
    let mut out = Vec::new();
    for l in (0..LAYERS).rev() {
        let rank = LAYERS - 1 - l;
        for i in (0..SLOT_COUNT).rev() {
            if layer.slots[i].layer != l {
                continue;
            }
            let Some(d) = &slots[i] else { continue };
            out.push(Op {
                id: OpId {
                    rank,
                    kind: 0,
                    index: (SLOT_COUNT - 1 - i) as u16,
                },
                key: Key::Slot {
                    slot: layer.slots[i],
                    shift: d.shift,
                    framed: layer.wide > 0 && layer.framed.contains(&i),
                    paper: d.paper.clone(),
                    hires: d.hires.clone(),
                    text: d.text.map(|t| t.as_ptr() as usize),
                },
                bbox: d.bbox(c),
                palettes: [Some(layer.slots[i].palette), None, None],
            });
        }
        for (n, w) in layer.widgets.iter().filter(|w| w.layer == l).enumerate() {
            let shift = layer.anchor_shift(w.anchor);
            out.push(Op {
                id: OpId {
                    rank,
                    kind: 1,
                    index: n.min(usize::from(u16::MAX)) as u16,
                },
                key: Key::Widget {
                    widget: w.clone(),
                    shift,
                },
                bbox: widget_bbox(w, sx, sy, shift, c),
                palettes: widget_palettes(w),
            });
        }
    }
    for (n, fan) in layer.fans.iter().enumerate() {
        let shift = layer.anchor_shift(fan.anchor);
        out.push(Op {
            id: OpId {
                rank: LAYERS,
                kind: 2,
                index: n as u16,
            },
            key: Key::Fan { fan: *fan, shift },
            bbox: fan_bbox(fan, sx, sy, shift, c),
            palettes: [None; 3],
        });
    }
    out
}

/// The boxes of the draws that changed since `prev`.
fn diff(prev: &Prev, ops: &[Op], palettes: &[Option<Palette>]) -> Vec<[usize; 4]> {
    let changed: Vec<bool> = (0..palettes.len().max(prev.palettes.len()))
        .map(|i| palettes.get(i) != prev.palettes.get(i))
        .collect();
    let reads_changed = |op: &Op| {
        op.palettes
            .iter()
            .flatten()
            .any(|&p| changed.get(usize::from(p)) == Some(&true))
    };
    let mut out = Vec::new();
    let mut add = |op: &Op| out.extend(op.bbox);
    let (old, new) = (&prev.ops, ops);
    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        match (old.get(i), new.get(j)) {
            (Some(a), Some(b)) if a.id == b.id => {
                if !a.key.same(&b.key) || reads_changed(a) || reads_changed(b) || a.bbox != b.bbox {
                    add(a);
                    add(b);
                }
                i += 1;
                j += 1;
            }
            (Some(a), Some(b)) if a.id < b.id => {
                add(a);
                i += 1;
            }
            (Some(a), None) => {
                add(a);
                i += 1;
            }
            (_, Some(b)) => {
                add(b);
                j += 1;
            }
            (None, None) => break,
        }
    }
    out
}

fn area(rects: &[[usize; 4]]) -> usize {
    rects.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum()
}

/// Merges overlapping boxes, and boxes whose union is barely larger than the two, until no two
/// overlap.
fn merge(mut rects: Vec<[usize; 4]>) -> Vec<[usize; 4]> {
    rects.retain(|r| r[0] < r[2] && r[1] < r[3]);
    let size = |r: &[usize; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    loop {
        let mut merged = false;
        'outer: for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let (a, b) = (rects[i], rects[j]);
                let u = [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ];
                if super::overlaps(a, b) || size(&u) * 4 <= (size(&a) + size(&b)) * 5 {
                    rects[i] = u;
                    rects.swap_remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            return rects;
        }
    }
}

fn clear(rgba: &mut [u8], w: usize, [x0, y0, x1, y1]: [usize; 4]) {
    for y in y0..y1 {
        rgba[(y * w + x0) * 4..(y * w + x1) * 4].fill(0);
    }
}

/// Debug aid: a partial compose must equal the full one.
fn check(full: &[u8], got: &[u8], (w, _): (usize, usize), rects: &[[usize; 4]]) {
    if let Some(i) = full.iter().zip(got).position(|(a, b)| a != b) {
        let p = i / 4;
        panic!(
            "partial compose differs at pixel ({}, {}): full {:?}, partial {:?}; redrawn {rects:?}",
            p % w,
            p / w,
            &full[p * 4..p * 4 + 4],
            &got[p * 4..p * 4 + 4],
        );
    }
}
