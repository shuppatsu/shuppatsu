//! MPEG-1 video decoder (ISO/IEC 11172-2).
//!
//! Decodes an elementary video stream into 4:2:0 frames in display order: sequence, group of
//! pictures, picture, slice, macroblock and block layers; I, P, B and D pictures; default and
//! downloaded quantiser matrices; half-pel forward, backward and bidirectional motion
//! compensation; skipped macroblocks. Malformed input gives errors, never panics: a picture
//! with a damaged slice is still completed (the damaged slices conceal from the forward
//! reference, or mid-grey in an intra picture) and kept as a reference, the error is returned
//! once, and the next call carries on with the following picture.

use std::collections::VecDeque;

use super::bits::BitReader;
use super::idct::idct;
use super::ps::{ElementaryStream, pts_for};
use super::vtables::*;
use crate::{FormatError, Result};

const PICTURE: u8 = 0x00;
const SLICE_LAST: u8 = 0xaf;
const USER_DATA: u8 = 0xb2;
const SEQUENCE_HEADER: u8 = 0xb3;
const SEQUENCE_ERROR: u8 = 0xb4;
const EXTENSION: u8 = 0xb5;
const SEQUENCE_END: u8 = 0xb7;
const GOP: u8 = 0xb8;

const WHAT: &str = "mpeg-1 video";

fn invalid(detail: impl Into<String>) -> FormatError {
    FormatError::invalid(WHAT, detail)
}

/// Picture coding type (11172-2, 2.4.3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureType {
    I,
    P,
    B,
    D,
}

impl PictureType {
    fn index(self) -> usize {
        match self {
            Self::I => 0,
            Self::P => 1,
            Self::B => 2,
            Self::D => 3,
        }
    }
}

/// Sequence header fields (11172-2, 2.4.2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceHeader {
    pub width: u16,
    pub height: u16,
    /// pel_aspect_ratio code 1..=14.
    pub aspect_code: u8,
    /// picture_rate code 1..=8.
    pub rate_code: u8,
    /// In units of 400 bit/s.
    pub bit_rate: u32,
    pub vbv_buffer_size: u16,
    pub constrained: bool,
    /// Quantiser matrices in natural order.
    pub intra_matrix: [u8; 64],
    pub non_intra_matrix: [u8; 64],
}

impl SequenceHeader {
    /// Frames per second as a fraction.
    pub fn frame_rate(&self) -> (u32, u32) {
        match self.rate_code {
            1 => (24_000, 1001),
            2 => (24, 1),
            3 => (25, 1),
            4 => (30_000, 1001),
            5 => (30, 1),
            6 => (50, 1),
            7 => (60_000, 1001),
            8 => (60, 1),
            _ => (0, 1),
        }
    }

    /// Sample aspect ratio (pixel width / height). MPEG-1 codes the pel aspect ratio as
    /// height / width (Table 2-D.4.1 / 2.4.3.2), so this is its inverse.
    pub fn sample_aspect(&self) -> f64 {
        const HEIGHT_OVER_WIDTH: [f64; 15] = [
            1.0, 1.0, 0.6735, 0.7031, 0.7615, 0.8055, 0.8437, 0.8935, 0.9157, 0.9815, 1.0255,
            1.0695, 1.0950, 1.1575, 1.2015,
        ];
        1.0 / HEIGHT_OVER_WIDTH
            .get(usize::from(self.aspect_code))
            .copied()
            .unwrap_or(1.0)
    }

    /// Display aspect ratio of the whole picture.
    pub fn display_aspect(&self) -> f64 {
        f64::from(self.width) * self.sample_aspect() / f64::from(self.height.max(1))
    }

    fn parse(r: &mut BitReader<'_>) -> Result<Self> {
        let width = r.read(12) as u16;
        let height = r.read(12) as u16;
        let aspect_code = r.read(4) as u8;
        let rate_code = r.read(4) as u8;
        let bit_rate = r.read(18);
        r.skip(1); // marker
        let vbv_buffer_size = r.read(10) as u16;
        let constrained = r.bit();
        let mut intra_matrix = DEFAULT_INTRA;
        let mut non_intra_matrix = DEFAULT_NON_INTRA;
        if r.bit() {
            for &z in &ZIGZAG {
                intra_matrix[usize::from(z)] = r.read(8) as u8;
            }
        }
        if r.bit() {
            for &z in &ZIGZAG {
                non_intra_matrix[usize::from(z)] = r.read(8) as u8;
            }
        }
        r.check("mpeg-1 sequence header")?;
        if width == 0 || height == 0 || width > 4095 || height > 4095 {
            return Err(invalid(format!("picture size {width}×{height}")));
        }
        if !(1..=8).contains(&rate_code) {
            return Err(invalid(format!("picture_rate code {rate_code}")));
        }
        if intra_matrix.contains(&0) || non_intra_matrix.contains(&0) {
            return Err(invalid("zero in a quantiser matrix"));
        }
        Ok(Self {
            width,
            height,
            aspect_code,
            rate_code,
            bit_rate,
            vbv_buffer_size,
            constrained,
            intra_matrix,
            non_intra_matrix,
        })
    }
}

/// One decoded picture: 8-bit Y, Cb and Cr planes (4:2:0) of the coded size, whose top-left
/// `width × height` (luma) is the picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    /// Luma row stride (the coded width: a multiple of 16).
    pub y_stride: usize,
    /// Chroma row stride (`y_stride / 2`).
    pub c_stride: usize,
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
    pub kind: PictureType,
    pub temporal_reference: u16,
    /// Presentation time stamp (90 kHz) from the program stream, when the picture had one.
    pub pts: Option<u64>,
    /// Pictures decoded before this one in decode order (0-based).
    pub decode_index: u64,
}

impl Frame {
    fn new(width: usize, height: usize) -> Self {
        let y_stride = width.div_ceil(16) * 16;
        let rows = height.div_ceil(16) * 16;
        let c_stride = y_stride / 2;
        Self {
            width,
            height,
            y_stride,
            c_stride,
            y: vec![16; y_stride * rows],
            cb: vec![128; c_stride * rows / 2],
            cr: vec![128; c_stride * rows / 2],
            kind: PictureType::I,
            temporal_reference: 0,
            pts: None,
            decode_index: 0,
        }
    }

    /// The visible luma plane, tightly packed (`width × height`).
    pub fn y_plane(&self) -> Vec<u8> {
        crop(&self.y, self.y_stride, self.width, self.height)
    }

    /// The visible chroma planes, tightly packed (`ceil(width/2) × ceil(height/2)`).
    pub fn chroma_planes(&self) -> (Vec<u8>, Vec<u8>) {
        let (w, h) = (self.width.div_ceil(2), self.height.div_ceil(2));
        (
            crop(&self.cb, self.c_stride, w, h),
            crop(&self.cr, self.c_stride, w, h),
        )
    }

    /// Converts to RGBA8 (`width × height × 4`) with the ITU-R BT.601 matrix on video-range
    /// levels (Y 16..235, C 16..240), chroma upsampled bilinearly from the MPEG-1 sample
    /// positions (centred between the luma samples, 11172-2 2.4.1).
    pub fn to_rgba(&self, out: &mut Vec<u8>) {
        let (w, h) = (self.width, self.height);
        out.clear();
        out.resize(w * h * 4, 255);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        // Chroma index pairs and weights (out of 4) for a luma coordinate.
        let taps = |i: usize, n: usize| -> (usize, usize, i32) {
            let c = i / 2;
            if i.is_multiple_of(2) {
                (c, c.saturating_sub(1), 3)
            } else {
                (c, (c + 1).min(n - 1), 3)
            }
        };
        for yy in 0..h {
            let (r0, r1, wr) = taps(yy, ch);
            for xx in 0..w {
                let (c0, c1, wc) = taps(xx, cw);
                let sample = |p: &[u8]| -> i32 {
                    let at = |r: usize, c: usize| i32::from(p[r * self.c_stride + c]);
                    let top = wc * at(r0, c0) + (4 - wc) * at(r0, c1);
                    let bottom = wc * at(r1, c0) + (4 - wc) * at(r1, c1);
                    wr * top + (4 - wr) * bottom
                };
                let cb = sample(&self.cb) as f32 / 16.0 - 128.0;
                let cr = sample(&self.cr) as f32 / 16.0 - 128.0;
                let y = (f32::from(self.y[yy * self.y_stride + xx]) - 16.0) * (255.0 / 219.0);
                let k = 255.0 / 224.0;
                let r = y + k * 1.402 * cr;
                let g = y - k * (0.114 * 1.772 / 0.587) * cb - k * (0.299 * 1.402 / 0.587) * cr;
                let b = y + k * 1.772 * cb;
                let o = (yy * w + xx) * 4;
                out[o] = r.round().clamp(0.0, 255.0) as u8;
                out[o + 1] = g.round().clamp(0.0, 255.0) as u8;
                out[o + 2] = b.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn crop(plane: &[u8], stride: usize, w: usize, h: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(w * h);
    for row in 0..h {
        v.extend_from_slice(&plane[row * stride..row * stride + w]);
    }
    v
}

/// Picture header fields needed while decoding.
#[derive(Clone, Copy)]
struct PictureHeader {
    kind: PictureType,
    temporal_reference: u16,
    full_pel_fwd: bool,
    fwd_f_code: u8,
    full_pel_bwd: bool,
    bwd_f_code: u8,
}

impl PictureHeader {
    fn parse(r: &mut BitReader<'_>) -> Result<Self> {
        let temporal_reference = r.read(10) as u16;
        let kind = match r.read(3) {
            1 => PictureType::I,
            2 => PictureType::P,
            3 => PictureType::B,
            4 => PictureType::D,
            t => return Err(invalid(format!("picture_coding_type {t}"))),
        };
        r.skip(16); // vbv_delay
        let (mut full_pel_fwd, mut fwd_f_code, mut full_pel_bwd, mut bwd_f_code) =
            (false, 1, false, 1);
        if matches!(kind, PictureType::P | PictureType::B) {
            full_pel_fwd = r.bit();
            fwd_f_code = r.read(3) as u8;
            if fwd_f_code == 0 {
                return Err(invalid("forward_f_code 0"));
            }
        }
        if kind == PictureType::B {
            full_pel_bwd = r.bit();
            bwd_f_code = r.read(3) as u8;
            if bwd_f_code == 0 {
                return Err(invalid("backward_f_code 0"));
            }
        }
        // extra_information_picture
        let mut guard = 0;
        while r.bit() {
            r.skip(8);
            guard += 1;
            if guard > 1 << 16 || r.overrun() {
                return Err(invalid("unterminated extra_information_picture"));
            }
        }
        r.check("mpeg-1 picture header")?;
        Ok(Self {
            kind,
            temporal_reference,
            full_pel_fwd,
            fwd_f_code,
            full_pel_bwd,
            bwd_f_code,
        })
    }
}

/// Motion of one macroblock: which references, and vectors in half-pel luma units.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Motion {
    fwd: Option<(i32, i32)>,
    bwd: Option<(i32, i32)>,
}

/// Per-slice decoding state.
struct SliceState {
    qscale: i32,
    dc_pred: [i32; 3],
    /// Motion vector predictors (the last reconstructed vectors, in f_code units).
    pred_fwd: (i32, i32),
    pred_bwd: (i32, i32),
    /// The previous macroblock's motion, reused by skipped macroblocks in B pictures.
    last_motion: Motion,
    last_intra: bool,
}

impl SliceState {
    fn reset_dc(&mut self) {
        self.dc_pred = [1024; 3];
    }
}

/// Decoded pictures waiting to come out, and a pending error.
#[derive(Default)]
struct Output {
    frames: VecDeque<Frame>,
    error: Option<FormatError>,
}

/// Decodes an MPEG-1 video elementary stream into frames in display order.
pub struct VideoDecoder {
    data: Vec<u8>,
    pts: Vec<(usize, u64)>,
    pos: usize,
    seq: Option<SequenceHeader>,
    /// Older and newer reference pictures (forward and backward references of a B picture).
    past: Option<Frame>,
    future: Option<Frame>,
    future_shown: bool,
    /// B pictures are dropped until a reference after a broken link arrives.
    drop_b: bool,
    spare: Option<Frame>,
    decoded: u64,
    last_picture_offset: Option<usize>,
    out: Output,
    finished: bool,
}

impl VideoDecoder {
    /// A decoder over an elementary stream (with its PES time stamps, if any).
    pub fn new(es: ElementaryStream) -> Self {
        Self {
            data: es.data,
            pts: es.pts,
            pos: 0,
            seq: None,
            past: None,
            future: None,
            future_shown: true,
            drop_b: false,
            spare: None,
            decoded: 0,
            last_picture_offset: None,
            out: Output::default(),
            finished: false,
        }
    }

    /// A decoder over raw elementary stream bytes without time stamps.
    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self::new(ElementaryStream {
            stream_id: 0xe0,
            data,
            pts: Vec::new(),
        })
    }

    /// The current sequence header (after the first frame or [`Self::read_sequence_header`]).
    pub fn sequence(&self) -> Option<&SequenceHeader> {
        self.seq.as_ref()
    }

    /// Parses up to the first sequence header without decoding pictures.
    pub fn read_sequence_header(&mut self) -> Result<&SequenceHeader> {
        if self.seq.is_none() {
            let mut r = BitReader::at_byte(&self.data, self.pos);
            while let Some(code) = r.next_start_code() {
                if code == SEQUENCE_HEADER {
                    let seq = SequenceHeader::parse(&mut r)?;
                    self.seq = Some(seq);
                    break;
                }
            }
        }
        self.seq
            .as_ref()
            .ok_or_else(|| invalid("no sequence header"))
    }

    /// Rewinds to the start of the stream, dropping all decoder state.
    pub fn rewind(&mut self) {
        let data = std::mem::take(&mut self.data);
        let pts = std::mem::take(&mut self.pts);
        *self = Self::new(ElementaryStream {
            stream_id: 0xe0,
            data,
            pts,
        });
    }

    /// The next frame in display order; `Ok(None)` at the end of the stream. After an error the
    /// decoder has moved on, and the next call continues with the following picture.
    pub fn next_frame(&mut self) -> Result<Option<Frame>> {
        loop {
            if let Some(e) = self.out.error.take() {
                return Err(e);
            }
            if let Some(f) = self.out.frames.pop_front() {
                return Ok(Some(f));
            }
            if self.finished {
                return Ok(None);
            }
            self.step();
        }
    }

    /// Recycles a frame returned by [`Self::next_frame`] to save an allocation.
    pub fn recycle(&mut self, frame: Frame) {
        if self.spare.is_none() {
            self.spare = Some(frame);
        }
    }

    fn flush_future(&mut self) {
        if !self.future_shown
            && let Some(f) = &self.future
        {
            self.out.frames.push_back(f.clone());
            self.future_shown = true;
        }
    }

    /// Advances over one start code (and the unit it opens), queueing any output.
    fn step(&mut self) {
        let data = std::mem::take(&mut self.data);
        self.step_in(&data);
        self.data = data;
    }

    fn step_in(&mut self, data: &[u8]) {
        let mut r = BitReader::at_byte(data, self.pos);
        let Some(code) = r.next_start_code() else {
            self.flush_future();
            self.finished = true;
            return;
        };
        let unit_start = r.byte_pos() - 4;
        self.pos = r.byte_pos();
        match code {
            SEQUENCE_HEADER => match SequenceHeader::parse(&mut r) {
                Ok(seq) => {
                    let resized = self
                        .seq
                        .as_ref()
                        .is_some_and(|s| s.width != seq.width || s.height != seq.height);
                    if resized {
                        self.flush_future();
                        self.past = None;
                        self.future = None;
                    }
                    self.seq = Some(seq);
                    self.pos = r.byte_pos();
                }
                Err(e) => self.out.error = Some(e),
            },
            GOP => {
                r.skip(25); // time_code
                let _closed = r.bit();
                let broken_link = r.bit();
                if broken_link {
                    self.drop_b = true;
                }
                self.pos = r.byte_pos();
            }
            PICTURE => {
                let result = self.decode_picture(&mut r, unit_start);
                // Continue at the next start code after whatever the picture consumed.
                self.pos = r.byte_pos().max(self.pos);
                if let Err(e) = result {
                    self.out.error = Some(e);
                }
            }
            SEQUENCE_END => self.flush_future(),
            USER_DATA | EXTENSION | SEQUENCE_ERROR => {}
            // A slice outside a picture, or a reserved or system start code: skip it.
            _ => {}
        }
    }

    fn decode_picture(&mut self, r: &mut BitReader<'_>, offset: usize) -> Result<()> {
        let seq = self
            .seq
            .clone()
            .ok_or_else(|| invalid("picture before any sequence header"))?;
        let header = PictureHeader::parse(r)?;
        let pts = pts_for(&self.pts, self.last_picture_offset, offset);
        self.last_picture_offset = Some(offset);
        let decode_index = self.decoded;
        self.decoded += 1;

        let is_b = header.kind == PictureType::B;
        // A P picture needs a forward reference, a B picture both.
        let refs_ok = match header.kind {
            PictureType::P => self.future.is_some(),
            PictureType::B => self.past.is_some() && self.future.is_some() && !self.drop_b,
            _ => true,
        };
        if !refs_ok {
            // Skip its slices without output (a leading B picture of an open GOP or after a
            // broken link, or a P picture with nothing to predict from).
            skip_to_non_slice(r);
            return Ok(());
        }

        let (w, h) = (usize::from(seq.width), usize::from(seq.height));
        let mut frame = match self.spare.take() {
            Some(f) if f.width == w && f.height == h => f,
            _ => Frame::new(w, h),
        };
        frame.kind = header.kind;
        frame.temporal_reference = header.temporal_reference;
        frame.pts = pts;
        frame.decode_index = decode_index;

        // Concealment base for slices that are missing or damaged.
        let conceal_from = match header.kind {
            PictureType::B | PictureType::P => self.future.as_ref(),
            _ => None,
        };
        match conceal_from {
            Some(src) => {
                frame.y.copy_from_slice(&src.y);
                frame.cb.copy_from_slice(&src.cb);
                frame.cr.copy_from_slice(&src.cr);
            }
            None => {
                frame.y.fill(128);
                frame.cb.fill(128);
                frame.cr.fill(128);
            }
        }

        let mut first_error = None;
        {
            let (fwd_ref, bwd_ref) = match header.kind {
                PictureType::P => (self.future.as_ref(), None),
                PictureType::B => (self.past.as_ref(), self.future.as_ref()),
                _ => (None, None),
            };
            let ctx = PictureCtx {
                seq: &seq,
                header,
                fwd_ref,
                bwd_ref,
                mb_width: w.div_ceil(16),
                mb_height: h.div_ceil(16),
            };
            loop {
                // Peek at the next start code: slices belong to this picture, anything else ends
                // it and is left for `step`.
                let mut probe = r.clone();
                let Some(code) = probe.next_start_code() else {
                    *r = probe;
                    break;
                };
                if !(0x01..=SLICE_LAST).contains(&code) {
                    // Leave `r` just before the start code.
                    r.set_byte_pos(probe.byte_pos() - 4);
                    break;
                }
                *r = probe;
                if let Err(e) = ctx.decode_slice(r, code, &mut frame) {
                    first_error.get_or_insert(e);
                }
            }
        }

        if is_b {
            self.out.frames.push_back(frame);
        } else {
            self.flush_future();
            if let Some(old) = self.past.take() {
                self.spare = Some(old);
            }
            self.past = self.future.take();
            self.future = Some(frame);
            self.future_shown = false;
            // The second reference after a broken link makes B pictures decodable again.
            if self.drop_b && self.past.is_some() && header.kind != PictureType::I {
                self.drop_b = false;
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Moves `r` to just before the next start code that is not a slice.
fn skip_to_non_slice(r: &mut BitReader<'_>) {
    loop {
        let mut probe = r.clone();
        match probe.next_start_code() {
            Some(code) if (0x01..=SLICE_LAST).contains(&code) => *r = probe,
            Some(_) => {
                r.set_byte_pos(probe.byte_pos() - 4);
                return;
            }
            None => {
                *r = probe;
                return;
            }
        }
    }
}

struct PictureCtx<'a> {
    seq: &'a SequenceHeader,
    header: PictureHeader,
    fwd_ref: Option<&'a Frame>,
    bwd_ref: Option<&'a Frame>,
    mb_width: usize,
    mb_height: usize,
}

impl PictureCtx<'_> {
    fn decode_slice(&self, r: &mut BitReader<'_>, code: u8, frame: &mut Frame) -> Result<()> {
        let t = tables();
        let row = usize::from(code) - 1;
        if row >= self.mb_height {
            return Err(invalid(format!(
                "slice row {row} beyond {} rows",
                self.mb_height
            )));
        }
        let qscale = r.read(5) as i32;
        if qscale == 0 {
            return Err(invalid("quantizer_scale 0"));
        }
        let mut guard = 0;
        while r.bit() {
            r.skip(8);
            guard += 1;
            if guard > 1 << 16 || r.overrun() {
                return Err(invalid("unterminated extra_information_slice"));
            }
        }
        let mut st = SliceState {
            qscale,
            dc_pred: [1024; 3],
            pred_fwd: (0, 0),
            pred_bwd: (0, 0),
            last_motion: Motion::default(),
            last_intra: false,
        };
        let mb_count = self.mb_width * self.mb_height;
        let mut address = (row * self.mb_width) as isize - 1;
        let mut first = true;
        loop {
            // macroblock_address_increment, with stuffing and escapes.
            let mut increment = 0usize;
            loop {
                match t.mba.read(r) {
                    Some(MBA_STUFFING) => {}
                    Some(MBA_ESCAPE) => increment += 33,
                    Some(n) => {
                        increment += usize::from(n);
                        break;
                    }
                    None => return Err(invalid("bad macroblock_address_increment")),
                }
                if r.overrun() || increment > mb_count {
                    return Err(invalid("runaway macroblock address escapes"));
                }
            }
            let next = address + increment as isize;
            if next < 0 || next as usize >= mb_count {
                return Err(invalid(format!("macroblock address {next} out of range")));
            }
            if !first {
                // Skipped macroblocks between the previous and this one.
                for skipped in (address + 1) as usize..next as usize {
                    self.skipped_macroblock(skipped, &mut st, frame)?;
                }
            }
            address = next;
            first = false;
            self.macroblock(r, address as usize, &mut st, frame)?;
            r.check("mpeg-1 macroblock")?;
            // The slice ends where only zero bits remain before the next start code.
            if r.peek(23) == 0 {
                break;
            }
        }
        Ok(())
    }

    fn skipped_macroblock(
        &self,
        address: usize,
        st: &mut SliceState,
        frame: &mut Frame,
    ) -> Result<()> {
        let (mbx, mby) = (address % self.mb_width, address / self.mb_width);
        st.reset_dc();
        let motion = match self.header.kind {
            PictureType::P => {
                st.pred_fwd = (0, 0);
                Motion {
                    fwd: Some((0, 0)),
                    bwd: None,
                }
            }
            PictureType::B => {
                if st.last_intra {
                    return Err(invalid(
                        "skipped macroblock after an intra one in a B picture",
                    ));
                }
                st.last_motion
            }
            _ => return Err(invalid("skipped macroblock in an intra picture")),
        };
        let mut pred = [0u8; 384];
        self.predict(mbx, mby, motion, &mut pred)?;
        put_macroblock(frame, mbx, mby, &pred);
        Ok(())
    }

    fn macroblock(
        &self,
        r: &mut BitReader<'_>,
        address: usize,
        st: &mut SliceState,
        frame: &mut Frame,
    ) -> Result<()> {
        let t = tables();
        let (mbx, mby) = (address % self.mb_width, address / self.mb_width);
        let kind = self.header.kind;
        let flags = t.mb_type[kind.index()]
            .read(r)
            .ok_or_else(|| invalid("bad macroblock_type"))?;
        if flags & MB_QUANT != 0 {
            st.qscale = r.read(5) as i32;
            if st.qscale == 0 {
                return Err(invalid("quantizer_scale 0"));
            }
        }
        let intra = flags & MB_INTRA != 0;
        let mut motion = Motion::default();
        if flags & MB_FWD != 0 {
            decode_vector(r, self.header.fwd_f_code, &mut st.pred_fwd)?;
            motion.fwd = Some(scale(st.pred_fwd, self.header.full_pel_fwd));
        }
        if flags & MB_BWD != 0 {
            decode_vector(r, self.header.bwd_f_code, &mut st.pred_bwd)?;
            motion.bwd = Some(scale(st.pred_bwd, self.header.full_pel_bwd));
        }
        let cbp = if flags & MB_PATTERN != 0 {
            t.cbp
                .read(r)
                .ok_or_else(|| invalid("bad coded_block_pattern"))?
        } else if intra {
            63
        } else {
            0
        };

        if intra {
            st.pred_fwd = (0, 0);
            st.pred_bwd = (0, 0);
            st.last_intra = true;
        } else {
            st.reset_dc();
            st.last_intra = false;
            if kind == PictureType::P && motion.fwd.is_none() {
                // "Coded, no motion compensation": a zero forward vector.
                st.pred_fwd = (0, 0);
                motion.fwd = Some((0, 0));
            }
            st.last_motion = motion;
        }

        let mut pred = [0u8; 384];
        if !intra {
            self.predict(mbx, mby, motion, &mut pred)?;
        }
        let mut coeffs = [0i32; 64];
        let mut spatial = [0i32; 64];
        for block in 0..6 {
            if cbp & (32 >> block) == 0 {
                continue;
            }
            coeffs.fill(0);
            self.block(r, block, intra, st, &mut coeffs)?;
            idct(&coeffs, &mut spatial);
            let (off, stride) = block_layout(block);
            for y in 0..8 {
                for x in 0..8 {
                    let p = &mut pred[off + y * stride + x];
                    let v = spatial[y * 8 + x] + if intra { 0 } else { i32::from(*p) };
                    *p = v.clamp(0, 255) as u8;
                }
            }
        }
        if kind == PictureType::D && !r.bit() {
            return Err(invalid("missing end_of_macroblock in a D picture"));
        }
        put_macroblock(frame, mbx, mby, &pred);
        Ok(())
    }

    /// Reads one block's coefficients and dequantises them into `out` (natural order).
    fn block(
        &self,
        r: &mut BitReader<'_>,
        block: usize,
        intra: bool,
        st: &mut SliceState,
        out: &mut [i32; 64],
    ) -> Result<()> {
        let t = tables();
        let q = st.qscale;
        let mut index;
        let matrix;
        if intra {
            let (size, comp) = if block < 4 {
                (t.dc_luma.read(r), 0)
            } else {
                (t.dc_chroma.read(r), block - 3)
            };
            let size = u32::from(size.ok_or_else(|| invalid("bad dct_dc_size"))?);
            let diff = if size == 0 {
                0
            } else {
                let bits = r.read(size) as i32;
                if bits & (1 << (size - 1)) != 0 {
                    bits
                } else {
                    bits + 1 - (1 << size)
                }
            };
            st.dc_pred[comp] += diff * 8;
            out[0] = st.dc_pred[comp];
            if self.header.kind == PictureType::D {
                return Ok(());
            }
            index = 1;
            matrix = &self.seq.intra_matrix;
        } else {
            index = 0;
            matrix = &self.seq.non_intra_matrix;
        }
        loop {
            let (run, level);
            if !intra && index == 0 && r.peek(1) == 1 {
                // dct_coeff_first: "1s" is run 0, level ±1.
                r.skip(1);
                run = 0;
                level = if r.bit() { -1 } else { 1 };
            } else {
                let v = t.dct.read(r).ok_or_else(|| invalid("bad dct_coeff"))?;
                if v == DCT_EOB {
                    break;
                }
                if v == DCT_ESCAPE {
                    run = r.read(6) as usize;
                    let l = r.read(8) as i32;
                    level = match l {
                        0 => r.read(8) as i32,
                        128 => r.read(8) as i32 - 256,
                        _ => l as u8 as i8 as i32,
                    };
                    if level == 0 {
                        return Err(invalid("escaped level 0"));
                    }
                } else {
                    run = usize::from(v >> 8);
                    let l = i32::from(v & 0xff);
                    level = if r.bit() { -l } else { l };
                }
            }
            index += run;
            if index > 63 {
                return Err(invalid("dct coefficient index beyond 63"));
            }
            let pos = usize::from(ZIGZAG[index]);
            let w = i32::from(matrix[pos]);
            let mut v = if intra {
                (2 * level * q * w) / 16
            } else {
                ((2 * level + level.signum()) * q * w) / 16
            };
            // Oddification: even values move one step towards zero (2.4.4.2).
            if v & 1 == 0 {
                v -= v.signum();
            }
            out[pos] = v.clamp(-2048, 2047);
            index += 1;
            if r.overrun() {
                return Err(invalid("block runs past the end of the data"));
            }
        }
        Ok(())
    }

    /// Builds the motion-compensated prediction of a macroblock: 16×16 Y, then 8×8 Cb, 8×8 Cr.
    fn predict(&self, mbx: usize, mby: usize, motion: Motion, out: &mut [u8; 384]) -> Result<()> {
        let fetch = |frame: &Frame, mv: (i32, i32), dst: &mut [u8; 384]| {
            let (x, y) = ((mbx * 16) as i32, (mby * 16) as i32);
            predict_plane(
                &frame.y,
                frame.y_stride,
                frame.y.len() / frame.y_stride,
                x,
                y,
                mv,
                16,
                &mut dst[..256],
            );
            let cmv = (mv.0 / 2, mv.1 / 2);
            let rows = frame.cb.len() / frame.c_stride;
            let (cx, cy) = (x / 2, y / 2);
            predict_plane(
                &frame.cb,
                frame.c_stride,
                rows,
                cx,
                cy,
                cmv,
                8,
                &mut dst[256..320],
            );
            predict_plane(
                &frame.cr,
                frame.c_stride,
                rows,
                cx,
                cy,
                cmv,
                8,
                &mut dst[320..384],
            );
        };
        match (motion.fwd, motion.bwd) {
            (Some(f), None) => fetch(
                self.fwd_ref
                    .ok_or_else(|| invalid("no forward reference"))?,
                f,
                out,
            ),
            (None, Some(b)) => fetch(
                self.bwd_ref
                    .ok_or_else(|| invalid("no backward reference"))?,
                b,
                out,
            ),
            (Some(f), Some(b)) => {
                fetch(
                    self.fwd_ref
                        .ok_or_else(|| invalid("no forward reference"))?,
                    f,
                    out,
                );
                let mut other = [0u8; 384];
                fetch(
                    self.bwd_ref
                        .ok_or_else(|| invalid("no backward reference"))?,
                    b,
                    &mut other,
                );
                for (o, &p) in out.iter_mut().zip(other.iter()) {
                    *o = ((u16::from(*o) + u16::from(p) + 1) >> 1) as u8;
                }
            }
            (None, None) => return Err(invalid("non-intra macroblock without motion")),
        }
        Ok(())
    }
}

/// Reads one motion vector (horizontal then vertical) into the predictor (2.4.4.2, "motion
/// vectors"): the motion_code and residual give a delta, which wraps into the range of f_code.
fn decode_vector(r: &mut BitReader<'_>, f_code: u8, pred: &mut (i32, i32)) -> Result<()> {
    let t = tables();
    let r_size = u32::from(f_code - 1);
    let f = 1i32 << r_size;
    for comp in [&mut pred.0, &mut pred.1] {
        let code = i32::from(t.motion.read(r).ok_or_else(|| invalid("bad motion_code"))?);
        let delta = if f == 1 || code == 0 {
            code
        } else {
            let residual = r.read(r_size) as i32;
            ((code.abs() - 1) * f + residual + 1) * code.signum()
        };
        let mut v = *comp + delta;
        if v > 16 * f - 1 {
            v -= 32 * f;
        } else if v < -16 * f {
            v += 32 * f;
        }
        *comp = v;
    }
    Ok(())
}

/// A reconstructed vector in half-pel units.
fn scale(v: (i32, i32), full_pel: bool) -> (i32, i32) {
    if full_pel { (v.0 * 2, v.1 * 2) } else { v }
}

/// Copies a `size × size` block from `plane` at (`x`, `y`) displaced by `mv` half-pels,
/// averaging neighbours for half-pel positions. Coordinates outside the plane are clamped to
/// its edge (a conforming stream never points outside).
#[allow(clippy::too_many_arguments)]
fn predict_plane(
    plane: &[u8],
    stride: usize,
    rows: usize,
    x: i32,
    y: i32,
    mv: (i32, i32),
    size: usize,
    dst: &mut [u8],
) {
    let ix = x + (mv.0 >> 1);
    let iy = y + (mv.1 >> 1);
    let (hx, hy) = (mv.0 & 1 != 0, mv.1 & 1 != 0);
    let n = size as i32;
    let inside = ix >= 0 && iy >= 0 && ix + n < stride as i32 && iy + n < rows as i32;
    let at = |xx: i32, yy: i32| -> u16 {
        if inside {
            u16::from(plane[yy as usize * stride + xx as usize])
        } else {
            let cx = xx.clamp(0, stride as i32 - 1) as usize;
            let cy = yy.clamp(0, rows as i32 - 1) as usize;
            u16::from(plane[cy * stride + cx])
        }
    };
    for row in 0..n {
        for col in 0..n {
            let (sx, sy) = (ix + col, iy + row);
            let v = match (hx, hy) {
                (false, false) => at(sx, sy),
                (true, false) => (at(sx, sy) + at(sx + 1, sy) + 1) >> 1,
                (false, true) => (at(sx, sy) + at(sx, sy + 1) + 1) >> 1,
                (true, true) => {
                    (at(sx, sy) + at(sx + 1, sy) + at(sx, sy + 1) + at(sx + 1, sy + 1) + 2) >> 2
                }
            };
            dst[(row * n + col) as usize] = v as u8;
        }
    }
}

/// Offset and stride of block `b` (0..6) inside the 384-byte macroblock buffer.
fn block_layout(b: usize) -> (usize, usize) {
    match b {
        0 => (0, 16),
        1 => (8, 16),
        2 => (128, 16),
        3 => (136, 16),
        4 => (256, 8),
        _ => (320, 8),
    }
}

fn put_macroblock(frame: &mut Frame, mbx: usize, mby: usize, mb: &[u8; 384]) {
    let ys = frame.y_stride;
    for row in 0..16 {
        let o = (mby * 16 + row) * ys + mbx * 16;
        frame.y[o..o + 16].copy_from_slice(&mb[row * 16..row * 16 + 16]);
    }
    let cs = frame.c_stride;
    for row in 0..8 {
        let o = (mby * 8 + row) * cs + mbx * 8;
        frame.cb[o..o + 8].copy_from_slice(&mb[256 + row * 8..256 + row * 8 + 8]);
        frame.cr[o..o + 8].copy_from_slice(&mb[320 + row * 8..320 + row * 8 + 8]);
    }
}

#[cfg(test)]
pub(crate) mod tests;
