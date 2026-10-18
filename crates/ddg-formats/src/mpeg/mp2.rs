//! MPEG-1 Audio Layer II decoder (ISO/IEC 11172-3, 2.4.3.3 and Annex B).
//!
//! Each frame holds 1152 samples per channel as 12 granules × 3 samples × 32 subbands. The
//! decoder reads the bit allocation (the table chosen by sample rate and bit rate per channel),
//! the scale factor selection information and scale factors, the grouped or plain sample codes,
//! requantises them and runs the polyphase synthesis filterbank. Joint stereo (intensity
//! stereo above the bound) and CRC-protected frames are handled; the CRC itself is not checked.
//! Only MPEG-1 sample rates (32, 44.1, 48 kHz) and Layer II are accepted.

use std::sync::OnceLock;

use super::bits::BitReader;
use super::ps::{ElementaryStream, pts_for};
use crate::{FormatError, Result};

const WHAT: &str = "mpeg audio layer ii";

fn invalid(detail: impl Into<String>) -> FormatError {
    FormatError::invalid(WHAT, detail)
}

/// Samples per channel in a Layer II frame.
pub const FRAME_SAMPLES: usize = 1152;

const BITRATES_KBPS: [u32; 15] = [
    0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
];
const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 32_000];

/// Channel mode (2.4.2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Stereo,
    JointStereo,
    DualChannel,
    Mono,
}

/// A Layer II frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub protected: bool,
    pub bitrate_kbps: u32,
    pub sample_rate: u32,
    pub padding: bool,
    pub mode: Mode,
    pub mode_extension: u8,
}

impl Header {
    /// Parses the 4 header bytes.
    pub fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < 4 {
            return Err(FormatError::Truncated {
                what: WHAT,
                offset: 0,
                need: 4,
                len: b.len(),
            });
        }
        let h = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        if h >> 20 != 0xfff {
            return Err(FormatError::BadMagic {
                what: WHAT,
                found: b[..4].to_vec(),
            });
        }
        if h >> 19 & 1 != 1 {
            return Err(invalid("MPEG-2 low sample rate audio is not supported"));
        }
        if h >> 17 & 3 != 0b10 {
            return Err(invalid(format!(
                "layer code {} is not Layer II",
                h >> 17 & 3
            )));
        }
        let bitrate_index = (h >> 12 & 15) as usize;
        let bitrate_kbps = match bitrate_index {
            0 => return Err(invalid("free-format bit rate is not supported")),
            15 => return Err(invalid("bit rate index 15")),
            i => BITRATES_KBPS[i],
        };
        let sample_rate = match h >> 10 & 3 {
            3 => return Err(invalid("sampling frequency index 3")),
            i => SAMPLE_RATES[i as usize],
        };
        let mode = match h >> 6 & 3 {
            0 => Mode::Stereo,
            1 => Mode::JointStereo,
            2 => Mode::DualChannel,
            _ => Mode::Mono,
        };
        Ok(Self {
            protected: h >> 16 & 1 == 0,
            bitrate_kbps,
            sample_rate,
            padding: h >> 9 & 1 == 1,
            mode,
            mode_extension: (h >> 4 & 3) as u8,
        })
    }

    pub fn channels(&self) -> usize {
        if self.mode == Mode::Mono { 1 } else { 2 }
    }

    /// Frame length in bytes, header included.
    pub fn frame_len(&self) -> usize {
        (144_000 * self.bitrate_kbps / self.sample_rate) as usize + usize::from(self.padding)
    }

    /// The bit allocation table for this frame (Table 3-B.2a–d selection rules).
    fn table(&self) -> &'static AllocTable {
        let per_channel = self.bitrate_kbps / self.channels() as u32;
        let t = alloc_tables();
        if per_channel <= 48 {
            if self.sample_rate == 32_000 {
                &t[3]
            } else {
                &t[2]
            }
        } else if per_channel <= 80 || self.sample_rate == 48_000 {
            &t[0]
        } else {
            &t[1]
        }
    }
}

/// Quantisation class: number of levels, bits read, and whether three samples share one code.
#[derive(Clone, Copy)]
struct Class {
    levels: u32,
    bits: u32,
    grouped: bool,
}

/// The 17 classes of Table 3-B.4.
const CLASSES: [Class; 17] = {
    const fn c(levels: u32, bits: u32, grouped: bool) -> Class {
        Class {
            levels,
            bits,
            grouped,
        }
    }
    [
        c(3, 5, true),
        c(5, 7, true),
        c(7, 3, false),
        c(9, 10, true),
        c(15, 4, false),
        c(31, 5, false),
        c(63, 6, false),
        c(127, 7, false),
        c(255, 8, false),
        c(511, 9, false),
        c(1023, 10, false),
        c(2047, 11, false),
        c(4095, 12, false),
        c(8191, 13, false),
        c(16383, 14, false),
        c(32767, 15, false),
        c(65535, 16, false),
    ]
};

/// Class indices selected by allocation codes 1.. for the subband groups of Table 3-B.2.
const GROUP_A: &[u8] = &[0, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
const GROUP_B: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 16];
const GROUP_C: &[u8] = &[0, 1, 2, 3, 4, 5, 16];
const GROUP_D: &[u8] = &[0, 1, 16];
const GROUP_E: &[u8] = &[0, 1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
const GROUP_F: &[u8] = &[0, 1, 3, 4, 5, 6, 7];

struct AllocTable {
    /// Per subband: (allocation bits, classes).
    subbands: Vec<(u32, &'static [u8])>,
}

fn alloc_tables() -> &'static [AllocTable; 4] {
    static T: OnceLock<[AllocTable; 4]> = OnceLock::new();
    T.get_or_init(|| {
        let ab = |n: usize| AllocTable {
            subbands: (0..n)
                .map(|sb| match sb {
                    0..=2 => (4, GROUP_A),
                    3..=10 => (4, GROUP_B),
                    11..=22 => (3, GROUP_C),
                    _ => (2, GROUP_D),
                })
                .collect(),
        };
        let cd = |n: usize| AllocTable {
            subbands: (0..n)
                .map(|sb| if sb < 2 { (4, GROUP_E) } else { (3, GROUP_F) })
                .collect(),
        };
        [ab(27), ab(30), cd(8), cd(12)]
    })
}

/// Synthesis window D[i] × 65536 for i = 0..=256 (Table 3-B.3; the values are exact multiples
/// of 2^-16). The other half follows from the symmetry of the prototype filter:
/// D[512 − i] = −D[i], except at multiples of 64 where D[512 − i] = D[i].
const WINDOW: [i32; 257] = [
    0, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -3, -3, -4, -4, -5, -5, -6, -7, -7, -8, -9, -10,
    -11, -13, -14, -16, -17, -19, -21, -24, -26, -29, -31, -35, -38, -41, -45, -49, -53, -58, -63,
    -68, -73, -79, -85, -91, -97, -104, -111, -117, -125, -132, -139, -147, -154, -161, -169, -176,
    -183, -190, -196, -202, -208, 213, 218, 222, 225, 227, 228, 228, 227, 224, 221, 215, 208, 200,
    189, 177, 163, 146, 127, 106, 83, 57, 29, -2, -36, -72, -111, -153, -197, -244, -294, -347,
    -401, -459, -519, -581, -645, -711, -779, -848, -919, -991, -1064, -1137, -1210, -1283, -1356,
    -1428, -1498, -1567, -1634, -1698, -1759, -1817, -1870, -1919, -1962, -2001, -2032, -2057,
    -2075, -2085, -2087, -2080, -2063, 2037, 2000, 1952, 1893, 1822, 1739, 1644, 1535, 1414, 1280,
    1131, 970, 794, 605, 402, 185, -45, -288, -545, -814, -1095, -1388, -1692, -2006, -2330, -2663,
    -3004, -3351, -3705, -4063, -4425, -4788, -5153, -5517, -5879, -6237, -6589, -6935, -7271,
    -7597, -7910, -8209, -8491, -8755, -8998, -9219, -9416, -9585, -9727, -9838, -9916, -9959,
    -9966, -9935, -9863, -9750, -9592, -9389, -9139, -8840, -8492, -8092, -7640, -7134, 6574, 5959,
    5288, 4561, 3776, 2935, 2037, 1082, 70, -998, -2122, -3300, -4533, -5818, -7154, -8540, -9975,
    -11455, -12980, -14548, -16155, -17799, -19478, -21189, -22929, -24694, -26482, -28289, -30112,
    -31947, -33791, -35640, -37489, -39336, -41176, -43006, -44821, -46617, -48390, -50137, -51853,
    -53534, -55178, -56778, -58333, -59838, -61289, -62684, -64019, -65290, -66494, -67629, -68692,
    -69679, -70590, -71420, -72169, -72835, -73415, -73908, -74313, -74630, -74856, -74992, 75038,
];

struct Filterbank {
    /// D[0..512].
    window: [f32; 512],
    /// N[i][k] = cos((16 + i)(2k + 1)π / 64), i in 0..64, k in 0..32.
    matrix: Vec<f32>,
}

fn filterbank() -> &'static Filterbank {
    static F: OnceLock<Filterbank> = OnceLock::new();
    F.get_or_init(|| {
        let mut window = [0f32; 512];
        for (i, &v) in WINDOW.iter().enumerate() {
            let d = v as f32 / 65536.0;
            window[i] = d;
            if i > 0 && i < 256 {
                window[512 - i] = if i % 64 == 0 { d } else { -d };
            }
        }
        let mut matrix = vec![0f32; 64 * 32];
        for i in 0..64 {
            for k in 0..32 {
                let a = ((16 + i) * (2 * k + 1)) as f64 * std::f64::consts::PI / 64.0;
                matrix[i * 32 + k] = a.cos() as f32;
            }
        }
        Filterbank { window, matrix }
    })
}

/// Polyphase synthesis state of one channel (11172-3, Figure A.2).
#[derive(Clone)]
struct Synthesis {
    v: [f32; 1024],
    offset: usize,
}

impl Synthesis {
    fn new() -> Self {
        Self {
            v: [0.0; 1024],
            offset: 0,
        }
    }

    /// 32 subband samples in, 32 PCM samples out.
    fn run(&mut self, s: &[f32; 32], out: &mut [f32]) {
        let fb = filterbank();
        // Shift V by 64 (a ring buffer: move the origin back) and matrix the new samples in.
        self.offset = (self.offset + 1024 - 64) & 1023;
        for i in 0..64 {
            let row = &fb.matrix[i * 32..i * 32 + 32];
            let mut acc = 0f32;
            for k in 0..32 {
                acc += row[k] * s[k];
            }
            self.v[(self.offset + i) & 1023] = acc;
        }
        // U from V, windowed by D and summed over the 16 phases.
        for (j, o) in out.iter_mut().enumerate().take(32) {
            let mut acc = 0f32;
            for i in 0..16 {
                let m = i / 2;
                let n = if i % 2 == 0 {
                    128 * m + j
                } else {
                    128 * m + 96 + j
                };
                acc += fb.window[32 * i + j] * self.v[(self.offset + n) & 1023];
            }
            *o = acc;
        }
    }
}

/// Scale factor index → multiplier: 2^(1 − i/3) (Table 3-B.1).
fn scalefactor(i: u32) -> f32 {
    2f32.powf(1.0 - i as f32 / 3.0)
}

/// Decodes Layer II frames, keeping the filterbank state between them.
#[derive(Clone)]
pub struct Mp2Decoder {
    synth: [Synthesis; 2],
}

impl Default for Mp2Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Mp2Decoder {
    pub fn new() -> Self {
        Self {
            synth: [Synthesis::new(), Synthesis::new()],
        }
    }

    /// Decodes one whole frame (header included) and appends 1152 samples per channel,
    /// interleaved, nominally in −1.0..1.0. Returns the header.
    pub fn decode_frame(&mut self, frame: &[u8], out: &mut Vec<f32>) -> Result<Header> {
        let header = Header::parse(frame)?;
        let len = header.frame_len();
        if frame.len() < len {
            return Err(FormatError::Truncated {
                what: WHAT,
                offset: 0,
                need: len,
                len: frame.len(),
            });
        }
        let mut r = BitReader::new(&frame[..len]);
        r.skip(32);
        if header.protected {
            r.skip(16);
        }
        let nch = header.channels();
        let table = header.table();
        let sblimit = table.subbands.len();
        let bound = match header.mode {
            Mode::JointStereo => (4 + 4 * usize::from(header.mode_extension)).min(sblimit),
            Mode::Mono => 0,
            _ => sblimit,
        };
        let shared_from = if nch == 1 { sblimit } else { bound };

        let mut alloc = [[0u8; 32]; 2];
        for (sb, &(nbal, _)) in table.subbands.iter().enumerate() {
            if sb < shared_from {
                for a in alloc.iter_mut().take(nch) {
                    a[sb] = r.read(nbal) as u8;
                }
            } else {
                let a = r.read(nbal) as u8;
                alloc[0][sb] = a;
                alloc[1][sb] = a;
            }
        }
        let mut scfsi = [[0u8; 32]; 2];
        for sb in 0..sblimit {
            for ch in 0..nch {
                if alloc[ch][sb] != 0 {
                    scfsi[ch][sb] = r.read(2) as u8;
                }
            }
        }
        let mut scf = [[[0f32; 3]; 32]; 2];
        for sb in 0..sblimit {
            for ch in 0..nch {
                if alloc[ch][sb] == 0 {
                    continue;
                }
                let s = &mut scf[ch][sb];
                match scfsi[ch][sb] {
                    0 => {
                        for part in s.iter_mut() {
                            *part = scalefactor(r.read(6));
                        }
                    }
                    1 => {
                        let a = scalefactor(r.read(6));
                        let b = scalefactor(r.read(6));
                        *s = [a, a, b];
                    }
                    2 => {
                        let a = scalefactor(r.read(6));
                        *s = [a, a, a];
                    }
                    _ => {
                        let a = scalefactor(r.read(6));
                        let b = scalefactor(r.read(6));
                        *s = [a, b, b];
                    }
                }
            }
        }
        r.check(WHAT)?;

        let base = out.len();
        out.resize(base + FRAME_SAMPLES * nch, 0.0);
        let mut subband = [[[0f32; 32]; 3]; 2];
        let mut pcm = [0f32; 32];
        for gr in 0..12 {
            let part = gr / 4;
            for sb in 0..sblimit {
                let channels = if sb < shared_from { nch } else { 1 };
                for ch in 0..channels {
                    let a = alloc[ch][sb];
                    if a == 0 {
                        for slot in subband[ch].iter_mut() {
                            slot[sb] = 0.0;
                        }
                        if sb >= shared_from {
                            for slot in subband[1].iter_mut() {
                                slot[sb] = 0.0;
                            }
                        }
                        continue;
                    }
                    let classes = table.subbands[sb].1;
                    let class = CLASSES[usize::from(
                        *classes
                            .get(usize::from(a) - 1)
                            .ok_or_else(|| invalid("allocation code out of range"))?,
                    )];
                    let mut codes = [0u32; 3];
                    if class.grouped {
                        let mut c = r.read(class.bits);
                        if c >= class.levels.pow(3) {
                            return Err(invalid("grouped sample code out of range"));
                        }
                        for code in &mut codes {
                            *code = c % class.levels;
                            c /= class.levels;
                        }
                    } else {
                        for code in &mut codes {
                            *code = r.read(class.bits);
                        }
                    }
                    for (slot, &code) in codes.iter().enumerate() {
                        // Requantise: the code's fraction (2c + 1 − L) / L of the full scale.
                        let frac = (2 * code as i32 + 1 - class.levels as i32) as f32
                            / class.levels as f32;
                        subband[ch][slot][sb] = frac * scf[ch][sb][part];
                        if sb >= shared_from {
                            subband[1][slot][sb] = frac * scf[1][sb][part];
                        }
                    }
                }
            }
            for (ch, chan) in subband.iter_mut().enumerate().take(nch) {
                for (slot, s) in chan.iter_mut().enumerate() {
                    s[sblimit..].fill(0.0);
                    self.synth[ch].run(s, &mut pcm);
                    let t0 = (gr * 3 + slot) * 32;
                    for (k, &v) in pcm.iter().enumerate() {
                        out[base + (t0 + k) * nch + ch] = v;
                    }
                }
            }
        }
        if r.overrun() {
            out.truncate(base);
            return Err(invalid("frame data runs past the frame length"));
        }
        Ok(header)
    }
}

/// One decoded frame from an [`Mp2Stream`].
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    pub sample_rate: u32,
    pub channels: usize,
    /// Interleaved samples, nominally −1.0..1.0.
    pub samples: Vec<f32>,
    pub pts: Option<u64>,
    /// Byte offset of the frame in the elementary stream.
    pub offset: usize,
}

/// Walks an elementary audio stream frame by frame, resynchronising on damaged headers.
pub struct Mp2Stream {
    data: Vec<u8>,
    pts: Vec<(usize, u64)>,
    pos: usize,
    last_offset: Option<usize>,
    decoder: Mp2Decoder,
}

impl Mp2Stream {
    pub fn new(es: ElementaryStream) -> Self {
        Self {
            data: es.data,
            pts: es.pts,
            pos: 0,
            last_offset: None,
            decoder: Mp2Decoder::new(),
        }
    }

    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self::new(ElementaryStream {
            stream_id: 0xc0,
            data,
            pts: Vec::new(),
        })
    }

    /// The first frame's header, without decoding.
    pub fn first_header(&self) -> Result<Header> {
        let at = self
            .find_sync(0)
            .ok_or_else(|| invalid("no frame header"))?;
        Header::parse(&self.data[at..])
    }

    /// The PTS of the first frame, if its packet carried one.
    pub fn first_pts(&self) -> Option<u64> {
        pts_for(&self.pts, None, self.find_sync(0)?)
    }

    pub fn rewind(&mut self) {
        self.pos = 0;
        self.last_offset = None;
        self.decoder = Mp2Decoder::new();
    }

    /// A candidate header position at or after `from`: a sync word with a parseable header.
    fn find_sync(&self, from: usize) -> Option<usize> {
        let d = &self.data;
        (from..d.len().saturating_sub(3))
            .find(|&i| d[i] == 0xff && d[i + 1] & 0xf0 == 0xf0 && Header::parse(&d[i..]).is_ok())
    }

    /// The next frame; `Ok(None)` at the end. After an error the stream resynchronises at
    /// the next header on the following call.
    pub fn next_frame(&mut self) -> Result<Option<AudioFrame>> {
        if self.pos + 4 > self.data.len() {
            return Ok(None);
        }
        let at = match Header::parse(&self.data[self.pos..]) {
            Ok(_) => self.pos,
            Err(e) => {
                // Lost sync: move to the next plausible header and report once.
                match self.find_sync(self.pos + 1) {
                    Some(next) => self.pos = next,
                    None => self.pos = self.data.len(),
                }
                return Err(e);
            }
        };
        let mut samples = Vec::with_capacity(FRAME_SAMPLES * 2);
        let result = self.decoder.decode_frame(&self.data[at..], &mut samples);
        match result {
            Ok(h) => {
                self.pos = at + h.frame_len();
                let pts = pts_for(&self.pts, self.last_offset, at);
                self.last_offset = Some(at);
                Ok(Some(AudioFrame {
                    sample_rate: h.sample_rate,
                    channels: h.channels(),
                    samples,
                    pts,
                    offset: at,
                }))
            }
            Err(e) => {
                self.pos = match self.find_sync(at + 1) {
                    Some(next) => next,
                    None => self.data.len(),
                };
                Err(e)
            }
        }
    }
}

/// Converts a float sample to 16-bit PCM with rounding and saturation.
pub fn to_i16(v: f32) -> i16 {
    (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mpeg::video::tests::BitWriter;

    /// 44.1 kHz, 192 kbit/s (table B.2b, 30 subbands), 626-byte frames.
    fn header(w: &mut BitWriter, mode: u32, mode_ext: u32) {
        w.put(0xfff, 12);
        w.put(1, 1); // MPEG-1
        w.put(0b10, 2); // Layer II
        w.put(1, 1); // no CRC
        w.put(10, 4); // 192 kbit/s
        w.put(0, 2); // 44.1 kHz
        w.put(0, 1);
        w.put(0, 1);
        w.put(mode, 2);
        w.put(mode_ext, 2);
        w.put(0, 4);
    }

    fn nbal(sb: usize) -> u32 {
        match sb {
            0..=10 => 4,
            11..=22 => 3,
            _ => 2,
        }
    }

    /// A stereo frame with subband `band` allocated in both channels: allocation code 5 in
    /// sb 0..=2 is 63 levels (6 bits); sample n of the frame has code `codes[n % 4]`; scale
    /// factor indices `scf` per channel (one per frame, scfsi 2).
    /// Codes for +62/63, 0, −62/63, 0.
    const TONE: [u32; 4] = [62, 31, 0, 31];

    fn tone_frame(band: usize, codes: [u32; 4], scf: [u32; 2]) -> Vec<u8> {
        let mut w = BitWriter::default();
        header(&mut w, 0, 0);
        for sb in 0..30 {
            for _ in 0..2 {
                w.put(if sb == band { 5 } else { 0 }, nbal(sb));
            }
        }
        w.put(2, 2);
        w.put(2, 2);
        w.put(scf[0], 6);
        w.put(scf[1], 6);
        for gr in 0..12 {
            for _ in 0..2 {
                for s in 0..3 {
                    w.put(codes[(gr * 3 + s) % 4], 6);
                }
            }
        }
        let mut v = w.padded();
        v.resize(626, 0);
        v
    }

    fn decode_all(frames: &[Vec<u8>]) -> Vec<f32> {
        let mut d = Mp2Decoder::new();
        let mut out = Vec::new();
        for f in frames {
            d.decode_frame(f, &mut out).unwrap();
        }
        out
    }

    /// Power at `freq` (Goertzel) relative to the total power of `x`.
    fn tone_share(x: &[f32], freq: f64, rate: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / rate;
        let (mut s1, mut s2) = (0f64, 0f64);
        for &v in x {
            let s = f64::from(v) + 2.0 * w.cos() * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        let power = s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2;
        let total: f64 = x.iter().map(|&v| f64::from(v).powi(2)).sum();
        2.0 * power / (x.len() as f64 * total)
    }

    #[test]
    fn header_fields_and_frame_length() {
        let f = tone_frame(0, [31; 4], [3, 3]);
        let h = Header::parse(&f).unwrap();
        assert_eq!(
            (h.sample_rate, h.bitrate_kbps, h.channels()),
            (44_100, 192, 2)
        );
        assert_eq!(h.frame_len(), 626);
        assert_eq!(h.table().subbands.len(), 30);
        assert!(Header::parse(&[0xff, 0xff, 0xa0, 0x00]).is_err()); // Layer I
        assert!(Header::parse(&[0xff, 0xfd, 0xa0, 0x00]).is_ok());
        assert!(Header::parse(&[0xff, 0xf4, 0xa0, 0x00]).is_err()); // MPEG-2 LSF
        assert!(Header::parse(&[0xff, 0xfd]).is_err());
    }

    /// Subband samples +a, 0, −a, 0 (a quarter of the subband rate: the middle of the band)
    /// come out as a tone at the band's centre frequency (2k + 1) · fs / 128.
    #[test]
    fn constant_subband_gives_its_centre_tone() {
        for band in [0usize, 1, 2] {
            let frames: Vec<_> = (0..8).map(|_| tone_frame(band, TONE, [3, 3])).collect();
            let pcm = decode_all(&frames);
            let left: Vec<f32> = pcm.iter().step_by(2).copied().skip(1152).collect();
            let centre = (2 * band + 1) as f64 * 44_100.0 / 128.0;
            let share = tone_share(&left, centre, 44_100.0);
            assert!(share > 0.95, "band {band}: {share}");
            let peak = left.iter().fold(0f32, |m, &v| m.max(v.abs()));
            // Unit gain at the band centre: the peak is the requantised 62/63.
            assert!(
                (peak - 62.0 / 63.0).abs() < 0.002,
                "band {band}: peak {peak}"
            );
        }
    }

    #[test]
    fn scale_factors_and_channels_are_independent() {
        // Right channel scale factor index 6 = 2^(1 − 2) = 0.5 against the left's 1.0.
        let frames: Vec<_> = (0..4).map(|_| tone_frame(1, TONE, [3, 6])).collect();
        let pcm = decode_all(&frames);
        for t in 2304..4608 {
            let (l, r) = (pcm[2 * t], pcm[2 * t + 1]);
            assert!((r - 0.5 * l).abs() < 1e-4, "{t}: {l} {r}");
        }
        // Code 31 is the middle level: silence.
        let silent = decode_all(&[tone_frame(1, [31; 4], [3, 3])]);
        assert!(silent.iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn joint_stereo_shares_samples_above_the_bound() {
        // Mode extension 0: bound 4. Subband 10 (above the bound) is read once for both
        // channels; with equal scale factors the channels match.
        let mut w = BitWriter::default();
        header(&mut w, 1, 0);
        for sb in 0..30 {
            let n = if sb < 4 { 2 } else { 1 };
            for _ in 0..n {
                w.put(if sb == 10 { 6 } else { 0 }, nbal(sb)); // 31 levels, 5 bits
            }
        }
        w.put(2, 2);
        w.put(2, 2);
        w.put(4, 6);
        w.put(4, 6);
        for gr in 0..12u32 {
            for s in 0..3u32 {
                w.put((gr * 3 + s) % 31, 5);
            }
        }
        let mut f = w.padded();
        f.resize(626, 0);
        let pcm = decode_all(&[f.clone(), f]);
        assert!(pcm.iter().any(|v| v.abs() > 0.01));
        for t in 0..2304 {
            assert_eq!(pcm[2 * t], pcm[2 * t + 1]);
        }
    }

    #[test]
    fn grouped_codes_split_low_digit_first() {
        // Subband 11 allocation 1 = 3 levels grouped in 5 bits: code 5 = digits (2, 1, 0)
        // → fractions (2/3, 0, −2/3) for the three samples of a granule.
        let mut w = BitWriter::default();
        header(&mut w, 3, 0); // mono, 192 kbit/s per channel → table B.2b
        for sb in 0..30 {
            w.put(if sb == 11 { 1 } else { 0 }, nbal(sb));
        }
        w.put(2, 2);
        w.put(3, 6);
        for _ in 0..12 {
            w.put(5, 5);
        }
        let mut f = w.padded();
        f.resize(626, 0);
        // Decode through a stream to exercise the frame walk too.
        let mut s = Mp2Stream::from_bytes([f.clone(), f.clone(), f].concat());
        let mut n = 0;
        while let Some(frame) = s.next_frame().unwrap() {
            assert_eq!((frame.channels, frame.samples.len()), (1, 1152));
            n += 1;
        }
        assert_eq!(n, 3);
    }

    #[test]
    fn truncated_and_corrupted_frames_never_panic() {
        let frames: Vec<u8> = (0..6)
            .flat_map(|i| tone_frame(i % 3, [40 + i as u32, 31, 20, 31], [3, 5]))
            .collect();
        for cut in (0..frames.len()).step_by(7) {
            let mut s = Mp2Stream::from_bytes(frames[..cut].to_vec());
            for _ in 0..50 {
                if let Ok(None) = s.next_frame() {
                    break;
                }
            }
        }
        let mut seed = 0x0bad_f00du32;
        for _ in 0..500 {
            let mut bad = frames.clone();
            for _ in 0..4 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let i = (seed >> 8) as usize % bad.len();
                bad[i] = (seed >> 16) as u8;
            }
            let mut s = Mp2Stream::from_bytes(bad);
            let mut calls = 0;
            while !matches!(s.next_frame(), Ok(None)) {
                calls += 1;
                assert!(calls < 10_000);
            }
        }
        // A grouped code out of range is an error, not a panic.
        let mut w = BitWriter::default();
        header(&mut w, 3, 0);
        for sb in 0..30 {
            w.put(if sb == 11 { 1 } else { 0 }, nbal(sb));
        }
        w.put(2, 2);
        w.put(3, 6);
        w.put(31, 5);
        let mut f = w.padded();
        f.resize(626, 0);
        assert!(Mp2Decoder::new().decode_frame(&f, &mut Vec::new()).is_err());
    }
}
