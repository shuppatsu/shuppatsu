//! Enhancement `hud_upscale`: the sprite art enlarged with index-preserving xBRZ
//! ([`super::xbrz::scale_indices`]) instead of whole-pixel blocks, at canvas multiples 2–4.
//!
//! A tilemap is upscaled as one image (not tile by tile), so edges that cross tile borders
//! are shaped as one line and no seams appear. The result is palette indices, drawn with a
//! palette lookup per canvas pixel like the blocks: palette swaps, animated entries and fades
//! keep working. Images are built on first use and kept per (tilemap, multiple, view) in
//! [`Cache`]; an entry whose tilemap was replaced is built again.
//!
//! The xBRZ decisions need colours ([`super::xbrz::IndexMetric`]): they come from the palette
//! the map is drawn with, once its colours have settled. A palette fading in from black would
//! make poor decisions, so the build waits (drawing the blocks) until the map's colours repeat
//! a state seen at an earlier draw: the end of a fade, a blink or the speed readout's segments
//! coming round again. After that the shapes stay; palette effects only change colours.

use super::xbrz::{self, IndexMetric};
use ddg_formats::cg::{Palette, TILE_SIZE, Tilemap};
use std::sync::Arc;

/// The canvas multiples with an upscaled image; others draw whole-pixel blocks.
pub const MULTIPLES: std::ops::RangeInclusive<usize> = 2..=4;
/// Colour states of a waiting map remembered (a blink has two).
const SEEN: usize = 16;
/// Index 0's reference colour: unlike any art colour (it is transparent).
const KEY_COLOUR: [u8; 3] = [255, 0, 255];
/// Upscaled images kept (bytes); the least recently drawn go first.
const BUDGET: usize = 96 << 20;

/// Which part of a tilemap an image covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// The whole map; slots showing a sub-rectangle sample it at their offset.
    Whole,
    /// Tile columns `first..=last` of a wide backdrop continued by its repeat
    /// (`wide_menus`): the columns as drawn side by side, so the joins are shaped too.
    Wide { first: i32, last: i32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub tbl: u16,
    pub scale: usize,
    pub view: View,
}

/// An upscaled view: `w × h` palette indices (canvas pixels), `scale` per art pixel.
#[derive(Debug, PartialEq, Eq)]
pub struct Hires {
    pub w: usize,
    pub h: usize,
    pub idx: Vec<u8>,
}

impl Hires {
    /// Row `y` of the image, `x0..x1`.
    pub fn row(&self, x0: usize, x1: usize, y: usize) -> &[u8] {
        &self.idx[y * self.w + x0..y * self.w + x1]
    }
}

enum State {
    /// Not built: the colour states (hashes) seen so far.
    Waiting(Vec<u64>),
    Ready(Arc<Hires>),
}

struct Entry {
    key: Key,
    /// The tilemap as built, to notice a replaced one.
    map: Tilemap,
    state: State,
    last_used: u64,
}

/// The upscaled images of a [`super::SpriteBank`].
#[derive(Default)]
pub struct Cache {
    entries: Vec<Entry>,
    clock: u64,
    /// Images built so far (for tests and diagnostics).
    pub builds: usize,
}

/// The art of a view in art pixels: `(w, h, indices)`.
pub type Art = (usize, usize, Vec<u8>);

impl Cache {
    /// The image of `key`, building it with `art` when missing; `None` while the map's
    /// colours have not settled (draw the blocks then).
    pub fn get(
        &mut self,
        key: Key,
        map: &Tilemap,
        palette: &Palette,
        art: impl FnOnce() -> Art,
    ) -> Option<Arc<Hires>> {
        self.clock += 1;
        let clock = self.clock;
        let at = self.entries.iter().position(|e| e.key == key);
        if let Some(i) = at {
            let e = &mut self.entries[i];
            if e.map == *map {
                e.last_used = clock;
                if let State::Ready(h) = &e.state {
                    return Some(h.clone());
                }
            } else {
                e.state = State::Waiting(Vec::new());
                e.map = map.clone();
            }
        }
        let (w, h, px) = art();
        let mut used = [false; 256];
        for &i in &px {
            used[usize::from(i)] = true;
        }
        let colours: [[u8; 3]; 256] = std::array::from_fn(|i| {
            let [r, g, b, _] = palette.rgba(i as u8);
            if i == 0 { KEY_COLOUR } else { [r, g, b] }
        });
        let hash = palette_hash(&colours, &used);
        let mut seen = match at.map(|i| &mut self.entries[i].state) {
            Some(State::Waiting(seen)) => std::mem::take(seen),
            _ => Vec::new(),
        };
        let state = if seen.contains(&hash) {
            let idx = xbrz::scale_indices(&px, w, h, key.scale, &IndexMetric::new(&colours));
            self.builds += 1;
            State::Ready(Arc::new(Hires {
                w: w * key.scale,
                h: h * key.scale,
                idx,
            }))
        } else {
            if seen.len() == SEEN {
                seen.remove(0);
            }
            seen.push(hash);
            State::Waiting(seen)
        };
        let out = match &state {
            State::Ready(h) => Some(h.clone()),
            State::Waiting(_) => None,
        };
        let entry = Entry {
            key,
            map: map.clone(),
            state,
            last_used: clock,
        };
        match at {
            Some(i) => self.entries[i] = entry,
            None => self.entries.push(entry),
        }
        self.evict();
        out
    }

    /// Drops everything (the bank's art changed).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Images built and kept.
    pub fn built(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.state, State::Ready(_)))
            .count()
    }

    /// Least recently drawn images out until the rest fit [`BUDGET`].
    fn evict(&mut self) {
        let size = |e: &Entry| match &e.state {
            State::Ready(h) => h.idx.len(),
            State::Waiting(_) => 0,
        };
        let mut total: usize = self.entries.iter().map(size).sum();
        while total > BUDGET && self.entries.len() > 1 {
            let Some(oldest) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(i, _)| i)
            else {
                break;
            };
            total -= size(&self.entries[oldest]);
            self.entries.swap_remove(oldest);
        }
    }
}

/// FNV-1a of the colours of the used indices.
fn palette_hash(colours: &[[u8; 3]; 256], used: &[bool; 256]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for (c, _) in colours.iter().zip(used).filter(|(_, u)| **u) {
        for &b in c {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// One tile's size in canvas pixels at `scale`.
pub fn tile_px(scale: usize) -> usize {
    TILE_SIZE * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(tiles: Vec<u16>) -> Tilemap {
        Tilemap {
            width: tiles.len() as u8,
            height: 1,
            tiles,
        }
    }

    fn grey(level: u8) -> Palette {
        let c = u32::from(level);
        Palette(std::array::from_fn(|i| {
            if i == 0 { 0 } else { c * 0x0001_0101 }
        }))
    }

    fn art() -> Art {
        // A diagonal of index 2 on index 1, 4×4.
        let px = (0..16)
            .map(|i| if i % 4 <= i / 4 { 2 } else { 1 })
            .collect();
        (4, 4, px)
    }

    fn key(scale: usize) -> Key {
        Key {
            tbl: 3,
            scale,
            view: View::Whole,
        }
    }

    #[test]
    fn built_once_per_key_and_rebuilt_for_a_new_map() {
        let mut c = Cache::default();
        let m = map(vec![1]);
        assert!(c.get(key(2), &m, &grey(200), art).is_none());
        let a = c.get(key(2), &m, &grey(200), art).unwrap();
        assert_eq!((a.w, a.h, a.idx.len()), (8, 8, 64));
        let b = c.get(key(2), &m, &grey(200), || unreachable!()).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(c.builds, 1);
        // Another multiple is another image.
        assert!(c.get(key(3), &m, &grey(200), art).is_none());
        assert_eq!(c.get(key(3), &m, &grey(200), art).unwrap().w, 12);
        assert_eq!((c.builds, c.built()), (2, 2));
        // A replaced tilemap is built again.
        assert!(c.get(key(2), &map(vec![2]), &grey(200), art).is_none());
        c.get(key(2), &map(vec![2]), &grey(200), art).unwrap();
        assert_eq!(c.builds, 3);
        c.clear();
        assert_eq!(c.built(), 0);
    }

    #[test]
    fn changing_colours_wait_until_they_repeat() {
        let mut c = Cache::default();
        let m = map(vec![1]);
        // A fade from black: every draw new, so blocks are drawn.
        for level in (0..=240).step_by(40) {
            assert!(c.get(key(4), &m, &grey(level), art).is_none());
        }
        // Settled: built from the final colours.
        assert!(c.get(key(4), &m, &grey(240), art).is_some());
        // A blink between two states is built when the first comes round again.
        let mut c = Cache::default();
        assert!(c.get(key(4), &m, &grey(90), art).is_none());
        assert!(c.get(key(4), &m, &grey(200), art).is_none());
        assert!(c.get(key(4), &m, &grey(90), art).is_some());
    }
}
