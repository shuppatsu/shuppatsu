//! Synthetic-stream tests: a tiny test-only encoder writes I, P, B and D pictures whose
//! decoded pixels are computed independently here (flat DC blocks, one AC coefficient through
//! a f64 IDCT, and motion compensation evaluated from its definition).

use super::*;
use crate::mpeg::vtables::{CBP, DC_CHROMA, DC_LUMA, MB_TYPE_B, MB_TYPE_P, MBA, MOTION};

#[derive(Default)]
pub(crate) struct BitWriter {
    pub bytes: Vec<u8>,
    bits: u32,
    acc: u32,
}

impl BitWriter {
    pub fn put(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            self.acc = self.acc << 1 | (value >> i & 1);
            self.bits += 1;
            if self.bits == 8 {
                self.bytes.push(self.acc as u8);
                self.acc = 0;
                self.bits = 0;
            }
        }
    }

    pub fn code(&mut self, s: &str) {
        for c in s.chars() {
            self.put(u32::from(c == '1'), 1);
        }
    }

    /// Zero-pads to a byte boundary and writes a start code.
    pub fn start(&mut self, code: u8) {
        while self.bits != 0 {
            self.put(0, 1);
        }
        self.bytes.extend_from_slice(&[0, 0, 1, code]);
    }

    /// The bytes so far, zero-padded to a byte boundary.
    pub fn padded(mut self) -> Vec<u8> {
        while self.bits != 0 {
            self.put(0, 1);
        }
        self.bytes
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.start(SEQUENCE_END);
        self.bytes
    }
}

fn find<T: PartialEq + Copy>(table: &[(&'static str, T)], v: T) -> &'static str {
    table.iter().find(|e| e.1 == v).expect("code in table").0
}

const W: usize = 32;
const H: usize = 32;

fn sequence(w: &mut BitWriter, non_intra: Option<[u8; 64]>) {
    w.start(SEQUENCE_HEADER);
    w.put(W as u32, 12);
    w.put(H as u32, 12);
    w.put(12, 4); // aspect: CCIR 601 525-line
    w.put(4, 4); // 29.97 Hz
    w.put(5000, 18);
    w.put(1, 1);
    w.put(20, 10);
    w.put(0, 1);
    w.put(0, 1); // default intra matrix
    match non_intra {
        Some(m) => {
            w.put(1, 1);
            for &z in &ZIGZAG {
                w.put(u32::from(m[usize::from(z)]), 8);
            }
        }
        None => w.put(0, 1),
    }
    w.start(GOP);
    w.put(1 << 12, 25);
    w.put(1, 1); // closed
    w.put(0, 1);
}

fn picture(w: &mut BitWriter, tref: u32, kind: u32, f_codes: &[u32]) {
    w.start(PICTURE);
    w.put(tref, 10);
    w.put(kind, 3);
    w.put(0xffff, 16);
    for &f in f_codes {
        w.put(0, 1); // half-pel
        w.put(f, 3);
    }
    w.put(0, 1);
}

fn slice(w: &mut BitWriter, row: u8, qscale: u32) {
    w.start(row + 1);
    w.put(qscale, 5);
    w.put(0, 1);
}

/// dct_dc_size + differential for a difference `d` (in sample units).
fn dc(w: &mut BitWriter, luma: bool, d: i32) {
    let size = 32 - d.unsigned_abs().leading_zeros();
    w.code(find(if luma { DC_LUMA } else { DC_CHROMA }, size as u8));
    if size > 0 {
        let bits = if d > 0 { d } else { d + (1 << size) - 1 };
        w.put(bits as u32, size);
    }
}

fn motion(w: &mut BitWriter, code: i8) {
    if code == 0 {
        w.code("1");
    } else {
        w.code(find(MOTION, code.abs()));
        w.put(u32::from(code < 0), 1);
    }
}

/// Encoded sample values: DC level of each block of each macroblock.
fn levels(mb: usize) -> [i32; 6] {
    [
        40 + 30 * mb as i32,
        70 + 25 * mb as i32,
        100 + 20 * mb as i32,
        130 + 10 * mb as i32,
        90 + 15 * mb as i32,
        160 - 12 * mb as i32,
    ]
}

/// Writes an intra picture body: flat blocks at `levels`, plus one escaped AC coefficient
/// (run 0, level `ac`) in block 0 of macroblock 0.
fn intra_slices(w: &mut BitWriter, ac: i32) {
    slice(w, 0, 8);
    let mut pred = [128i32; 3];
    for mb in 0..4 {
        w.code("1"); // address increment 1
        w.code("1"); // intra
        let l = levels(mb);
        for (b, &level) in l.iter().enumerate() {
            let comp = if b < 4 { 0 } else { b - 3 };
            dc(w, b < 4, level - pred[comp]);
            pred[comp] = level;
            if mb == 0 && b == 0 && ac != 0 {
                w.code("000001");
                w.put(0, 6);
                w.put(ac as u8 as u32, 8);
            }
            w.code("10");
        }
    }
}

/// Reference planes of the intra picture.
struct Planes {
    y: Vec<i32>,
    cb: Vec<i32>,
    cr: Vec<i32>,
}

fn intra_reference(ac: i32) -> Planes {
    let mut p = Planes {
        y: vec![0; W * H],
        cb: vec![0; W * H / 4],
        cr: vec![0; W * H / 4],
    };
    for mb in 0..4 {
        let (mx, my) = (mb % 2, mb / 2);
        let l = levels(mb);
        for (b, &level) in l.iter().enumerate().take(4) {
            let (bx, by) = (mx * 16 + (b % 2) * 8, my * 16 + (b / 2) * 8);
            for y in 0..8 {
                for x in 0..8 {
                    p.y[(by + y) * W + bx + x] = level;
                }
            }
        }
        for y in 0..8 {
            for x in 0..8 {
                p.cb[(my * 8 + y) * (W / 2) + mx * 8 + x] = l[4];
                p.cr[(my * 8 + y) * (W / 2) + mx * 8 + x] = l[5];
            }
        }
    }
    if ac != 0 {
        // F(0,0) = 8·level, F(0,1) = (2·ac·q·16)/16 oddified, q = 8; the f64 IDCT of the pair.
        let mut f1 = 2 * ac * 8 * 16 / 16;
        if f1 & 1 == 0 {
            f1 -= f1.signum();
        }
        let f0 = 8 * levels(0)[0];
        for y in 0..8 {
            for x in 0..8 {
                let c0 = std::f64::consts::FRAC_1_SQRT_2;
                let cos = ((2 * x + 1) as f64 * std::f64::consts::PI / 16.0).cos();
                let v = f64::from(f0) * c0 * c0 / 4.0 + f64::from(f1) * c0 / 4.0 * cos;
                p.y[y * W + x] = (v.round() as i32).clamp(0, 255);
            }
        }
    }
    p
}

/// Motion-compensated sample from its definition: average of the up to four integer samples
/// around the half-pel position, rounded half up; edges clamped.
fn mc(plane: &[i32], stride: usize, rows: usize, x: i32, y: i32, mv: (i32, i32)) -> i32 {
    let fx = 2 * x + mv.0;
    let fy = 2 * y + mv.1;
    let at = |xx: i32, yy: i32| {
        plane[yy.clamp(0, rows as i32 - 1) as usize * stride
            + xx.clamp(0, stride as i32 - 1) as usize]
    };
    let (x0, y0) = (fx.div_euclid(2), fy.div_euclid(2));
    let xs: &[i32] = if fx % 2 != 0 { &[x0, x0 + 1] } else { &[x0] };
    let ys: &[i32] = if fy % 2 != 0 { &[y0, y0 + 1] } else { &[y0] };
    let mut sum = 0;
    for &yy in ys {
        for &xx in xs {
            sum += at(xx, yy);
        }
    }
    let n = (xs.len() * ys.len()) as i32;
    (sum + n / 2) / n
}

/// Prediction of the whole picture from per-macroblock vectors (None = both refs averaged
/// with the given pair).
fn predict_ref(refp: &Planes, vectors: &[(i32, i32); 4]) -> Planes {
    let mut out = Planes {
        y: vec![0; W * H],
        cb: vec![0; W * H / 4],
        cr: vec![0; W * H / 4],
    };
    for (mb, &mv) in vectors.iter().enumerate() {
        let (mx, my) = ((mb % 2) as i32, (mb / 2) as i32);
        for y in 0..16 {
            for x in 0..16 {
                let (px, py) = (mx * 16 + x, my * 16 + y);
                out.y[py as usize * W + px as usize] = mc(&refp.y, W, H, px, py, mv);
            }
        }
        let cmv = (mv.0 / 2, mv.1 / 2);
        for y in 0..8 {
            for x in 0..8 {
                let (px, py) = (mx * 8 + x, my * 8 + y);
                let i = py as usize * (W / 2) + px as usize;
                out.cb[i] = mc(&refp.cb, W / 2, H / 2, px, py, cmv);
                out.cr[i] = mc(&refp.cr, W / 2, H / 2, px, py, cmv);
            }
        }
    }
    out
}

fn average(a: &Planes, b: &Planes) -> Planes {
    let avg = |x: &[i32], y: &[i32]| x.iter().zip(y).map(|(p, q)| (p + q + 1) >> 1).collect();
    Planes {
        y: avg(&a.y, &b.y),
        cb: avg(&a.cb, &b.cb),
        cr: avg(&a.cr, &b.cr),
    }
}

fn assert_frame(f: &Frame, want: &Planes, what: &str) {
    let y: Vec<i32> = f.y_plane().into_iter().map(i32::from).collect();
    let (cb, cr) = f.chroma_planes();
    let cb: Vec<i32> = cb.into_iter().map(i32::from).collect();
    let cr: Vec<i32> = cr.into_iter().map(i32::from).collect();
    assert_eq!(y, want.y, "{what}: luma");
    assert_eq!(cb, want.cb, "{what}: cb");
    assert_eq!(cr, want.cr, "{what}: cr");
}

/// I (tref 0), P (tref 2), B (tref 1) in decode order.
pub(crate) fn ipb_stream() -> Vec<u8> {
    let mut w = BitWriter::default();
    sequence(&mut w, None);
    picture(&mut w, 0, 1, &[]);
    intra_slices(&mut w, 3);

    // P: one slice; MB0 forward (3, 2) not coded, MB1 and MB2 skipped, MB3 forward (−1, −3).
    picture(&mut w, 2, 2, &[1]);
    slice(&mut w, 0, 8);
    let p_mc = find(MB_TYPE_P, MB_FWD);
    w.code(find(MBA, 1));
    w.code(p_mc);
    motion(&mut w, 3);
    motion(&mut w, 2);
    w.code(find(MBA, 3));
    w.code(p_mc);
    motion(&mut w, -1);
    motion(&mut w, -3);

    // B: MB0 interpolated fwd (1, 0) bwd (0, 1); MB1, MB2 skipped (same); MB3 forward only,
    // vector predictor (1, 0) + delta (−2, 1) = (−1, 1).
    picture(&mut w, 1, 3, &[1, 1]);
    slice(&mut w, 0, 8);
    w.code(find(MBA, 1));
    w.code(find(MB_TYPE_B, MB_FWD | MB_BWD));
    motion(&mut w, 1);
    motion(&mut w, 0);
    motion(&mut w, 0);
    motion(&mut w, 1);
    w.code(find(MBA, 3));
    w.code(find(MB_TYPE_B, MB_FWD));
    motion(&mut w, -2);
    motion(&mut w, 1);
    w.finish()
}

#[test]
fn decodes_i_p_b_in_display_order() {
    let mut d = VideoDecoder::from_bytes(ipb_stream());
    let i = d.next_frame().unwrap().unwrap();
    let b = d.next_frame().unwrap().unwrap();
    let p = d.next_frame().unwrap().unwrap();
    assert!(d.next_frame().unwrap().is_none());
    assert_eq!(
        (i.kind, b.kind, p.kind),
        (PictureType::I, PictureType::B, PictureType::P)
    );
    assert_eq!(
        (
            i.temporal_reference,
            b.temporal_reference,
            p.temporal_reference
        ),
        (0, 1, 2)
    );
    let seq = d.sequence().unwrap();
    assert_eq!(
        (seq.width, seq.height, seq.frame_rate()),
        (32, 32, (30_000, 1001))
    );
    assert!((seq.sample_aspect() - 200.0 / 219.0).abs() < 1e-4);

    let iref = intra_reference(3);
    assert_frame(&i, &iref, "I");
    let pref = predict_ref(&iref, &[(3, 2), (0, 0), (0, 0), (-1, -3)]);
    assert_frame(&p, &pref, "P");
    let fwd = predict_ref(&iref, &[(1, 0), (1, 0), (1, 0), (-1, 1)]);
    let bwd = predict_ref(&pref, &[(0, 1), (0, 1), (0, 1), (0, 0)]);
    let mut bref = average(&fwd, &bwd);
    // MB3 is forward-only: copy its region from `fwd`.
    for y in 16..32 {
        for x in 16..32 {
            bref.y[y * W + x] = fwd.y[y * W + x];
        }
    }
    for y in 8..16 {
        for x in 8..16 {
            bref.cb[y * 16 + x] = fwd.cb[y * 16 + x];
            bref.cr[y * 16 + x] = fwd.cr[y * 16 + x];
        }
    }
    assert_frame(&b, &bref, "B");
}

#[test]
fn coded_residual_uses_the_non_intra_matrix() {
    // P picture with one coded macroblock: cbp = block 0 only, first coefficient "1s" (run 0,
    // level +1) at DC, with a custom non-intra matrix entry of 40 at DC.
    let mut m = [16u8; 64];
    m[0] = 40;
    let mut w = BitWriter::default();
    sequence(&mut w, Some(m));
    picture(&mut w, 0, 1, &[]);
    intra_slices(&mut w, 0);
    picture(&mut w, 1, 2, &[1]);
    slice(&mut w, 0, 4);
    w.code(find(MBA, 1));
    w.code(find(MB_TYPE_P, MB_PATTERN)); // coded, no motion compensation
    w.code(find(CBP, 32));
    w.code("10"); // dct_coeff_first "1s", s = 0
    w.code("10"); // end of block
    let mut d = VideoDecoder::from_bytes(w.finish());
    let i = d.next_frame().unwrap().unwrap();
    let p = d.next_frame().unwrap().unwrap();
    // ((2·1 + 1)·4·40)/16 = 30, even → 29; a DC-only IDCT gives round(29/8) = 4.
    let mut want = intra_reference(0);
    for y in 0..8 {
        for x in 0..8 {
            want.y[y * W + x] += 4;
        }
    }
    assert_frame(&i, &intra_reference(0), "I");
    assert_eq!(p.kind, PictureType::P);
    // Only the first macroblock is coded; the rest of the picture is missing from the slice,
    // so it conceals from the reference, i.e. equals it.
    assert_frame(&p, &want, "P");
}

#[test]
fn decodes_a_d_picture() {
    let mut w = BitWriter::default();
    sequence(&mut w, None);
    picture(&mut w, 0, 4, &[]);
    slice(&mut w, 0, 8);
    let mut pred = [128i32; 3];
    for mb in 0..4 {
        w.code("1");
        w.code("1");
        for (b, &level) in levels(mb).iter().enumerate() {
            let comp = if b < 4 { 0 } else { b - 3 };
            dc(&mut w, b < 4, level - pred[comp]);
            pred[comp] = level;
        }
        w.code("1"); // end_of_macroblock
    }
    let mut d = VideoDecoder::from_bytes(w.finish());
    let f = d.next_frame().unwrap().unwrap();
    assert_eq!(f.kind, PictureType::D);
    assert_frame(&f, &intra_reference(0), "D");
    assert!(d.next_frame().unwrap().is_none());
}

#[test]
fn rgba_conversion_maps_video_levels() {
    let mut f = Frame::new(2, 2);
    f.y.fill(235);
    f.cb.fill(128);
    f.cr.fill(128);
    let mut out = Vec::new();
    f.to_rgba(&mut out);
    assert_eq!(&out[..4], &[255, 255, 255, 255]);
    f.y.fill(16);
    f.to_rgba(&mut out);
    assert_eq!(&out[..4], &[0, 0, 0, 255]);
    // Pure red in BT.601 video levels: Y 81, Cb 90, Cr 240.
    f.y.fill(81);
    f.cb.fill(90);
    f.cr.fill(240);
    f.to_rgba(&mut out);
    assert!(
        out[0] >= 254 && out[1] <= 1 && out[2] <= 1,
        "{:?}",
        &out[..4]
    );
}

/// Drains a decoder, tolerating errors, with a bound on the number of calls.
fn drain(data: Vec<u8>) -> usize {
    let mut d = VideoDecoder::from_bytes(data);
    let mut frames = 0;
    for _ in 0..1000 {
        match d.next_frame() {
            Ok(Some(_)) => frames += 1,
            Ok(None) => return frames,
            Err(_) => {}
        }
    }
    panic!("decoder did not finish");
}

#[test]
fn truncated_and_corrupted_streams_never_panic() {
    let s = ipb_stream();
    for cut in 0..s.len() {
        drain(s[..cut].to_vec());
    }
    let mut seed = 0x9e37_79b9u32;
    for _ in 0..3000 {
        let mut bad = s.clone();
        let flips = 1 + (seed % 6) as usize;
        for _ in 0..flips {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let i = (seed >> 8) as usize % bad.len();
            bad[i] ^= 1 << (seed >> 29);
        }
        drain(bad);
    }
    // Random bytes behind a valid sequence header.
    for n in 0..200u32 {
        let mut w = BitWriter::default();
        sequence(&mut w, None);
        picture(&mut w, 0, 1 + n % 4, &[1, 1][..(n % 4).min(2) as usize]);
        slice(&mut w, 0, 8);
        let mut data = w.bytes;
        let mut x = n.wrapping_mul(2_654_435_761).wrapping_add(1);
        for _ in 0..300 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            data.push(x as u8);
        }
        drain(data);
    }
}

#[test]
fn errors_are_reported_and_decoding_continues() {
    let mut s = ipb_stream();
    // Break the P picture's first macroblock type (the byte after its slice header).
    let p_pic = s
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == [0, 0, 1, 0])
        .map(|(i, _)| i)
        .nth(1)
        .unwrap();
    let slice_at = p_pic
        + 4
        + s[p_pic + 4..]
            .windows(4)
            .position(|w| w == [0, 0, 1, 1])
            .unwrap();
    s[slice_at + 5] = 0;
    s[slice_at + 6] = 0;
    let mut d = VideoDecoder::from_bytes(s);
    let mut kinds = Vec::new();
    let mut errors = 0;
    loop {
        match d.next_frame() {
            Ok(Some(f)) => kinds.push(f.kind),
            Ok(None) => break,
            Err(_) => errors += 1,
        }
    }
    assert_eq!(errors, 1);
    assert_eq!(kinds, vec![PictureType::I, PictureType::B, PictureType::P]);
}
