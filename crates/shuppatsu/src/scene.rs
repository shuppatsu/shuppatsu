//! Route scenes: loads a route with its area's models, and composes each frame the way the
//! original does (docs/re/scene.md): track pieces for the camera position, live scenery, and
//! the 2D background.

use anyhow::{Context, Result};
use glam::{Mat4, Vec3};
use shuppatsu_formats::scene::{
    self, LiveInterval, ORIGINAL_TAN_HALF_FOV_X, SceneContext, SceneTables, TrackPos, WideView,
};
use shuppatsu_formats::{
    exe::Exe, part::Part, rom2::Rom2, route::Route, stages::StageTables, tex::TexFile,
};
use shuppatsu_render::{Camera, FrameGeometry, PartLibrary};
use std::path::PathBuf;

#[derive(Clone)]
pub struct GameData {
    data: PathBuf,
    bin: PathBuf,
    /// Enhancement `texture_pack`: a directory of replacement texture pages
    /// (docs/texture-packs.md); `None` = the original textures.
    pub texture_pack: Option<PathBuf>,
}

impl GameData {
    pub fn from_env() -> Result<Self> {
        let install = crate::data_dir::locate()?;
        Ok(Self {
            data: install.join("DATA"),
            bin: install.join("BIN"),
            texture_pack: None,
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

    /// Adds the texture file `name` (`texR1.rom`, `texTR2.rom`, …) to `library`, with the
    /// pages the texture pack replaces.
    pub fn add_textures(&self, library: &mut PartLibrary, name: &str) -> Result<()> {
        let tex = TexFile::parse(&self.read(name)?).with_context(|| name.to_owned())?;
        let base = library.pages.len();
        library.add_textures(&tex);
        if let Some(dir) = &self.texture_pack {
            crate::texpack::apply(dir, name, &mut library.pages[base..]);
        }
        Ok(())
    }

    /// The installed `DATA` directory.
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data
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

/// Driving FOV in tenths of a degree (scene.md §3, likely).
const DRIVING_FOV_TENTHS: u16 = (scene::DRIVING_FOV_Y_DEG * 10.0) as u16;
/// Depth words of the gradient quads (renderer.md §8: backgrounds at ≈0x7ffe; the ground is
/// drawn just in front of the sky; likely).
const SKY_GRADIENT_DEPTH: u16 = 0x7ffe;
const GROUND_FLAT_DEPTH: u16 = 0x7ffd;

/// Bucket look-back for side tracks in wide mode (scene.md §5.2).
const WIDE_EXTRA_BUCKETS: usize = 16;

/// Enhancement `draw_distance`: the far plane as a multiple of the original's 32 units
/// (320 m).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrawDistance {
    /// 32 units, the original's far plane.
    Original,
    /// 64 units: the decided default (docs/settings.md).
    #[default]
    Double,
    Triple,
    /// 4×, 128 units: as far as the panorama's anchor (scene.md §6.4), behind which the
    /// background shows mountains anyway.
    Max,
}

impl DrawDistance {
    pub const ALL: [Self; 4] = [Self::Original, Self::Double, Self::Triple, Self::Max];

    /// The far plane in world units.
    pub fn far(self) -> f32 {
        shuppatsu_render::camera::FAR
            * match self {
                Self::Original => 1.0,
                Self::Double => 2.0,
                Self::Triple => 3.0,
                Self::Max => 4.0,
            }
    }

    /// The name in `config.toml`, on the command line and in the environment.
    pub fn name(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Double => "2x",
            Self::Triple => "3x",
            Self::Max => "max",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|d| {
            d.name() == s || (*d == Self::Double && s == "2") || (*d == Self::Triple && s == "3")
        })
    }
}

/// The geometry enhancements of the route view (docs/settings.md); `Default` is the
/// original.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Geometry {
    /// `max_lod`: the most detailed model of every track piece and AI-train car at every
    /// distance (scene.md §5.3).
    pub max_lod: bool,
    /// `even_track_width`: rail and track-side pieces of even width, so the rails do not step
    /// sideways at the joins (scene.md §2.3).
    pub even_track_width: bool,
    /// `early_activation`: scenery and side tracks from where they come within the far plane
    /// instead of the original's activation points (scene.md §5.3).
    pub early_activation: bool,
    /// `draw_distance`: the far plane; beyond the original, scenery and track are activated
    /// and built out to it.
    pub draw_distance: DrawDistance,
    /// `distance_fade`: the world dissolves into the background over the last stretch before
    /// the panorama's depth instead of ending at a hard edge.
    pub distance_fade: bool,
}

impl Geometry {
    pub fn far(&self) -> f32 {
        self.draw_distance.far()
    }

    /// How far ahead track and scenery are activated: `None` = the original's lists.
    pub fn reach(&self) -> Option<f32> {
        (self.early_activation || self.draw_distance != DrawDistance::Original).then(|| self.far())
    }

    /// The eye depths of `distance_fade`: it ends where the panorama (depth word 0x7ff0,
    /// drawn in front of everything behind it) starts to hide the world, and spans a quarter
    /// of the far plane.
    pub fn fade(&self) -> Option<(f32, f32)> {
        self.distance_fade.then(|| {
            let far = self.far();
            let end = shuppatsu_render::camera::background_eye_depth(scene::SKY_SPRITE_DEPTH, far);
            (end - far / 4.0, end)
        })
    }

    /// Sets the far plane and the fade on a renderer drawing this scene.
    pub fn configure(&self, r: &mut shuppatsu_render::Renderer) {
        r.projection.far = self.far();
        r.distance_fade = self.fade();
    }
}

pub struct RouteScene {
    pub route: Route,
    pub tables: SceneTables,
    pub ctx: SceneContext,
    pub library: PartLibrary,
    pub eye_height: f32,
    intervals: Vec<LiveInterval>,
    /// The intervals before `early_activation` / `draw_distance` (original or widened).
    base_intervals: Vec<LiveInterval>,
    visibility: Visibility,
    /// The geometry enhancements ([`RouteScene::set_geometry`]).
    geometry: Geometry,
    /// Enhancement `stable_track_variants` (off = the original's flicker): side-track model
    /// variants anchored to the track instead of switching every point.
    pub stable_track_variants: bool,
    /// Gradient set: 0 normal, 1 the alternate set of the timed per-stage event (scene.md §6.5).
    pub gradient_variant: usize,
    /// A palette fade toward `gradient_variant` in progress: (from variant, frame of
    /// [`shuppatsu_formats::scene::PALETTE_FADE_TICKS`]) (task 0x436a60, scene.md §6.2).
    pub gradient_fade: Option<(usize, u32)>,
    /// Skip the panorama sprites (`0x8502ac == 0` while it snows; 0x41dfc0, renderer.md §13).
    pub hide_panorama: bool,
    /// AI-train cars to draw this frame ([`shuppatsu_engine::drive::Drive::traffic_poses`]).
    pub traffic: Vec<shuppatsu_engine::traffic::CarPose>,
    /// Enhancement `far_traffic`: draw AI trains this far ahead (world units) in the farthest
    /// distance band instead of the original's 15 (`None`: the original bands).
    pub traffic_reach: Option<f64>,
    /// Frame of the animated 3D signals: the rules' `signal_model` (`0x85744c`).
    pub signal_model: usize,
    /// The texture pages already on the GPU when the scene was loaded on a thread
    /// ([`shuppatsu_render::PageUploader`]), for the run to take instead of uploading them.
    pub gpu_pages: Option<shuppatsu_render::UploadedPages>,
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
            data.add_textures(&mut library, &tex)?;
        }
        for parts in ["partsRC.rom".to_owned(), format!("partsR{r_index}.rom")] {
            let bytes = data.read(&parts)?;
            let entries = Rom2::parse(&bytes)?
                .iter()
                .map(Part::parse_entry)
                .collect::<shuppatsu_formats::Result<Vec<_>>>()
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
            base_intervals: intervals.clone(),
            intervals,
            visibility,
            geometry: Geometry::default(),
            stable_track_variants: false,
            gradient_variant: 0,
            gradient_fade: None,
            hide_panorama: false,
            traffic: Vec::new(),
            traffic_reach: None,
            signal_model: 0,
            gpu_pages: None,
        })
    }

    /// Applies the geometry enhancements: activation moved ahead to their reach.
    pub fn set_geometry(&mut self, geometry: Geometry) {
        self.geometry = geometry;
        self.intervals = match geometry.reach() {
            None => self.base_intervals.clone(),
            Some(reach) => {
                let tan_half_fov_x = match self.visibility {
                    Visibility::Original => ORIGINAL_TAN_HALF_FOV_X,
                    Visibility::Wide { tan_half_fov_x } => tan_half_fov_x,
                };
                let library = &self.library;
                scene::activate_early(
                    &self.route,
                    &self.base_intervals,
                    scene::EarlyView {
                        reach,
                        tan_half_fov_x,
                        // Twice the reach in points: the track may wind.
                        max_extra_points: (2.0 * reach / shuppatsu_formats::route::UNITS_PER_POINT)
                            as usize,
                    },
                    |part| library.horizontal_radius(part),
                )
            }
        };
    }

    pub fn geometry(&self) -> Geometry {
        self.geometry
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
    /// `aspect` is the viewport's width / height in pixels (for the 2D background layout).
    pub fn compose(&self, frame: &mut FrameGeometry, pos: TrackPos, show_lines: bool, aspect: f32) {
        frame.clear(&self.library);
        let far = self.geometry.far();
        frame.far = (far != shuppatsu_render::camera::FAR).then_some(far);
        let cam = scene::cab_camera(&self.route, pos, self.eye_height, 0.0);

        self.background(frame, pos, ScreenMap { aspect });
        // Positions relative to the eye keep coplanar layers coplanar (renderer.md §6.1).
        frame.origin = Vec3::from(cam.eye);

        let options = scene::TrackOptions {
            anchor_side_variants: self.stable_track_variants,
            max_lod: self.geometry.max_lod,
            even_width: self.geometry.even_track_width,
            reach: self.geometry.reach(),
        };
        let pieces = match self.visibility {
            Visibility::Original => {
                scene::track_instances_with(&self.route, &self.tables, self.ctx, pos, &cam, options)
            }
            Visibility::Wide { .. } => scene::track_instances_wide_with(
                &self.route,
                &self.tables,
                self.ctx,
                pos,
                &cam,
                WIDE_EXTRA_BUCKETS,
                options,
            ),
        };
        for p in &pieces {
            let m = Mat4::from_cols_array(&p.transform.to_cols_array());
            // Non-rail runs (fields, snow, gravel, verges) may be ground for the shading
            // enhancements; rails, buffer stops and the tunnel plug never are (shuppatsu_render::ground).
            if matches!(p.source, scene::PieceSource::Kind { .. }) {
                frame.add_terrain_instance(&self.library, p.part_id, &m);
            } else {
                frame.add_instance(&self.library, p.part_id, &m);
            }
        }

        let live = scene::live_at(&self.intervals, pos.engine_index);
        for s in scene::scenery_from(&self.route, &self.tables, self.ctx, pos.engine_index, live) {
            let m = Mat4::from_cols_array(&s.transform.to_cols_array());
            // Animated signal parts (classes 0–5) show frame `0x85744c` (0x401140).
            let part = match self.tables.animated(s.part_id) {
                Some(a) if s.animated && a.class < 6 => a
                    .frames
                    .get(self.signal_model)
                    .copied()
                    .unwrap_or(s.part_id),
                _ => s.part_id,
            };
            // Scenery may hold ground slabs (only its upward-facing groups count).
            frame.add_terrain_instance(&self.library, part, &m);
        }
        crate::traffic::add_cars(self, frame, pos, &self.traffic);

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

    /// The 2D background (scene.md §6): sky gradient, flat or textured ground, panorama
    /// sprites and the area-3 extra quads, each at the original's depth word.
    fn background(&self, frame: &mut FrameGeometry, pos: TrackPos, screen: ScreenMap) {
        let plan = scene::background(&self.route, &self.tables, self.ctx, pos);
        let bg = &self.tables.background;
        let r_index = self.ctx.area + 1;
        let Some(gradient) = bg.gradients[self.gradient_variant.min(1)].get(r_index) else {
            return;
        };
        let mut palette = gradient.palette();
        if let Some((from, frame)) = self.gradient_fade
            && let Some(g) = bg.gradients[from.min(1)].get(r_index)
        {
            palette = scene::palette_fade(&g.palette(), &palette, frame);
        }
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
        let (left, right) = screen.x_range();
        let x_range = [left, right];
        let to_ndc = |xy: [[f32; 2]; 4]| xy.map(|[x, y]| screen.ndc(x, y));

        if plan.sky_gradient {
            // The original's sky starts at virtual y 66, under the cab roof. The viewer has no
            // cab, so the sky's top edge is raised to the top of the window.
            let mut xy = bg.sky_quad.widened(x_range);
            let top = xy.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
            for p in &mut xy {
                if p[1] == top {
                    p[1] = f32::from(VIEWER_TOP);
                }
            }
            frame.add_bg_quad(
                to_ndc(xy),
                bg.sky_quad.colours.map(colour),
                SKY_GRADIENT_DEPTH,
            );
        }

        // A sprite texture from a part's first group (panorama sprites, textured ground).
        let sprite = |part_id: u32| -> Option<(usize, scene::SpriteTexture)> {
            let group = self.library.part(part_id)?.groups.first()?;
            let (page, rect) = self.library.rect(group.rect_id)?;
            Some((page, scene::SpriteTexture::from_group(group, &rect)?))
        };

        match &plan.ground {
            scene::Ground::None => {}
            scene::Ground::Flat(q) => frame.add_bg_quad(
                to_ndc(q.widened(x_range)),
                q.colours.map(colour),
                GROUND_FLAT_DEPTH,
            ),
            g @ scene::Ground::Textured { part_id, depth, .. } => {
                if let Some((page, tex)) = sprite(*part_id) {
                    for q in scene::ground_quad(bg, g, &tex, x_range) {
                        frame.add_bg_tex_quad(page, to_ndc(q.xy), q.uv, *depth);
                    }
                }
            }
        }

        let cam = scene::cab_camera(&self.route, pos, self.eye_height, 0.0);
        let view = scene::ViewBasis::from_cab(&cam);
        let tan_half = shuppatsu_render::camera::quantised_tan_half(DRIVING_FOV_TENTHS);
        let vp = scene::Viewport2d::original(tan_half, scene::DRIVING_ASPECT)
            .hor_plus(screen.half_width());
        let sky_sprites: &[usize] = if self.hide_panorama {
            &[]
        } else {
            &plan.sky_sprites
        };
        for &i in sky_sprites {
            let Some(s) = bg.sky_sprites.get(i) else {
                continue;
            };
            let Some((page, tex)) = sprite(s.part_id) else {
                continue;
            };
            let tex = tex.for_sky(s.flags);
            for q in scene::sky_quads(s, &tex, &view, &vp, true) {
                frame.add_bg_tex_quad(page, to_ndc(q.xy), q.uv, q.depth);
            }
        }
        if let Some(which) = plan.extra_quad
            && let Some(q) = scene::extra_quad(bg, which, &view, &vp, true)
        {
            frame.add_bg_quad(to_ndc(q.xy), q.colours.map(colour), q.depth);
        }
    }
}

/// Virtual y of the window's top edge in the viewer's band mapping (the 3D band starts at 52).
const VIEWER_TOP: i16 = 52;

/// Placement of the original's 496×400 virtual screen in the viewer's window. The original's
/// 3D band (virtual y 52..400) fills the window height; virtual x keeps the original's pixel
/// ratio to y (1 x unit = 640/496 px, 1 y unit = 480/400 px at 640×480) around x = 248, so
/// 2D layers line up with the 3D view at any aspect (Hor+ reveals more virtual x).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenMap {
    pub aspect: f32,
}

impl ScreenMap {
    /// Original x:y size ratio of one virtual unit.
    const UNIT_RATIO: f32 = (640.0 / 496.0) / (480.0 / 400.0);

    /// Half the visible width in virtual x units.
    pub fn half_width(&self) -> f32 {
        self.aspect * 348.0 / (2.0 * Self::UNIT_RATIO)
    }

    /// Visible virtual x range (left, right).
    pub fn x_range(&self) -> (f32, f32) {
        (248.0 - self.half_width(), 248.0 + self.half_width())
    }

    pub fn ndc(&self, x: f32, y: f32) -> [f32; 2] {
        [
            (x - 248.0) / self.half_width(),
            1.0 - (y - 52.0) / 348.0 * 2.0,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_map_hor_plus() {
        let m = ScreenMap { aspect: 4.0 / 3.0 };
        assert_eq!(m.ndc(248.0, 52.0), [0.0, 1.0]);
        // 16:10 shows more virtual x than 4:3, centred on 248.
        let wide = ScreenMap { aspect: 1.6 };
        assert!(wide.x_range().0 < m.x_range().0);
        assert!((wide.ndc(wide.x_range().1, 400.0)[0] - 1.0).abs() < 1e-6);
    }
}
