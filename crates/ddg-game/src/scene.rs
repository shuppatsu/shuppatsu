//! Route scenes: loads a route with its area's models, and composes each frame the way the
//! original does (docs/re/scene.md): track pieces for the camera position, live scenery, and
//! the 2D background.

use anyhow::{Context, Result};
use ddg_formats::scene::{
    self, LiveInterval, ORIGINAL_TAN_HALF_FOV_X, Quad2d, SceneContext, SceneTables, TrackPos,
    WideView,
};
use ddg_formats::{
    exe::Exe, part::Part, rom2::Rom2, route::Route, stages::StageTables, tex::TexFile,
};
use ddg_render::{Camera, FrameGeometry, PartLibrary};
use glam::{Mat4, Vec3};
use std::path::PathBuf;

pub struct GameData {
    data: PathBuf,
    bin: PathBuf,
}

impl GameData {
    pub fn from_env() -> Result<Self> {
        let root = std::env::var_os("DDG_DATA_DIR")
            .map(PathBuf::from)
            .context("DDG_DATA_DIR is not set (run inside `nix develop`)")?;
        let install = root.join("install/Program_Executable_Files");
        Ok(Self {
            data: install.join("DATA"),
            bin: install.join("BIN"),
        })
    }

    /// Reads a data file by case-insensitive name (the exe uses lowercase, the disc some uppercase).
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let path = std::fs::read_dir(&self.data)
            .with_context(|| format!("reading {}", self.data.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
            })
            .with_context(|| format!("{name} not found in {}", self.data.display()))?;
        Ok(std::fs::read(path)?)
    }

    pub fn exe(&self) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.bin.join("dengo3k.exe"))?)
    }
}

/// How visibility treats the area outside the original 4:3 view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Visibility {
    /// Exactly the original's precomputed lists (objects vanish at ~32° off-axis).
    Original,
    /// Enhancement `wide_visibility`: keep objects and side tracks until they leave the wider
    /// view; unchanged inside the original view.
    Wide { tan_half_fov_x: f32 },
}

/// Bucket look-back for side tracks in wide mode (scene.md §5.2).
const WIDE_EXTRA_BUCKETS: usize = 16;

pub struct RouteScene {
    pub route: Route,
    pub tables: SceneTables,
    pub ctx: SceneContext,
    pub library: PartLibrary,
    pub eye_height: f32,
    intervals: Vec<LiveInterval>,
    visibility: Visibility,
}

impl RouteScene {
    /// Loads `com_name` (e.g. "yamate1a.com").
    pub fn load(data: &GameData, com_name: &str, visibility: Visibility) -> Result<Self> {
        let exe_bytes = data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let stages = StageTables::read(&exe)?;
        let com = stages
            .com_files
            .iter()
            .position(|f| {
                f.as_deref()
                    .is_some_and(|f| f.eq_ignore_ascii_case(com_name))
            })
            .with_context(|| format!("{com_name} is not a route file listed in the exe"))?;
        let r_index = stages
            .stages
            .iter()
            .find(|s| s.segments.iter().any(|seg| seg.com as usize == com))
            .map(|s| s.area)
            .with_context(|| format!("no stage uses {com_name}"))?;

        let route = Route::parse(&data.read(com_name)?).with_context(|| com_name.to_owned())?;
        let tables = SceneTables::read(&exe, scene::max_run_kind(&route))?;
        let ctx = SceneContext {
            area: r_index as usize - 1,
            com: com as u32,
            station: 1,
        };

        let mut library = PartLibrary::default();
        // Common set first, then the area: later rects win, earlier parts win (as the game).
        for tex in ["texRC.rom".to_owned(), format!("texR{r_index}.rom")] {
            library.add_textures(&TexFile::parse(&data.read(&tex)?).with_context(|| tex.clone())?);
        }
        for parts in ["partsRC.rom".to_owned(), format!("partsR{r_index}.rom")] {
            let bytes = data.read(&parts)?;
            let entries = Rom2::parse(&bytes)?
                .iter()
                .map(Part::parse_entry)
                .collect::<ddg_formats::Result<Vec<_>>>()
                .with_context(|| parts.clone())?;
            library.add_parts(entries);
        }

        let original = scene::live_intervals(&route);
        let intervals = match visibility {
            Visibility::Original => original,
            Visibility::Wide { tan_half_fov_x } => scene::widen_intervals(
                &route,
                &original,
                WideView {
                    tan_half_fov_x,
                    original_tan_half_fov_x: ORIGINAL_TAN_HALF_FOV_X,
                    max_extra_points: 400,
                },
                |part| library.horizontal_radius(part),
            ),
        };
        Ok(Self {
            eye_height: tables.eye_height(ctx.area, ctx.com),
            route,
            tables,
            ctx,
            library,
            intervals,
            visibility,
        })
    }

    /// The camera position range the original allows on this route.
    pub fn limits(&self) -> (usize, usize) {
        scene::camera_limits(&self.route)
    }

    pub fn cab_camera(&self, pos: TrackPos) -> Camera {
        let c = scene::cab_camera(&self.route, pos, self.eye_height, 0.0);
        Camera {
            eye: Vec3::from(c.eye),
            target: Vec3::from(c.target),
        }
    }

    /// Fills `frame` with everything drawn at `pos`.
    pub fn compose(&self, frame: &mut FrameGeometry, pos: TrackPos, show_lines: bool) {
        frame.clear(&self.library);
        let cam = scene::cab_camera(&self.route, pos, self.eye_height, 0.0);

        self.background(frame, pos);

        let pieces = match self.visibility {
            Visibility::Original => {
                scene::track_instances(&self.route, &self.tables, self.ctx, pos, &cam)
            }
            Visibility::Wide { .. } => scene::track_instances_wide(
                &self.route,
                &self.tables,
                self.ctx,
                pos,
                &cam,
                WIDE_EXTRA_BUCKETS,
            ),
        };
        for p in pieces {
            let m = Mat4::from_cols_array(&p.transform.to_cols_array());
            frame.add_instance(&self.library, p.part_id, &m);
        }

        let live = scene::live_at(&self.intervals, pos.engine_index);
        for s in scene::scenery_from(&self.route, &self.tables, self.ctx, pos.engine_index, live) {
            let m = Mat4::from_cols_array(&s.transform.to_cols_array());
            frame.add_instance(&self.library, s.part_id, &m);
        }

        if show_lines {
            for (i, seg) in self.route.segments.iter().enumerate() {
                let pts: Vec<Vec3> = self.route.slots[seg.points.clone()]
                    .iter()
                    .map(|p| Vec3::new(p.x, p.y(), p.z))
                    .collect();
                let colour = if i == 0 {
                    [1.0, 0.1, 0.1, 1.0]
                } else {
                    [1.0, 0.6, 0.1, 1.0]
                };
                frame.add_line_strip(&pts, colour);
            }
        }
    }

    /// Sky and ground gradients (scene.md §6). Panorama sprites and textured ground are not
    /// drawn yet; textured ground falls back to the flat ground gradient.
    fn background(&self, frame: &mut FrameGeometry, pos: TrackPos) {
        let plan = scene::background(&self.route, &self.tables, self.ctx, pos);
        let bg = &self.tables.background;
        let r_index = self.ctx.area + 1;
        let Some(gradient) = bg.gradients[0].get(r_index) else {
            return;
        };
        let palette = gradient.palette();
        let colour = |word: u16| -> [f32; 4] {
            let [lo, hi] = word.to_le_bytes();
            let srgb = if hi >= 0x80 {
                // Inline ARGB4444 (renderer.md §8).
                let n = |s: u16| f32::from((word >> s) & 0xf) / 15.0;
                [n(8), n(4), n(0)]
            } else if hi == 3 {
                palette[usize::from(lo)].map(|c| f32::from(c) / 255.0)
            } else {
                [0.5; 3]
            };
            let lin = srgb.map(|c| c.powf(2.2));
            [lin[0], lin[1], lin[2], 1.0]
        };
        let mut quad = |q: &Quad2d| {
            frame.add_bg_quad(q.xy.map(virtual_to_ndc), q.colours.map(colour));
        };
        if plan.sky_gradient {
            // The original's sky starts at virtual y 66, under the cab roof. The viewer has no
            // cab, so the sky's top edge is raised to the top of the window.
            let mut sky = bg.sky_quad;
            let top = sky.xy.iter().map(|p| p[1]).min().unwrap_or(0);
            for p in &mut sky.xy {
                if p[1] == top {
                    p[1] = VIEWER_TOP;
                }
            }
            quad(&sky);
        }
        match plan.ground {
            scene::Ground::None => {}
            scene::Ground::Flat(q) => quad(&q),
            scene::Ground::Textured { .. } => quad(&bg.ground_quad),
        }
    }
}

/// Virtual y of the window's top edge in the viewer's band mapping (the 3D band starts at 52).
const VIEWER_TOP: i16 = 52;

/// Maps a 496×400 virtual-screen point to NDC for a viewer whose 3D view fills the window:
/// the original's 3D band (virtual y 52..400) becomes the full height, and x stretches over the
/// full width (background quads are horizontal gradients, so widening doesn't distort them).
fn virtual_to_ndc([x, y]: [i16; 2]) -> [f32; 2] {
    let nx = f32::from(x) / 496.0 * 2.0 - 1.0;
    let ny = 1.0 - (f32::from(y) - 52.0) / 348.0 * 2.0;
    [nx, ny]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_mapping() {
        assert_eq!(virtual_to_ndc([0, 52]), [-1.0, 1.0]);
        assert_eq!(virtual_to_ndc([496, 400]), [1.0, -1.0]);
    }
}
