//! MPEG program stream demultiplexer (ISO/IEC 11172-1 system stream; the MPEG-2 pack and PES
//! header forms of ISO/IEC 13818-1 are accepted too).
//!
//! A program stream is a sequence of packs (`00 00 01 BA`), optional system headers
//! (`00 00 01 BB`) and PES packets (`00 00 01 BC..FF`, each with a 16-bit length), ended by
//! `00 00 01 B9`. A PES packet carries part of one elementary stream and may stamp the access unit
//! that starts in it with a 33-bit presentation time (PTS, 90 kHz).

use crate::{FormatError, Result};

/// Clock rate of PTS / DTS / SCR values.
pub const CLOCK_HZ: u64 = 90_000;

const PACK: u8 = 0xba;
const SYSTEM_HEADER: u8 = 0xbb;
const END: u8 = 0xb9;
const PADDING: u8 = 0xbe;
const PRIVATE_2: u8 = 0xbf;
const STREAM_MAP: u8 = 0xbc;
const DIRECTORY: u8 = 0xff;

/// One PES packet's payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PesPacket<'a> {
    pub stream_id: u8,
    /// Byte offset of the packet's start code in the program stream.
    pub offset: usize,
    pub pts: Option<u64>,
    pub dts: Option<u64>,
    pub payload: &'a [u8],
}

/// Iterates the PES packets of a program stream, skipping packs, system headers, padding and
/// anything between start codes, and end codes (which may separate concatenated streams).
/// Stops at the end of the data or after an error.
pub struct ProgramStream<'a> {
    data: &'a [u8],
    pos: usize,
    done: bool,
}

impl<'a> ProgramStream<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            done: false,
        }
    }

    fn need(&self, at: usize, n: usize, what: &'static str) -> Result<()> {
        if at.checked_add(n).is_some_and(|end| end <= self.data.len()) {
            Ok(())
        } else {
            Err(FormatError::Truncated {
                what,
                offset: at,
                need: n,
                len: self.data.len(),
            })
        }
    }

    fn step(&mut self) -> Result<Option<PesPacket<'a>>> {
        loop {
            let Some(start) = super::bits::find_start_code(self.data, self.pos) else {
                return Ok(None);
            };
            let code = self.data[start + 3];
            let body = start + 4;
            match code {
                // An end code mid-file (komachi.dat has three: two before its last 100 KB) does
                // not end the data; keep scanning like ffmpeg does.
                END => self.pos = body,
                PACK => {
                    self.need(body, 1, "mpeg pack header")?;
                    if self.data[body] >> 6 == 0b01 {
                        // MPEG-2: 10 bytes, then up to 7 stuffing bytes.
                        self.need(body, 10, "mpeg-2 pack header")?;
                        let stuffing = usize::from(self.data[body + 9] & 7);
                        self.pos = body + 10 + stuffing;
                    } else if self.data[body] >> 4 == 0b0010 {
                        self.need(body, 8, "mpeg-1 pack header")?;
                        self.pos = body + 8;
                    } else {
                        return Err(FormatError::invalid(
                            "mpeg pack header",
                            format!("unknown pack form {:#04x} at {start:#x}", self.data[body]),
                        ));
                    }
                }
                0xbb..=0xff => {
                    self.need(body, 2, "mpeg packet length")?;
                    let len =
                        usize::from(u16::from_be_bytes([self.data[body], self.data[body + 1]]));
                    let payload_start = body + 2;
                    self.need(payload_start, len, "mpeg packet")?;
                    let packet = &self.data[payload_start..payload_start + len];
                    self.pos = payload_start + len;
                    match code {
                        SYSTEM_HEADER | PADDING | STREAM_MAP | DIRECTORY => {}
                        PRIVATE_2 => {
                            return Ok(Some(PesPacket {
                                stream_id: code,
                                offset: start,
                                pts: None,
                                dts: None,
                                payload: packet,
                            }));
                        }
                        _ => {
                            let (pts, dts, header) = pes_header(packet, start)?;
                            return Ok(Some(PesPacket {
                                stream_id: code,
                                offset: start,
                                pts,
                                dts,
                                payload: &packet[header..],
                            }));
                        }
                    }
                }
                // Video start codes (0x00..=0xb8) do not belong at the system layer: resync.
                _ => self.pos = start + 1,
            }
        }
    }
}

impl<'a> Iterator for ProgramStream<'a> {
    type Item = Result<PesPacket<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.step() {
            Ok(Some(p)) => Some(Ok(p)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

/// Reads a 33-bit time stamp from its 5-byte field (4-bit prefix, then 3 + 15 + 15 bits, each
/// group followed by a marker bit).
fn timestamp(b: &[u8]) -> u64 {
    (u64::from(b[0] >> 1 & 7) << 30)
        | (u64::from(b[1]) << 22)
        | (u64::from(b[2] >> 1) << 15)
        | (u64::from(b[3]) << 7)
        | u64::from(b[4] >> 1)
}

/// Parses a PES header; returns PTS, DTS and the header length (the payload offset).
fn pes_header(p: &[u8], start: usize) -> Result<(Option<u64>, Option<u64>, usize)> {
    let bad =
        |detail: &str| FormatError::invalid("mpeg pes header", format!("{detail} at {start:#x}"));
    let short = |need: usize| FormatError::Truncated {
        what: "mpeg pes header",
        offset: start,
        need,
        len: p.len(),
    };
    if p.first().is_some_and(|b| b >> 6 == 0b10) {
        // MPEG-2 PES header.
        if p.len() < 3 {
            return Err(short(3));
        }
        let flags = p[1] >> 6;
        let header_len = 3 + usize::from(p[2]);
        if p.len() < header_len {
            return Err(short(header_len));
        }
        let (mut pts, mut dts) = (None, None);
        if flags & 2 != 0 {
            if header_len < 8 {
                return Err(bad("pts flag without room"));
            }
            pts = Some(timestamp(&p[3..8]));
            if flags == 3 {
                if header_len < 13 {
                    return Err(bad("dts flag without room"));
                }
                dts = Some(timestamp(&p[8..13]));
            }
        }
        return Ok((pts, dts, header_len));
    }
    // MPEG-1: up to 16 stuffing bytes, an optional STD buffer field, then the time stamps.
    let mut i = 0;
    while p.get(i) == Some(&0xff) {
        i += 1;
        if i > 16 {
            return Err(bad("more than 16 stuffing bytes"));
        }
    }
    let Some(&b) = p.get(i) else {
        return Err(short(i + 1));
    };
    if b >> 6 == 0b01 {
        i += 2;
    }
    let Some(&b) = p.get(i) else {
        return Err(short(i + 1));
    };
    match b >> 4 {
        0b0010 => {
            let ts = p.get(i..i + 5).ok_or_else(|| short(i + 5))?;
            Ok((Some(timestamp(ts)), None, i + 5))
        }
        0b0011 => {
            let ts = p.get(i..i + 10).ok_or_else(|| short(i + 10))?;
            Ok((Some(timestamp(&ts[..5])), Some(timestamp(&ts[5..])), i + 10))
        }
        _ if b == 0x0f => Ok((None, None, i + 1)),
        _ => Err(bad("bad time stamp field")),
    }
}

/// One elementary stream gathered from a program stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElementaryStream {
    pub stream_id: u8,
    pub data: Vec<u8>,
    /// `(byte offset in data, pts)`: the PTS of the access unit starting at or after the offset.
    pub pts: Vec<(usize, u64)>,
}

/// The PTS of the access unit starting at `offset`: the latest stamp of a packet that began after
/// the previous access unit (`prev`) and at or before this one.
pub(crate) fn pts_for(stamps: &[(usize, u64)], prev: Option<usize>, offset: usize) -> Option<u64> {
    let i = stamps.partition_point(|&(o, _)| o <= offset);
    let &(o, pts) = stamps.get(i.checked_sub(1)?)?;
    match prev {
        Some(p) if o <= p => None,
        _ => Some(pts),
    }
}

/// The stream ids present, in order of first appearance.
pub fn stream_ids(data: &[u8]) -> Result<Vec<u8>> {
    let mut ids = Vec::new();
    for p in ProgramStream::new(data) {
        let p = p?;
        if !ids.contains(&p.stream_id) {
            ids.push(p.stream_id);
        }
    }
    Ok(ids)
}

/// Gathers the payload of every PES packet of `stream_id` into one buffer.
pub fn demux(data: &[u8], stream_id: u8) -> Result<ElementaryStream> {
    let mut out = ElementaryStream {
        stream_id,
        ..Default::default()
    };
    for p in ProgramStream::new(data) {
        let p = p?;
        if p.stream_id != stream_id {
            continue;
        }
        if let Some(pts) = p.pts {
            out.pts.push((out.data.len(), pts));
        }
        out.data.extend_from_slice(p.payload);
    }
    Ok(out)
}

/// Gathers several streams in one pass; the result follows the order of `ids`.
pub fn demux_many(data: &[u8], ids: &[u8]) -> Result<Vec<ElementaryStream>> {
    let mut out: Vec<ElementaryStream> = ids
        .iter()
        .map(|&stream_id| ElementaryStream {
            stream_id,
            ..Default::default()
        })
        .collect();
    for p in ProgramStream::new(data) {
        let p = p?;
        if let Some(es) = out.iter_mut().find(|e| e.stream_id == p.stream_id) {
            if let Some(pts) = p.pts {
                es.pts.push((es.data.len(), pts));
            }
            es.data.extend_from_slice(p.payload);
        }
    }
    Ok(out)
}

/// Builders for synthetic program streams, used by tests in this module and the decoders.
#[cfg(test)]
pub(crate) mod build {
    /// An MPEG-1 pack header with the given SCR.
    pub fn pack(scr: u64) -> Vec<u8> {
        let mut v = vec![0, 0, 1, 0xba];
        v.push(0x21 | ((scr >> 29) & 0x0e) as u8);
        v.push((scr >> 22) as u8);
        v.push(((scr >> 14) & 0xfe) as u8 | 1);
        v.push((scr >> 7) as u8);
        v.push(((scr << 1) & 0xfe) as u8 | 1);
        v.extend_from_slice(&[0x80, 0x2c, 0xad]);
        v
    }

    /// An MPEG-1 PES packet, with a PTS if given.
    pub fn pes(id: u8, pts: Option<u64>, payload: &[u8]) -> Vec<u8> {
        let mut h = vec![0xff, 0xff];
        match pts {
            Some(t) => {
                h.push(0x21 | ((t >> 29) & 0x0e) as u8);
                h.push((t >> 22) as u8);
                h.push(((t >> 14) & 0xfe) as u8 | 1);
                h.push((t >> 7) as u8);
                h.push(((t << 1) & 0xfe) as u8 | 1);
            }
            None => h.push(0x0f),
        }
        let len = h.len() + payload.len();
        let mut v = vec![0, 0, 1, id, (len >> 8) as u8, len as u8];
        v.extend_from_slice(&h);
        v.extend_from_slice(payload);
        v
    }

    /// Splits two elementary streams into a program stream of `chunk`-byte packets.
    pub fn mux(video: &[u8], audio: &[u8], chunk: usize) -> Vec<u8> {
        let mut out = pack(0);
        out.extend_from_slice(&[0, 0, 1, 0xbb, 0, 6, 0x80, 0x2c, 0xad, 0x04, 0xe1, 0xff]);
        let (mut v, mut a) = (video.chunks(chunk), audio.chunks(chunk));
        let mut n = 0u64;
        loop {
            let (cv, ca) = (v.next(), a.next());
            if cv.is_none() && ca.is_none() {
                break;
            }
            out.extend(pack(n * 3000));
            if let Some(c) = cv {
                out.extend(pes(0xe0, (n == 0).then_some(60_750), c));
            }
            if let Some(c) = ca {
                out.extend(pes(0xc0, Some(60_750 + n), c));
            }
            out.extend(pes(0xbe, None, &[0xff; 5]));
            n += 1;
        }
        out.extend_from_slice(&[0, 0, 1, 0xb9]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;

    #[test]
    fn demuxes_synthetic_streams_with_pts() {
        let video: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8 | 0x80).collect();
        let audio: Vec<u8> = (0..700u32).map(|i| (i * 3) as u8 | 0x80).collect();
        let ps = mux(&video, &audio, 300);
        assert_eq!(stream_ids(&ps).unwrap(), vec![0xe0, 0xc0]);
        let es = demux_many(&ps, &[0xe0, 0xc0]).unwrap();
        assert_eq!(es[0].data, video);
        assert_eq!(es[1].data, audio);
        assert_eq!(es[0].pts, vec![(0, 60_750)]);
        assert_eq!(es[1].pts, vec![(0, 60_750), (300, 60_751), (600, 60_752)]);
        assert_eq!(demux(&ps, 0xc0).unwrap(), es[1]);
        assert_eq!(pts_for(&es[1].pts, None, 450), Some(60_751));
        assert_eq!(pts_for(&es[1].pts, Some(310), 450), None);
    }

    #[test]
    fn timestamps_round_trip_33_bits() {
        let t = (1u64 << 33) - 12345;
        let p = pes(0xe0, Some(t), &[1, 2, 3]);
        let got: Vec<_> = ProgramStream::new(&p).collect::<Result<_>>().unwrap();
        assert_eq!(got[0].pts, Some(t));
        assert_eq!(got[0].payload, &[1, 2, 3]);
    }

    #[test]
    fn truncated_and_corrupt_streams_error_without_panicking() {
        let ps = mux(&[0x55; 900], &[0x66; 900], 200);
        for cut in 0..ps.len() {
            let _ = demux(&ps[..cut], 0xe0);
        }
        // A packet whose length runs past the end is an error.
        let cut = &ps[..ps.len() - 30];
        assert!(demux(cut, 0xc0).is_err());
        let mut seed = 0x1234_5678u32;
        for _ in 0..300 {
            let mut bad = ps.clone();
            for _ in 0..8 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let i = (seed >> 8) as usize % bad.len();
                bad[i] = (seed >> 3) as u8;
            }
            let _ = demux_many(&bad, &[0xe0, 0xc0]);
        }
    }
}
