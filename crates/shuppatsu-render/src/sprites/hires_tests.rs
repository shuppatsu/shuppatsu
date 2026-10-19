//! Enhancement `hud_upscale` through [`SpriteLayer::compose`].

use super::*;

/// Tile 1: index 2 on and below the diagonal, transparent above. Tile 2: index 3 left of a
/// steep line, 4 right of it. Tile 3: all transparent.
fn tiles() -> Vec<Option<Box<[u8; TILE_PIXELS]>>> {
    let tile = |f: &dyn Fn(usize, usize) -> u8| {
        Some(Box::new(std::array::from_fn(|i| {
            f(i % TILE_SIZE, i / TILE_SIZE)
        })))
    };
    vec![
        None,
        tile(&|x, y| if x <= y { 2 } else { 0 }),
        tile(&|x, y| if 2 * x <= y + 8 { 3 } else { 4 }),
        tile(&|_, _| 0),
    ]
}

fn bank(map: Tilemap) -> SpriteBank {
    SpriteBank {
        tiles: tiles(),
        tilemaps: vec![map],
        generation: 0,
        text: None,
        labels: None,
        display_labels: None,
        legend_font: None,
        wide_fill: Vec::new(),
        paper: Mutex::default(),
        upscale: false,
        hires: Mutex::default(),
    }
}

/// 3×2 tiles with edges crossing the tile borders.
fn map() -> Tilemap {
    Tilemap {
        width: 3,
        height: 2,
        tiles: vec![1, 2, 3, 2, 1, 0],
    }
}

/// Palette A, and B: the same indices in other colours.
fn palettes() -> (Palette, Palette) {
    let mut a = [0u32; 256];
    let mut b = [0u32; 256];
    for i in 1..256u32 {
        a[i as usize] = ((i * 0x0035_1b4f) ^ 0x00c0_c0c0) & 0x00ff_ffff;
        b[i as usize] = ((i * 0x0029_6b05) ^ 0x00ab_cdef) & 0x00ff_ffff;
    }
    (Palette(a), Palette(b))
}

fn layer(scale: usize, slot: Slot) -> SpriteLayer {
    let (a, b) = palettes();
    let mut l = SpriteLayer {
        scale,
        ..SpriteLayer::default()
    };
    l.palettes[0] = Some(a);
    l.palettes[1] = Some(b);
    l.slots[0] = slot;
    l
}

/// Composed twice: an upscaled image is built at the second draw with the same colours.
fn compose(b: &SpriteBank, l: &SpriteLayer) -> Vec<u8> {
    let mut c = l.new_canvas();
    l.compose(b, &mut c);
    l.compose(b, &mut c);
    c
}

fn whole(x: i16, palette: u8) -> Slot {
    Slot::new(0, x, 20, palette, 0, true)
}

#[test]
fn off_draws_blocks_and_other_multiples_keep_them() {
    let mut b = bank(map());
    let one = compose(&b, &layer(1, whole(10, 0)));
    for s in [1, 3, 5] {
        let off = compose(&b, &layer(s, whole(10, 0)));
        // Each original pixel an s × s block of the 1× compose.
        let (w, _) = layer(s, whole(10, 0)).canvas_size();
        for (i, px) in off.chunks(4).enumerate() {
            let (x, y) = ((i % w) / s, (i / w) / s);
            let o = (y * CANVAS_W + x) * 4;
            assert_eq!(px, &one[o..o + 4], "×{s} at {i}");
        }
        b.set_upscale(true);
        let on = compose(&b, &layer(s, whole(10, 0)));
        b.set_upscale(false);
        assert_eq!(on == off, s == 1 || s == 5, "×{s}");
    }
    assert_eq!(b.upscale_builds(), 1);
}

#[test]
fn on_shapes_edges_with_the_source_indices() {
    let mut b = bank(map());
    b.set_upscale(true);
    let (a, _) = palettes();
    for s in 2..=4 {
        let on = compose(&b, &layer(s, whole(10, 0)));
        let allowed: Vec<[u8; 4]> = [0, 2, 3, 4].map(|i| a.rgba(i)).to_vec();
        assert!(
            on.chunks(4)
                .all(|p| allowed.contains(&p.try_into().unwrap()))
        );
        // Pixels both ways across the diagonal changed: the staircase is smoothed.
        let off = {
            let mut b2 = bank(map());
            b2.set_upscale(false);
            compose(&b2, &layer(s, whole(10, 0)))
        };
        let changed = |from: u8, to: u8| {
            on.chunks(4)
                .zip(off.chunks(4))
                .any(|(n, o)| o == a.rgba(from) && n == a.rgba(to))
        };
        assert!(changed(0, 2) && changed(2, 0), "×{s}");
    }
}

#[test]
fn palette_swaps_change_only_colours() {
    let mut b = bank(map());
    b.set_upscale(true);
    let (pa, pb) = palettes();
    let a = compose(&b, &layer(4, whole(10, 0)));
    let bb = compose(&b, &layer(4, whole(10, 1)));
    let index_of = |p: &Palette, px: &[u8]| (0..=255u8).find(|&i| p.rgba(i) == px).unwrap();
    for (x, y) in a.chunks(4).zip(bb.chunks(4)) {
        assert_eq!(index_of(&pa, x), index_of(&pb, y));
    }
    // One build: the second palette reuses the shapes.
    assert_eq!(b.upscale_builds(), 1);
}

#[test]
fn sub_rects_sample_the_whole_image_at_their_offset() {
    let mut b = bank(map());
    b.set_upscale(true);
    let s = 3;
    let full = compose(&b, &layer(s, whole(10, 0)));
    // Column 1, both rows, drawn where the whole map draws it.
    let mut slot = whole(26, 0);
    (slot.col, slot.row, slot.w, slot.h) = (1, 0, 1, 2);
    let part = compose(&b, &layer(s, slot));
    let (w, h) = layer(s, slot).canvas_size();
    let inside =
        |x: usize, y: usize| (26 * s..42 * s).contains(&x) && (20 * s..52 * s).contains(&y);
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 4;
            let want = if inside(x, y) {
                &full[o..o + 4]
            } else {
                &[0; 4][..]
            };
            assert_eq!(&part[o..o + 4], want, "({x}, {y})");
        }
    }
}

#[test]
fn index_zero_stays_transparent() {
    let mut b = bank(Tilemap {
        width: 2,
        height: 1,
        tiles: vec![3, 0],
    });
    b.set_upscale(true);
    assert!(compose(&b, &layer(4, whole(10, 0))).iter().all(|&v| v == 0));
    // Around the art, far from its edges, nothing is drawn either.
    let mut b = bank(map());
    b.set_upscale(true);
    let c = compose(&b, &layer(4, whole(10, 0)));
    let (w, _) = layer(4, whole(10, 0)).canvas_size();
    let alpha = |x: usize, y: usize| c[(y * w + x) * 4 + 3];
    // The empty cell (2, 1) at original (42..58, 36..52) away from its left edge.
    assert!((44 * 4..58 * 4).all(|x| (38 * 4..52 * 4).all(|y| alpha(x, y) == 0)));
    // Tile 1 at (10, 20): above its diagonal stays clear, below it is drawn.
    assert_eq!(alpha((10 + 12) * 4, (20 + 2) * 4), 0);
    assert_eq!(alpha((10 + 2) * 4, (20 + 12) * 4), 255);
}

#[test]
fn cached_until_the_tilemap_changes() {
    let mut b = bank(map());
    b.set_upscale(true);
    let first = compose(&b, &layer(2, whole(10, 0)));
    assert_eq!(compose(&b, &layer(2, whole(10, 0))), first);
    assert_eq!(b.upscale_builds(), 1);
    b.tilemaps[0].tiles[2] = 1;
    let changed = compose(&b, &layer(2, whole(10, 0)));
    assert_eq!(b.upscale_builds(), 2);
    assert_ne!(changed, first);
    b.clear_upscaled();
    compose(&b, &layer(2, whole(10, 0)));
    assert_eq!(b.upscale_builds(), 3);
}

#[test]
fn wide_repeats_have_no_seams() {
    // A backdrop of 41 columns of tiles 2 (a period-1 pattern with diagonal edges across the
    // tile borders) continued on a wide canvas.
    let mut b = bank(Tilemap {
        width: 41,
        height: 1,
        tiles: vec![2; 41],
    });
    assert!(b.set_wide_fill(0));
    b.set_upscale(true);
    let s = 3;
    let mut l = layer(s, Slot::new(0, 0, 20, 0, 3, true));
    l.wide = 200 * s;
    let c = compose(&b, &l);
    let (w, _) = l.canvas_size();
    // Every tile column the same, from the left edge of the canvas to the right one.
    let t = TILE_SIZE * s;
    let x0 = (w / 2 - CANVAS_W * s / 2) % t;
    for y in 20 * s..36 * s {
        for x in x0..w - t {
            let o = (y * w + x) * 4;
            assert_eq!(c[o..o + 4], c[o + t * 4..o + t * 4 + 4], "({x}, {y})");
        }
    }
}
