//! MPEG-1 movie decoding: the program stream demultiplexer, the video decoder and the Layer II
//! audio decoder, written from ISO/IEC 11172-1/-2/-3. Used for `komachi.dat`
//! (docs/re/movie.md).

mod bits;
mod idct;
pub mod ps;
pub mod video;
mod vlc;
mod vtables;

pub use ps::{ElementaryStream, ProgramStream};
pub use video::{Frame, PictureType, SequenceHeader, VideoDecoder};
