//! CPU mipmap generation for the `texture_filtering` enhancement.
//!
//! Each level halves the previous one with a 2×2 box filter. Colour is averaged weighted by
//! alpha, so the fully transparent colour-key texels (RGB565 0x0000, uploaded as alpha 0) don't
//! darken the edges of cut-out textures.

/// The full chain down to 1×1, level 0 first: `(width, height, rgba8)`.
pub fn chain(width: u32, height: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut levels = vec![(width, height, rgba.to_vec())];
    levels.extend(below(width, height, rgba));
    levels
}

/// The chain without level 0: level 1 first, empty for a 1×1 image.
pub fn below(width: u32, height: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut levels: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    let (mut w, mut h) = (width, height);
    while w > 1 || h > 1 {
        let next = half(w, h, levels.last().map_or(rgba, |l| &l.2));
        (w, h) = (next.0, next.1);
        levels.push(next);
    }
    levels
}

fn half(w: u32, h: u32, data: &[u8]) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let texel = |x: u32, y: u32| {
        let i = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
        [data[i], data[i + 1], data[i + 2], data[i + 3]].map(u32::from)
    };
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        for x in 0..nw {
            let px = [
                texel(2 * x, 2 * y),
                texel(2 * x + 1, 2 * y),
                texel(2 * x, 2 * y + 1),
                texel(2 * x + 1, 2 * y + 1),
            ];
            let alpha: u32 = px.iter().map(|p| p[3]).sum();
            for c in 0..3 {
                let v = px
                    .iter()
                    .map(|p| p[c] * p[3])
                    .sum::<u32>()
                    .checked_div(alpha)
                    .unwrap_or(0);
                out.push(v as u8);
            }
            out.push((alpha / 4) as u8);
        }
    }
    (nw, nh, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_sizes_and_keyed_average() {
        // 2×2: three opaque white texels and one transparent black one.
        let rgba = [
            255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0,
        ];
        let levels = chain(2, 2, &rgba);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[1].0, 1);
        // Colour stays white (the keyed texel doesn't darken it), alpha is 3/4.
        assert_eq!(levels[1].2, [255, 255, 255, 191]);
        assert_eq!(
            chain(4, 1, &[0; 16])
                .iter()
                .map(|l| (l.0, l.1))
                .collect::<Vec<_>>(),
            [(4, 1), (2, 1), (1, 1)]
        );
    }
}
