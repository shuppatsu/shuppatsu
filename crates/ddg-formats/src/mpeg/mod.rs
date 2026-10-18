//! MPEG-1 movie decoding: the program stream demultiplexer, the video decoder and the Layer II
//! audio decoder, written from ISO/IEC 11172-1/-2/-3. Used for `komachi.dat`
//! (docs/re/movie.md).

mod bits;
mod idct;
pub mod mp2;
pub mod ps;
pub mod video;
mod vlc;
mod vtables;

pub use mp2::{AudioFrame, Mp2Decoder, Mp2Stream};
pub use ps::{ElementaryStream, ProgramStream};
pub use video::{Frame, PictureType, SequenceHeader, VideoDecoder};

use crate::{FormatError, Result};

/// The first video and audio streams of a program stream, ready to decode.
pub struct Movie {
    pub video: VideoDecoder,
    pub audio: Option<Mp2Stream>,
}

impl Movie {
    /// Demultiplexes the first MPEG video stream (0xe0..=0xef) and the first MPEG audio stream
    /// (0xc0..=0xdf) of a program stream.
    pub fn open(data: &[u8]) -> Result<Self> {
        let ids = ps::stream_ids(data)?;
        let video_id = ids
            .iter()
            .copied()
            .find(|id| (0xe0..=0xef).contains(id))
            .ok_or_else(|| FormatError::invalid("mpeg program stream", "no video stream"))?;
        let audio_id = ids.iter().copied().find(|id| (0xc0..=0xdf).contains(id));
        let mut wanted = vec![video_id];
        wanted.extend(audio_id);
        let mut es = ps::demux_many(data, &wanted)?;
        let audio = audio_id.map(|_| Mp2Stream::new(es.pop().unwrap_or_default()));
        let video = VideoDecoder::new(es.swap_remove(0));
        Ok(Self { video, audio })
    }
}
