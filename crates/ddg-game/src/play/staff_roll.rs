//! Enhancement `one_line_captions` for the ending's staff roll (docs/localisation.md, "Staff
//! roll"): with a translation drawn, a credit is code-built text instead of its sprite with the
//! English squeezed into the kanji boxes, the way the station plate above it is the in-game
//! caption's line ([`crate::captions::PlateLine`]).
//!
//! The English comes from the credit sprites' own `[tbl]` entries in the catalogue (one
//! source; the entries stay the sprite fallback with the setting off): parts in the shared
//! group `ending.name` are names, `ending.role` roles and headers, and parts without a shared
//! group are companies and the title's 高速編, or names when " · " separates several.
//!
//! Every screen (the credits shown together) is laid out the same way: the credited party in a
//! left column at the original's name position, one per line, and what they did in a right
//! column, one role line per line, at two sizes for the whole roll measured from the kanji (the
//! Latin capitals 0.75 of the lettering, as the captions). Companies are the credited party
//! when no person is named, in the name style; next to names they are the affiliation, in the
//! role style. Logos (the JR mark, Stack Lab.) and the Latin art (GD, ID, GM2 HEAD) are drawn
//! from the original art in the right column; the title card keeps its layout.

use ddg_render::sprites::{Anchor, Paint, SpriteBank, SpriteLayer, TextStyle, WidgetKind};
use ddg_render::text::{Align, FontRole, TextPart};
use std::collections::HashMap;

/// Shared groups of the credit parts in the catalogue.
const GROUP_NAME: &str = "ending.name";
const GROUP_ROLE: &str = "ending.role";
/// Between names that share a region.
const NAME_SEPARATOR: &str = " · ";
/// Latin capital height as a share of the original lettering height (as the captions).
const LATIN_CAP_OF_LETTERING: f32 = 0.75;
/// Baseline-to-baseline distances in capitals: stacked names (clear of macrons above and
/// descenders below: 1.26 + 0.32 in M+ 2), and role lines (the original's
/// two-line roles are 20 px apart with 17 px lettering, sprites 0x3a9–0x3b1).
const NAME_PITCH: f32 = 1.7;
const ROLE_PITCH: f32 = 1.6;
/// Between the name and the role column, in name capitals; between a logo and its text, in
/// role capitals.
const COLUMN_GAP: f32 = 1.0;
const ART_GAP: f32 = 0.6;
/// The text stays this far inside the 640-wide design and the black bands.
const SIDE_MARGIN: f32 = 16.0;
const BAND_MARGIN: f32 = 4.0;
/// Names shrink in these steps until every screen fits, down to this many role capitals.
const FIT_STEP: f32 = 0.5;
const MIN_NAME_OF_ROLE: f32 = 1.5;
/// Extra room round a text widget's ink, original pixels (the widget centres the ink).
const INK_MARGIN: f32 = 2.0;
/// A text widget's height in capitals: a line box with room to spare (M+ 2 is 1.98).
const LINE_BOX: f32 = 3.0;
const WIDGET_LAYER: u8 = 3;

/// Art kept from the original credits (`[x, y, w, h]` in art pixels; ink bounds of the
/// lettering colours from the art, see the `kept_art_covers_the_untranslated_ink` test) and
/// where it goes.
const KEPT: [(u16, [u16; 4], Place); 7] = [
    // 電車でGO!2 and STAFF of the title card.
    (0x3a8, [4, 6, 274, 85], Place::Stay),
    (0x3a8, [290, 57, 169, 27], Place::Stay),
    // The JR mark before 株式会社ジェイアール東日本企画 (part 1).
    (0x3bd, [8, 25, 60, 33], Place::Before(1)),
    // The Stack Lab. logo under 協力 (part 0).
    (0x3c2, [94, 3, 235, 42], Place::After(0)),
    // GD and ID after デザイン (part 1), GM2 HEAD on its own.
    (0x3b5, [272, 18, 32, 16], Place::After(1)),
    (0x3b6, [278, 19, 26, 15], Place::After(1)),
    (0x3bc, [211, 7, 130, 16], Place::Right),
];

/// Original ink that is dropped: the ・ between the two names of 0x3c6 (stacked here).
#[cfg(test)]
const DROPPED: [(u16, [u16; 4]); 1] = [(0x3c6, [161, 19, 6, 5])];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Where it is (the title card keeps its layout).
    Stay,
    /// On the row of part `i`, before its text.
    Before(usize),
    /// On its own row after part `i`, in the same column.
    After(usize),
    /// On its own row at the end of the right column.
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Name,
    Role,
    /// A company or the title's 高速編: the credited party unless a person is named.
    Feature,
}

#[derive(Debug, Clone, PartialEq)]
struct Part {
    kind: Kind,
    lines: Vec<String>,
    region: [u16; 4],
    /// The lettering colour of the region (palette 0x17) and the height of its rows.
    colour: u8,
    ink_h: usize,
}

/// One credit sprite.
#[derive(Debug, Clone, PartialEq)]
struct Card {
    size: (u16, u16),
    parts: Vec<Part>,
    kept: Vec<([u16; 4], Place)>,
}

/// Text measurement: `(width, ink top, ink bottom)` of a line with capitals `cap` high,
/// relative to its pen and baseline.
pub trait Measure {
    fn extent(&self, text: &str, cap: f32) -> (f32, f32, f32);
}

/// The label font of a bank in one role.
pub struct BankMeasure<'a>(pub &'a SpriteBank, pub FontRole);

impl Measure for BankMeasure<'_> {
    fn extent(&self, text: &str, cap: f32) -> (f32, f32, f32) {
        self.0
            .label_extent(text, cap, self.1)
            .unwrap_or((0.0, 0.0, 0.0))
    }
}

/// One shown credit: tilemap and top left (design pixels).
pub type Shown = (u16, i16, i16);

/// A screen of the roll ready to draw: text and art widgets, centred in a wide canvas like
/// the credit slots.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreditScreen {
    items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
enum Item {
    Text {
        rect: [f32; 4],
        text: String,
        style: TextStyle,
    },
    Art {
        rect: [f32; 4],
        tbl: u16,
        src: [u16; 4],
    },
}

impl CreditScreen {
    pub fn draw(&self, layer: &mut SpriteLayer) {
        layer.anchored(Anchor::Centre, |layer| {
            for it in &self.items {
                match it {
                    Item::Text { rect, text, style } => {
                        layer.add_styled_text(WIDGET_LAYER, *rect, text, *style);
                    }
                    Item::Art { rect, tbl, src } => layer.add_widget(
                        WIDGET_LAYER,
                        *rect,
                        WidgetKind::Sprite {
                            tilemap: *tbl,
                            src: *src,
                            scale: 1.0,
                            palette: super::ending::PAL_TEXT,
                        },
                    ),
                }
            }
        });
    }

    /// The ink boxes `[x0, y0, x1, y1]` of the screen's texts and art, by text (the art by
    /// tilemap), for overlap and bounds checks.
    #[cfg(test)]
    pub(super) fn ink_boxes(&self, m: &impl Measure) -> Vec<(String, [f32; 4])> {
        self.items
            .iter()
            .map(|i| match i {
                Item::Text { rect, text, style } => {
                    let (w, t, b) = m.extent(text, style.cap.unwrap_or(0.0));
                    let top = rect[1] + (rect[3] - (b - t)) / 2.0;
                    (text.clone(), [rect[0], top, rect[0] + w, top + b - t])
                }
                Item::Art { rect, tbl, .. } => (
                    format!("art {tbl:#x}"),
                    [rect[0], rect[1], rect[0] + rect[2], rect[1] + rect[3]],
                ),
            })
            .collect()
    }

    #[cfg(test)]
    fn texts(&self) -> Vec<(&str, [f32; 4])> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Text { rect, text, .. } => Some((text.as_str(), *rect)),
                Item::Art { .. } => None,
            })
            .collect()
    }
}

/// The black bands round the 3D view: the view runs from `view.0` to `view.1` (design y).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bands {
    pub view: (f32, f32),
}

/// The credits of a translation, measured from the art, with the sizes of the roll.
#[derive(Debug, Clone, PartialEq)]
pub struct StaffRoll {
    cards: HashMap<u16, Card>,
    font: FontRole,
    /// Name and role capital heights, original pixels.
    pub name_cap: f32,
    pub role_cap: f32,
}

impl StaffRoll {
    /// The cards of the credit sprites `tbls` that have a catalogue entry with art; the
    /// preferred sizes from the kanji. `None` when no credit has an entry.
    pub fn new(
        bank: &SpriteBank,
        parts: &HashMap<u16, Vec<TextPart>>,
        tbls: impl IntoIterator<Item = u16>,
        font: FontRole,
    ) -> Option<Self> {
        let mut cards = HashMap::new();
        for tbl in tbls {
            let (Some(p), Some((w, h, px))) = (parts.get(&tbl), bank.art(tbl)) else {
                continue;
            };
            let kept = KEPT
                .iter()
                .filter(|k| k.0 == tbl)
                .map(|k| (k.1, k.2))
                .collect();
            if let Some(c) = Card::of(p, w, h, &px, kept) {
                cards.insert(tbl, c);
            }
        }
        if cards.is_empty() {
            return None;
        }
        let (names, roles) = measure_lettering(&cards);
        Some(Self {
            cards,
            font,
            name_cap: LATIN_CAP_OF_LETTERING * names?,
            role_cap: LATIN_CAP_OF_LETTERING * roles?,
        })
    }

    /// Shrinks the names in steps until every screen of `screens` fits (each with its bands);
    /// false (sizes unchanged) when even the smallest does not.
    pub fn fit(&mut self, m: &impl Measure, screens: &[(Vec<Shown>, Bands)]) -> bool {
        let start = self.name_cap;
        let mut n = start;
        while n >= MIN_NAME_OF_ROLE * self.role_cap {
            self.name_cap = n;
            if screens
                .iter()
                .filter(|(s, _)| self.covers(s))
                .all(|(s, b)| self.layout(m, s, *b).is_some())
            {
                return true;
            }
            n -= FIT_STEP;
        }
        self.name_cap = start;
        false
    }

    /// Whether every credit of `shown` (at least one) has a card, so the screen is text.
    fn covers(&self, shown: &[Shown]) -> bool {
        !shown.is_empty() && shown.iter().all(|s| self.cards.contains_key(&s.0))
    }

    /// The screen of the credits `shown`; `None` keeps the sprites (a credit without an entry,
    /// like the closing logos, or a screen that does not fit).
    pub fn layout(&self, m: &impl Measure, shown: &[Shown], bands: Bands) -> Option<CreditScreen> {
        let cards: Vec<(&Card, i16, i16)> = shown
            .iter()
            .map(|&(t, x, y)| Some((self.cards.get(&t)?, x, y)))
            .collect::<Option<_>>()?;
        if cards.is_empty() {
            return None;
        }
        let mut items = Vec::new();
        if let [(card, x, y)] = cards[..]
            && card.kept.iter().any(|k| k.1 == Place::Stay)
        {
            self.in_place(m, card, (x, y), &mut items, shown[0].0);
            return Some(CreditScreen { items });
        }
        // Screens split by the 3D view: the plain ending shows a credit above it and three
        // names below it together.
        let middle = (bands.view.0 + bands.view.1) / 2.0;
        let (above, below): (Vec<_>, Vec<_>) = cards
            .iter()
            .zip(shown)
            .partition(|((c, _, y), _)| f32::from(*y) + f32::from(c.size.1) / 2.0 < middle);
        for (group, area) in [
            (above, (BAND_MARGIN, bands.view.0 - BAND_MARGIN)),
            (below, (bands.view.1 + BAND_MARGIN, 480.0 - BAND_MARGIN)),
        ] {
            if !group.is_empty() {
                let g: Vec<(&Card, i16, i16, u16)> = group
                    .iter()
                    .map(|((c, x, y), s)| (*c, *x, *y, s.0))
                    .collect();
                self.group(m, &g, area, &mut items)?;
            }
        }
        Some(CreditScreen { items })
    }

    /// The title card: its art where it is, 高速編 as text at the name size on its row.
    fn in_place(
        &self,
        m: &impl Measure,
        card: &Card,
        at: (i16, i16),
        items: &mut Vec<Item>,
        tbl: u16,
    ) {
        let (x, y) = (f32::from(at.0), f32::from(at.1));
        for &(src, _) in &card.kept {
            let [sx, sy, w, h] = src.map(f32::from);
            items.push(Item::Art {
                rect: [x + sx, y + sy, w, h],
                tbl,
                src,
            });
        }
        for p in &card.parts {
            let [rx, ry, _, rh] = p.region.map(f32::from);
            let text = p.lines.join(" ");
            let (_, t, b) = m.extent(&text, self.name_cap);
            let baseline = y + ry + rh / 2.0 - (t + b) / 2.0;
            items.push(self.text(m, &text, x + rx, baseline, self.name_cap, p.colour));
        }
    }

    /// One group of credits on one side of the 3D view, inside the band `area` (design y).
    fn group(
        &self,
        m: &impl Measure,
        g: &[(&Card, i16, i16, u16)],
        area: (f32, f32),
        items: &mut Vec<Item>,
    ) -> Option<()> {
        let has_names = g
            .iter()
            .any(|(c, ..)| c.parts.iter().any(|p| p.kind == Kind::Name));
        let mut left: Vec<Row> = Vec::new();
        let mut right: Vec<Row> = Vec::new();
        let mut anchor = f32::MAX;
        for &(card, x, _, tbl) in g {
            let before = |i: usize| {
                card.kept
                    .iter()
                    .find(|k| k.1 == Place::Before(i))
                    .map(|k| (tbl, k.0))
            };
            for (i, p) in card.parts.iter().enumerate() {
                // Without a person, a company is the credited party and its header goes above
                // it, as in the original.
                let to_left = p.kind == Kind::Name || !has_names;
                let style = match p.kind {
                    Kind::Name => Style::Name,
                    Kind::Role => Style::Role,
                    Kind::Feature if has_names => Style::Role,
                    Kind::Feature => Style::Name,
                };
                if to_left {
                    anchor = anchor.min(f32::from(x) + f32::from(p.region[0]));
                }
                let col = if to_left { &mut left } else { &mut right };
                match before(i) {
                    Some(art) => col.push(Row::Text {
                        lines: p.lines.clone(),
                        colour: p.colour,
                        style,
                        art: Some(art),
                    }),
                    None => col.extend(p.lines.iter().map(|l| Row::Text {
                        lines: vec![l.clone()],
                        colour: p.colour,
                        style,
                        art: None,
                    })),
                }
                for k in card.kept.iter().filter(|k| k.1 == Place::After(i)) {
                    col.push(Row::Art(tbl, k.0));
                }
            }
            for k in card.kept.iter().filter(|k| k.1 == Place::Right) {
                right.push(Row::Art(tbl, k.0));
            }
        }
        // A card always has a part, and without names every part goes left.
        if left.is_empty() {
            return None;
        }
        // The role column clears the widest name of the whole roll, so it lines up from card
        // to card; a wider left column (a company) pushes it further on its own screen. A
        // screen too wide for its place moves left (the plain ending's right-hand credits).
        let delta = self.widest_name(m) + COLUMN_GAP * self.name_cap;
        let (top, bottom) = g
            .iter()
            .map(|(c, _, y, _)| (f32::from(*y), f32::from(*y) + f32::from(c.size.1)))
            .fold((f32::MAX, f32::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)));
        let centre = (top + bottom) / 2.0;
        loop {
            let lw = self.column_width(m, &left);
            let off = delta.max(lw + COLUMN_GAP * self.name_cap);
            let width = if right.is_empty() {
                lw
            } else {
                off + self.column_width(m, &right)
            };
            let lx = anchor.min(640.0 - SIDE_MARGIN - width);
            if lx >= SIDE_MARGIN - 0.01 {
                self.stack(m, &left, lx, centre, area, items)?;
                self.stack(m, &right, lx + off, centre, area, items)?;
                return Some(());
            }
            // Too wide even against the left margin: wrap the widest line that can be.
            if !self.wrap_widest(m, &mut right) && !self.wrap_widest(m, &mut left) {
                return None;
            }
        }
    }

    fn widest_name(&self, m: &impl Measure) -> f32 {
        self.cards
            .values()
            .flat_map(|c| c.parts.iter().filter(|p| p.kind == Kind::Name))
            .flat_map(|p| p.lines.iter())
            .map(|l| m.extent(l, self.name_cap).0)
            .fold(0.0, f32::max)
    }

    fn cap(&self, s: Style) -> f32 {
        match s {
            Style::Name => self.name_cap,
            Style::Role => self.role_cap,
        }
    }

    fn pitch(&self, s: Style) -> f32 {
        match s {
            Style::Name => NAME_PITCH * self.name_cap,
            Style::Role => ROLE_PITCH * self.role_cap,
        }
    }

    fn row_width(&self, m: &impl Measure, r: &Row) -> f32 {
        match r {
            Row::Text {
                lines, style, art, ..
            } => {
                let w = lines
                    .iter()
                    .map(|l| m.extent(l, self.cap(*style)).0)
                    .fold(0.0, f32::max);
                art.map_or(w, |(_, s)| f32::from(s[2]) + ART_GAP * self.role_cap + w)
            }
            Row::Art(_, s) => f32::from(s[2]),
        }
    }

    fn column_width(&self, m: &impl Measure, col: &[Row]) -> f32 {
        col.iter().map(|r| self.row_width(m, r)).fold(0.0, f32::max)
    }

    /// Height of a row: text rows are a pitch per line; art gets the role lines' spacing.
    fn row_height(&self, r: &Row) -> f32 {
        let pad = (ROLE_PITCH - 1.0) * self.role_cap;
        match r {
            Row::Text {
                lines, style, art, ..
            } => {
                let text = lines.len() as f32 * self.pitch(*style);
                art.map_or(text, |(_, s)| text.max(f32::from(s[3]) + pad))
            }
            Row::Art(_, s) => f32::from(s[3]) + pad,
        }
    }

    /// Splits the widest line of `col` that has a space into two balanced lines.
    fn wrap_widest(&self, m: &impl Measure, col: &mut [Row]) -> bool {
        let mut best: Option<(usize, usize, f32)> = None;
        for (ri, r) in col.iter().enumerate() {
            if let Row::Text { lines, style, .. } = r {
                for (li, l) in lines.iter().enumerate() {
                    let w = m.extent(l, self.cap(*style)).0;
                    if l.contains(' ') && best.is_none_or(|b| w > b.2) {
                        best = Some((ri, li, w));
                    }
                }
            }
        }
        let Some((ri, li, _)) = best else {
            return false;
        };
        let Row::Text { lines, style, .. } = &mut col[ri] else {
            return false;
        };
        let cap = self.cap(*style);
        let line = lines[li].clone();
        let split = line
            .match_indices(' ')
            .map(|(i, _)| i)
            .min_by(|&a, &b| {
                let w = |i: usize| {
                    m.extent(&line[..i], cap)
                        .0
                        .max(m.extent(&line[i + 1..], cap).0)
                };
                w(a).total_cmp(&w(b))
            })
            .unwrap_or(0);
        lines.splice(
            li..=li,
            [line[..split].to_owned(), line[split + 1..].to_owned()],
        );
        true
    }

    /// Stacks the rows of a column from x = `x`, centred on `centre` and kept inside `area`.
    fn stack(
        &self,
        m: &impl Measure,
        col: &[Row],
        x: f32,
        centre: f32,
        area: (f32, f32),
        items: &mut Vec<Item>,
    ) -> Option<()> {
        if col.is_empty() {
            return Some(());
        }
        let h: f32 = col.iter().map(|r| self.row_height(r)).sum();
        if h > area.1 - area.0 + 0.01 {
            return None;
        }
        let mut top = (centre - h / 2.0).clamp(area.0, area.1 - h);
        for r in col {
            let rh = self.row_height(r);
            match r {
                Row::Text {
                    lines,
                    colour,
                    style,
                    art,
                } => {
                    let cap = self.cap(*style);
                    let pitch = self.pitch(*style);
                    let mut tx = x;
                    let first = top + (rh - lines.len() as f32 * pitch) / 2.0;
                    if let Some((tbl, src)) = art {
                        let [_, _, w, ah] = src.map(f32::from);
                        items.push(Item::Art {
                            rect: [x, top + (rh - ah) / 2.0, w, ah],
                            tbl: *tbl,
                            src: *src,
                        });
                        tx += w + ART_GAP * self.role_cap;
                    }
                    for (i, l) in lines.iter().enumerate() {
                        let baseline = first + i as f32 * pitch + (pitch + cap) / 2.0;
                        items.push(self.text(m, l, tx, baseline, cap, *colour));
                    }
                }
                Row::Art(tbl, src) => {
                    let [_, _, w, ah] = src.map(f32::from);
                    items.push(Item::Art {
                        rect: [x, top + (rh - ah) / 2.0, w, ah],
                        tbl: *tbl,
                        src: *src,
                    });
                }
            }
            top += rh;
        }
        Some(())
    }

    /// A left-aligned text widget with its pen at `x` and its baseline at `baseline`.
    fn text(
        &self,
        m: &impl Measure,
        text: &str,
        x: f32,
        baseline: f32,
        cap: f32,
        colour: u8,
    ) -> Item {
        let (w, t, b) = m.extent(text, cap);
        // Tall enough for the font's line height, which limits Latin text in a widget.
        let h = (b - t + 2.0 * INK_MARGIN).max(LINE_BOX * cap);
        Item::Text {
            rect: [
                x,
                baseline + t - (h - (b - t)) / 2.0,
                w + cap + 2.0 * INK_MARGIN,
                h,
            ],
            text: text.to_owned(),
            style: TextStyle {
                paint: Paint {
                    palette: super::ending::PAL_TEXT,
                    index: colour,
                },
                cap: Some(cap),
                outline: None,
                backdrop: None,
                align: Align::Left,
                vertical: false,
                font: self.font,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Name,
    Role,
}

/// A row of a column: text lines (with a logo before them), or art on its own.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    Text {
        lines: Vec<String>,
        colour: u8,
        style: Style,
        art: Option<(u16, [u16; 4])>,
    },
    Art(u16, [u16; 4]),
}

impl Card {
    fn of(
        parts: &[TextPart],
        w: usize,
        h: usize,
        px: &[u8],
        kept: Vec<([u16; 4], Place)>,
    ) -> Option<Self> {
        let backdrop = box_colour(w, h, px)?;
        let parts = parts
            .iter()
            .map(|p| {
                let kind = match p.shared_group.as_deref() {
                    Some(GROUP_NAME) => Kind::Name,
                    Some(GROUP_ROLE) => Kind::Role,
                    // Several names in one region, " · " between them (0x3c2: the art has
                    // them in one row, too narrow for the shared name size).
                    _ if p.text.contains(NAME_SEPARATOR) => Kind::Name,
                    _ => Kind::Feature,
                };
                let region = p.region.unwrap_or([0, 0, w as u16, h as u16]);
                // One credited party or role per line: names joined with " · " in one
                // region (0x3c2) are stacked too.
                let lines = p
                    .text
                    .lines()
                    .flat_map(|l| {
                        if kind == Kind::Role {
                            vec![l.to_owned()]
                        } else {
                            l.split(NAME_SEPARATOR).map(str::to_owned).collect()
                        }
                    })
                    .filter(|l| !l.trim().is_empty())
                    .collect();
                let (colour, ink_h) = lettering(w, h, px, region, backdrop)?;
                Some(Part {
                    kind,
                    lines,
                    region,
                    colour,
                    ink_h,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            size: (w as u16, h as u16),
            parts,
            kept,
        })
    }
}

/// The box colour: the commonest opaque colour on the sprite's edge.
fn box_colour(w: usize, h: usize, px: &[u8]) -> Option<u8> {
    let mut edge = [0u32; 256];
    for y in 0..h {
        for x in 0..w {
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                edge[usize::from(px[y * w + x])] += 1;
            }
        }
    }
    (1..=255u8)
        .filter(|&i| edge[usize::from(i)] > 0)
        .max_by_key(|&i| edge[usize::from(i)])
}

/// The lettering of a region: its commonest colour other than the box, and the height of
/// the rows it is on.
fn lettering(w: usize, h: usize, px: &[u8], region: [u16; 4], backdrop: u8) -> Option<(u8, usize)> {
    let [rx, ry, rw, rh] = region.map(usize::from);
    let (x1, y1) = ((rx + rw).min(w), (ry + rh).min(h));
    let mut counts = [0u32; 256];
    for y in ry..y1 {
        for x in rx..x1 {
            counts[usize::from(px[y * w + x])] += 1;
        }
    }
    let letter = (1..=255u8)
        .filter(|&i| i != backdrop && counts[usize::from(i)] > 0)
        .max_by_key(|&i| counts[usize::from(i)])?;
    let rows: Vec<usize> = (ry..y1)
        .filter(|&y| (rx..x1).any(|x| px[y * w + x] == letter))
        .collect();
    Some((letter, rows.last()? + 1 - rows.first()?))
}

/// The lettering heights of the roll, original pixels: the median kanji name (a name part's
/// rows) and the median one-line role.
fn measure_lettering(cards: &HashMap<u16, Card>) -> (Option<f32>, Option<f32>) {
    let median = |kind: Kind| {
        let mut v: Vec<usize> = cards
            .values()
            .flat_map(|c| c.parts.iter())
            .filter(|p| p.kind == kind && p.lines.len() == 1)
            .map(|p| p.ink_h)
            .collect();
        v.sort_unstable();
        v.get(v.len() / 2).map(|&h| h as f32)
    };
    (median(Kind::Name), median(Kind::Role))
}

/// The lettering of credit `tbl` that no catalogue part covers, outside the kept and dropped
/// art: `(x, y)` art pixels (empty when the card accounts for all of its ink), and the kept
/// boxes without ink.
#[cfg(test)]
pub(super) fn unaccounted_ink(
    bank: &SpriteBank,
    parts: &[TextPart],
    tbl: u16,
) -> (Vec<(usize, usize)>, Vec<[u16; 4]>) {
    let (w, h, px) = bank.art(tbl).expect("art");
    let backdrop = box_colour(w, h, &px).expect("box");
    let inside = |b: [u16; 4], x: usize, y: usize| {
        let [bx, by, bw, bh] = b.map(usize::from);
        (bx..bx + bw).contains(&x) && (by..by + bh).contains(&y)
    };
    let kept: Vec<[u16; 4]> = KEPT.iter().filter(|k| k.0 == tbl).map(|k| k.1).collect();
    let dropped: Vec<[u16; 4]> = DROPPED.iter().filter(|k| k.0 == tbl).map(|k| k.1).collect();
    let mut stray = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let v = px[y * w + x];
            if v == 0 || v == backdrop {
                continue;
            }
            let covered = parts
                .iter()
                .any(|p| p.region.is_some_and(|r| inside(r, x, y)))
                || kept.iter().chain(&dropped).any(|&b| inside(b, x, y));
            if !covered {
                stray.push((x, y));
            }
        }
    }
    let empty = kept
        .into_iter()
        .filter(|&b| {
            !(0..h)
                .any(|y| (0..w).any(|x| inside(b, x, y) && ![0, backdrop].contains(&px[y * w + x])))
        })
        .collect();
    (stray, empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Half a capital per character; capitals from the baseline up, macrons a quarter above,
    /// descenders 0.3 below.
    struct Fake;

    impl Measure for Fake {
        fn extent(&self, text: &str, cap: f32) -> (f32, f32, f32) {
            let top = if text.contains(['Ō', 'ō', 'ū']) {
                -1.25
            } else {
                -1.0
            };
            let bottom = if text.contains(['g', 'j', 'p', 'q', 'y', ',']) {
                0.3
            } else {
                0.0
            };
            (
                0.5 * cap * text.chars().count() as f32,
                top * cap,
                bottom * cap,
            )
        }
    }

    fn part(kind: Kind, text: &str, region: [u16; 4]) -> Part {
        let lines = text
            .lines()
            .flat_map(|l| {
                if kind == Kind::Role {
                    vec![l.to_owned()]
                } else {
                    l.split(NAME_SEPARATOR).map(str::to_owned).collect()
                }
            })
            .collect();
        Part {
            kind,
            lines,
            region,
            colour: 7,
            ink_h: 30,
        }
    }

    fn card(size: (u16, u16), parts: Vec<Part>) -> Card {
        Card {
            size,
            parts,
            kept: Vec::new(),
        }
    }

    /// Two person cards, a two-name card, a three-name card under a header, and a company
    /// card; names 20 and roles 10 px capitals.
    fn roll() -> StaffRoll {
        use Kind::*;
        let cards = [
            (
                1,
                card(
                    (352, 48),
                    vec![
                        part(Name, "Akira Saitō", [5, 3, 146, 41]),
                        part(
                            Role,
                            "Project Leader\nPlanning, Character Design",
                            [158, 2, 193, 44],
                        ),
                    ],
                ),
            ),
            (
                2,
                card(
                    (352, 48),
                    vec![
                        part(Name, "Yukihiro Moriyama", [5, 3, 145, 41]),
                        part(Role, "Map Design", [158, 4, 193, 41]),
                    ],
                ),
            ),
            (
                3,
                card(
                    (400, 48),
                    vec![
                        part(Name, "Masayuki Kikuchi", [3, 3, 148, 41]),
                        part(Name, "Tetsuyoshi Yamaji", [178, 3, 146, 41]),
                        part(Role, "Voice", [337, 12, 62, 24]),
                    ],
                ),
            ),
            (
                4,
                card(
                    (368, 64),
                    vec![
                        part(Role, "Reference Materials", [2, 1, 364, 22]),
                        part(Name, "Akira Iwai", [1, 32, 107, 31]),
                        part(Name, "Yūichi Ōnogi", [112, 32, 141, 31]),
                        part(Name, "Tetsushi Abe", [255, 32, 112, 31]),
                    ],
                ),
            ),
            (
                5,
                card(
                    (384, 64),
                    vec![
                        part(Role, "Thanks to", [2, 1, 200, 20]),
                        part(Feature, "Hokuetsu Express Corporation", [50, 22, 333, 41]),
                    ],
                ),
            ),
        ];
        StaffRoll {
            cards: cards.into_iter().collect(),
            font: FontRole::Body,
            name_cap: 20.0,
            role_cap: 10.0,
        }
    }

    const BANDS: Bands = Bands {
        view: (112.8, 345.6),
    };

    fn rect_of(s: &CreditScreen, text: &str) -> [f32; 4] {
        s.texts()
            .into_iter()
            .find(|t| t.0 == text)
            .unwrap_or_else(|| panic!("no {text:?} in {:?}", s.texts()))
            .1
    }

    fn style_of(s: &CreditScreen, text: &str) -> TextStyle {
        s.items
            .iter()
            .find_map(|i| match i {
                Item::Text { text: t, style, .. } if t == text => Some(*style),
                _ => None,
            })
            .unwrap()
    }

    /// The baseline of a text item, from its rectangle (the widget centres the ink).
    fn baseline(s: &CreditScreen, text: &str, cap: f32) -> f32 {
        let [_, y, _, h] = rect_of(s, text);
        let (_, t, b) = Fake.extent(text, cap);
        y + (h - (b - t)) / 2.0 - t
    }

    /// Names at one size in the left column from the original's name position, roles at one
    /// smaller size in one right column for the whole roll (clear of the widest name), in
    /// white on the box, palette 0x17, no outline, the caption font.
    #[test]
    fn names_left_roles_right_at_two_sizes() {
        let r = roll();
        let a = r.layout(&Fake, &[(1, 24, 360)], BANDS).unwrap();
        let b = r.layout(&Fake, &[(2, 24, 360)], BANDS).unwrap();
        let widest = Fake.extent("Tetsuyoshi Yamaji", 20.0).0;
        let role_x = 29.0 + widest + COLUMN_GAP * 20.0;
        assert_eq!(rect_of(&a, "Akira Saitō")[0], 29.0);
        assert_eq!(rect_of(&b, "Yukihiro Moriyama")[0], 29.0);
        assert_eq!(rect_of(&a, "Project Leader")[0], role_x);
        assert_eq!(rect_of(&a, "Planning, Character Design")[0], role_x);
        assert_eq!(rect_of(&b, "Map Design")[0], role_x);
        let name = style_of(&a, "Akira Saitō");
        assert_eq!(name.cap, Some(20.0));
        assert_eq!(style_of(&b, "Yukihiro Moriyama"), name);
        let role = style_of(&a, "Project Leader");
        assert_eq!(role.cap, Some(10.0));
        assert_eq!(style_of(&b, "Map Design"), role);
        assert_eq!(
            name.paint,
            Paint {
                palette: 0x17,
                index: 7
            }
        );
        assert_eq!(
            (name.outline, name.backdrop, name.align),
            (None, None, Align::Left)
        );
        assert_eq!(name.font, FontRole::Body);
        // Multi-line roles stay multi-line, a role pitch apart, centred with the name on
        // the card.
        let (p, q) = (
            baseline(&a, "Project Leader", 10.0),
            baseline(&a, "Planning, Character Design", 10.0),
        );
        assert!((q - p - ROLE_PITCH * 10.0).abs() < 1e-3);
        let n = baseline(&a, "Akira Saitō", 20.0);
        assert!((n - 20.0 / 2.0 - 384.0).abs() < 1e-3, "{n}");
        assert!(((p + q) / 2.0 - 10.0 / 2.0 - 384.0).abs() < 1e-3);
    }

    /// Several names: one per line, a name pitch apart (baselines, whatever the macrons and
    /// descenders), the header in the role column; inside the black band under the view.
    #[test]
    fn several_names_stack_without_overlap() {
        let r = roll();
        let s = r.layout(&Fake, &[(4, 24, 360)], BANDS).unwrap();
        let ys: Vec<f32> = ["Akira Iwai", "Yūichi Ōnogi", "Tetsushi Abe"]
            .iter()
            .map(|n| baseline(&s, n, 20.0))
            .collect();
        for w in ys.windows(2) {
            assert!((w[1] - w[0] - NAME_PITCH * 20.0).abs() < 1e-3, "{ys:?}");
        }
        assert_eq!(rect_of(&s, "Akira Iwai")[0], 25.0);
        assert_eq!(rect_of(&s, "Tetsushi Abe")[0], 25.0);
        assert!(rect_of(&s, "Reference Materials")[0] > 25.0 + Fake.extent("Tetsushi Abe", 20.0).0);
        // The stack (three name pitches) is clamped below the view.
        let top = ys[0] - (NAME_PITCH * 20.0 + 20.0) / 2.0;
        assert!(top >= BANDS.view.1 + BAND_MARGIN - 1e-3, "{top}");
        let two = r.layout(&Fake, &[(3, 24, 360)], BANDS).unwrap();
        let k = baseline(&two, "Masayuki Kikuchi", 20.0);
        let y = baseline(&two, "Tetsuyoshi Yamaji", 20.0);
        assert!((y - k - NAME_PITCH * 20.0).abs() < 1e-3);
        assert_eq!(rect_of(&two, "Tetsuyoshi Yamaji")[0], 27.0);
    }

    /// A company with no person named is the credited party: name size, its header above it
    /// in the role size, both from the left.
    #[test]
    fn company_alone_takes_the_name_style_under_its_header() {
        let r = roll();
        let s = r.layout(&Fake, &[(5, 24, 360)], BANDS).unwrap();
        assert_eq!(style_of(&s, "Hokuetsu Express Corporation").cap, Some(20.0));
        assert_eq!(style_of(&s, "Thanks to").cap, Some(10.0));
        assert_eq!(rect_of(&s, "Thanks to")[0], 26.0);
        assert_eq!(rect_of(&s, "Hokuetsu Express Corporation")[0], 26.0);
        assert!(
            baseline(&s, "Thanks to", 10.0)
                < baseline(&s, "Hokuetsu Express Corporation", 20.0) - 20.0
        );
    }

    /// Next to names a company is their affiliation, in the role style with its logo before
    /// it, wrapped when the screen is too wide; a logo kept after a part gets its own row.
    #[test]
    fn company_beside_names_is_a_role_with_its_logo() {
        use Kind::*;
        let mut r = roll();
        // Roles as big as the names, so the company does not fit on one line.
        r.role_cap = 20.0;
        let mut jr = card(
            (464, 64),
            vec![
                part(Role, "Production Support", [2, 0, 200, 20]),
                part(
                    Feature,
                    "JR East Marketing & Communications, Inc.",
                    [78, 21, 385, 40],
                ),
            ],
        );
        jr.kept.push(([8, 25, 60, 33], Place::Before(1)));
        r.cards.insert(6, jr);
        r.cards.insert(
            7,
            card(
                (160, 48),
                vec![part(Name, "Yoshikazu Aizu", [6, 3, 149, 41])],
            ),
        );
        r.cards.insert(
            8,
            card(
                (160, 48),
                vec![part(Name, "Takeshi Kawata", [6, 3, 149, 41])],
            ),
        );
        let s = r
            .layout(&Fake, &[(6, 24, 360), (7, 17, 424), (8, 465, 424)], BANDS)
            .unwrap();
        assert_eq!(rect_of(&s, "Yoshikazu Aizu")[0], 23.0);
        assert_eq!(rect_of(&s, "Takeshi Kawata")[0], 23.0);
        let header = rect_of(&s, "Production Support");
        assert_eq!(style_of(&s, "Production Support").cap, Some(20.0));
        let art = s
            .items
            .iter()
            .find_map(|i| match i {
                Item::Art { rect, tbl: 6, src } => Some((*rect, *src)),
                _ => None,
            })
            .unwrap();
        assert_eq!(art.1, [8, 25, 60, 33]);
        assert_eq!(art.0[0], header[0]);
        // 40 characters × 10 px would run past the margin: two balanced lines after the logo.
        let lines: Vec<&str> = s.texts().into_iter().map(|t| t.0).collect();
        assert!(lines.contains(&"JR East Marketing &"), "{lines:?}");
        assert!(lines.contains(&"Communications, Inc."), "{lines:?}");
        let company = rect_of(&s, "Communications, Inc.");
        assert_eq!(company[0], header[0] + 60.0 + ART_GAP * 20.0);
        assert_eq!(style_of(&s, "Communications, Inc.").cap, Some(20.0));
        for (t, rect) in s.texts() {
            assert!(
                rect[0] + Fake.extent(t, 20.0).0.min(rect[2]) <= 624.0,
                "{t}"
            );
        }
    }

    /// A screen too wide for its place (like the plain ending's credits at x 264) moves left to
    /// end at the right margin, keeping its layout.
    #[test]
    fn wide_screen_moves_left() {
        let r = roll();
        let at = r.layout(&Fake, &[(1, 24, 360)], BANDS).unwrap();
        let moved = r.layout(&Fake, &[(1, 400, 340)], BANDS).unwrap();
        let role = "Planning, Character Design";
        let right = rect_of(&moved, role)[0] + Fake.extent(role, 10.0).0;
        assert!((right - (640.0 - SIDE_MARGIN)).abs() < 1e-3, "{right}");
        let d = rect_of(&at, "Akira Saitō")[0] - rect_of(&moved, "Akira Saitō")[0];
        assert!((rect_of(&at, role)[0] - rect_of(&moved, role)[0] - d).abs() < 1e-3);
    }

    /// Above the view (the plain ending's top credits) the text stays above it.
    #[test]
    fn credits_above_the_view_stay_above_it() {
        let r = roll();
        let s = r.layout(&Fake, &[(3, 24, 62)], BANDS).unwrap();
        let last = baseline(&s, "Tetsuyoshi Yamaji", 20.0);
        assert!(last + (NAME_PITCH * 20.0 - 20.0) / 2.0 <= BANDS.view.0 - BAND_MARGIN + 1e-3);
    }

    /// The names shrink until every screen fits; a credit without a card keeps its sprite.
    #[test]
    fn fit_shrinks_the_names_only_as_needed() {
        let mut r = roll();
        r.name_cap = 40.0;
        let screens: Vec<(Vec<Shown>, Bands)> =
            (1..=5).map(|t| (vec![(t, 24, 360)], BANDS)).collect();
        assert!(r.fit(&Fake, &screens));
        assert!(r.name_cap < 40.0 && r.name_cap >= MIN_NAME_OF_ROLE * 10.0);
        let fitted = r.name_cap;
        r.name_cap += FIT_STEP;
        assert!(
            screens
                .iter()
                .any(|(s, b)| r.layout(&Fake, s, *b).is_none())
        );
        r.name_cap = fitted;
        assert!(r.layout(&Fake, &[(9, 24, 360)], BANDS).is_none());
        assert!(r.layout(&Fake, &[], BANDS).is_none());
        let mut tiny = roll();
        tiny.role_cap = 100.0;
        assert!(!tiny.fit(&Fake, &screens));
        assert_eq!(tiny.name_cap, 20.0);
    }

    /// The title card keeps its art where it is, 高速編 at the name size on its row.
    #[test]
    fn title_keeps_its_layout() {
        let mut r = roll();
        let mut title = card(
            (464, 96),
            vec![part(Kind::Feature, "Kōsoku-hen", [300, 2, 163, 42])],
        );
        title.kept = vec![
            ([4, 6, 274, 85], Place::Stay),
            ([290, 57, 169, 27], Place::Stay),
        ];
        r.cards.insert(0x3a8, title);
        let s = r.layout(&Fake, &[(0x3a8, 24, 360)], BANDS).unwrap();
        let arts: Vec<[f32; 4]> = s
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Art { rect, .. } => Some(*rect),
                Item::Text { .. } => None,
            })
            .collect();
        assert_eq!(
            arts,
            [[28.0, 366.0, 274.0, 85.0], [314.0, 417.0, 169.0, 27.0]]
        );
        assert_eq!(rect_of(&s, "Kōsoku-hen")[0], 324.0);
        let (_, t, b) = Fake.extent("Kōsoku-hen", 20.0);
        let mid = baseline(&s, "Kōsoku-hen", 20.0) + (t + b) / 2.0;
        assert!((mid - (362.0 + 21.0)).abs() < 1e-3);
    }
}
