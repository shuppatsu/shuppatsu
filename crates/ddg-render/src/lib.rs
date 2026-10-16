//! wgpu renderer: route, scenery, cab, HUD, text.
//!
//! Works in the original's left-handed, Y-up game space (docs/re/renderer.md). Phase 3 covers the
//! world (scenery, track, background) with the original projection, fog and texturing.

pub mod camera;
pub mod effects;
pub mod gpu;
pub mod label;
pub mod mip;
pub mod overlay;
pub mod sprites;
pub mod text;
pub mod world;

pub use camera::{Camera, Projection};
pub use gpu::{Fog, Renderer, SubView};
pub use world::{FrameGeometry, PartLibrary};
