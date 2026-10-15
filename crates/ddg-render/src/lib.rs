//! wgpu renderer: route, scenery, cab, HUD, text.
//!
//! Works in the original's left-handed, Y-up game space (docs/re/renderer.md). Phase 3 covers the
//! world (scenery, track, background) with the original projection, fog and texturing.

pub mod camera;
pub mod gpu;
pub mod world;

pub use camera::{Camera, Projection};
pub use gpu::{Fog, Renderer};
pub use world::{FrameGeometry, PartLibrary};
