//! The station result's 3D replay (camera task 0x419730 / 0x41a010 / 0x41a470, docs/re/scene.md
//! §8): with the drive's task list gone, the 3D layer shows a platform set at the origin with
//! the player's train replaying the last 4 s of its approach, in two views: a large orbit view
//! that follows the stop error and the small "platform monitor" inside the monitor frame.

use ddg_engine::stop_replay::StopRecord;
use ddg_formats::result_camera::{
    self, CouplingFlags, MONITOR_VIEWPORT, ORBIT_VIEWPORT, PlatformSet, ResultCamera, ResultTables,
    ResultViewport, STOPPED_BELOW_M, TrainSet,
};
use ddg_formats::traffic::{CarModel, TrainModels};
use ddg_render::{Camera, FrameGeometry, PartLibrary, Projection, Renderer, SubView};
use glam::{Mat4, Vec3};

/// The original's 496×400 virtual screen (renderer.md §5).
const VIRTUAL_W: f32 = 496.0;
const VIRTUAL_H: f32 = 400.0;

/// What 0x419730 reads when it is created.
pub struct ResultViewInput<'a> {
    pub stage: usize,
    /// Segment-start station (`0x856d78`); the train stands at `station + 1`.
    pub station: usize,
    /// Timetable field 7 of the arrival station.
    pub field7: i32,
    pub train_type: usize,
    pub record: &'a StopRecord,
    /// The coupling flags of `0x855ff8` (course event 12, the minigame).
    pub coupling: CouplingFlags,
}

/// The replay task's state.
pub struct ResultView {
    tables: ResultTables,
    platform: PlatformSet,
    set: TrainSet,
    lead: Vec<CarModel>,
    lead_after: Vec<CarModel>,
    waiting: Option<(Vec<CarModel>, Vec<CarModel>)>,
    samples: Vec<f32>,
    /// Next sample (`task+0x30`) and samples left (`+0x34`).
    next: usize,
    left: usize,
    /// The sample the views show now and at the previous tick (for interpolated frames).
    dist: f32,
    prev_dist: f32,
    /// `task+0x1cc`: draws since the record showed the train stopped (coupling stations).
    stopped: i32,
}

impl ResultView {
    /// 0x419730. The original rewrites its ring in place (the 40 m shift, the made-up overrun
    /// approach); the port replays a copy, which only differs for a later result that finds
    /// nothing recorded.
    pub fn new(tables: &ResultTables, models: &TrainModels, input: ResultViewInput<'_>) -> Self {
        let platform = PlatformSet::from_field(input.field7);
        let set = tables.train_set(input.stage, input.station, input.train_type, input.coupling);
        let cars = |row: usize| {
            models
                .row(row)
                .map(|m| m.cars[1].clone())
                .unwrap_or_default()
        };
        let mut record = input.record.clone();
        let samples = record.replay(set.shift_m);
        let dist = samples.first().copied().unwrap_or(record.stale_first());
        Self {
            tables: tables.clone(),
            platform,
            set,
            lead: cars(set.lead),
            lead_after: set.lead_after.map(cars).unwrap_or_default(),
            waiting: set.waiting.map(|(a, b)| (cars(a), cars(b))),
            left: samples.len(),
            samples,
            next: 0,
            dist,
            prev_dist: dist,
            stopped: input.coupling.stopped_counter_start(),
        }
    }

    /// One tick of 0x41a010: the next sample until the record runs out, then hold.
    pub fn tick(&mut self) {
        self.prev_dist = self.dist;
        if self.left >= 1 {
            self.dist = self.samples[self.next];
            self.left -= 1;
            if self.left > 0 {
                self.next += 1;
            }
        }
        // 0x41a470 counts draws with a waiting train and the record at a standstill.
        if self.waiting.is_some()
            && self.samples.get(self.next).copied().unwrap_or(self.dist) < STOPPED_BELOW_M
        {
            self.stopped = self.stopped.saturating_add(1);
        }
    }

    /// The stop error shown `alpha` of the way from the last tick to the next.
    fn dist_at(&self, alpha: f32) -> f32 {
        self.prev_dist + (self.dist - self.prev_dist) * alpha.clamp(0.0, 1.0)
    }

    /// The draw callback 0x41a470: the platform part at the origin, the player's cars from
    /// the replayed position back along −z, and at the coupling stations the waiting train's
    /// last two cars ahead.
    pub fn compose(&self, frame: &mut FrameGeometry, library: &PartLibrary, alpha: f32) {
        frame.clear(library);
        frame.add_instance(library, self.platform.part(), &Mat4::IDENTITY);
        let lead = if self.stopped > result_camera::LEAD_SWITCH_AFTER && !self.lead_after.is_empty()
        {
            &self.lead_after
        } else {
            &self.lead
        };
        let mut z = result_camera::train_z(self.dist_at(alpha), self.set.offset);
        for car in lead {
            z += car.spacing;
            if let Some(lod) = result_camera::car_lod(0.0, z) {
                let part = car.lods[lod];
                frame.add_instance(
                    library,
                    part,
                    &Mat4::from_translation(Vec3::new(0.0, 0.0, z)),
                );
            }
        }
        if let Some((before, after)) = &self.waiting {
            let cars = if self.stopped > result_camera::WAITING_SWITCH_AFTER {
                after
            } else {
                before
            };
            if let Some(last) = cars.last() {
                let mut z = last.spacing * 0.5;
                for car in cars.iter().rev().take(2) {
                    z -= car.spacing;
                    if car.lods[0] != 0 {
                        frame.add_instance(
                            library,
                            car.lods[0],
                            &Mat4::from_translation(Vec3::new(0.0, 0.0, z)),
                        );
                    }
                }
            }
        }
    }

    /// The orbit camera (eye `0x84c11c`, target `0x84c10c`).
    pub fn orbit_camera(&self, alpha: f32) -> Camera {
        camera(self.tables.orbit(self.platform, self.dist_at(alpha)))
    }

    /// The fixed monitor camera (`0x84c100` / `0x84c13c`).
    pub fn monitor_camera(&self) -> Camera {
        camera(self.platform.monitor())
    }

    /// Draws both views into `target`, positioned like the 640×480 sprite canvas at
    /// `canvas` (x, y, w, h pixels) so the monitor frame sprite lines up. The first view
    /// clears the target to black.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        renderer: &mut Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        size: (u32, u32),
        canvas: [f32; 4],
        square_pixels: bool,
        alpha: f32,
    ) {
        let views = [
            (ORBIT_VIEWPORT, self.orbit_camera(alpha), true),
            (MONITOR_VIEWPORT, self.monitor_camera(), false),
        ];
        for (vp, cam, clear) in views {
            let sub = sub_view(&vp, canvas, square_pixels, clear);
            renderer.render_view(device, queue, target, size, &cam, &sub);
        }
    }
}

fn camera(c: ResultCamera) -> Camera {
    Camera {
        eye: Vec3::from(c.eye),
        target: Vec3::from(c.target),
    }
}

/// One of the result's viewports in target pixels, with the original's projection: its FOV,
/// and its aspect after the 4.12 round trip of `Camera_SetProjection` (0x4183b0), stretched
/// to the viewport's pixel shape as the original's non-square virtual screen does (or with
/// square pixels under that option).
pub fn sub_view(
    vp: &ResultViewport,
    canvas: [f32; 4],
    square_pixels: bool,
    clear: bool,
) -> SubView {
    let [cx, cy, cw, ch] = canvas;
    let (sx, sy) = (cw / VIRTUAL_W, ch / VIRTUAL_H);
    let rect = [
        cx + (vp.centre[0] - vp.half[0]) * sx,
        cy + (vp.centre[1] - vp.half[1]) * sy,
        2.0 * vp.half[0] * sx,
        2.0 * vp.half[1] * sy,
    ];
    let quantised = 4096.0 / (4096.0 / vp.aspect).trunc();
    let stretch = if square_pixels {
        1.0
    } else {
        (rect[2] / rect[3]) / quantised
    };
    SubView {
        rect,
        projection: Projection {
            fov_tenths: vp.fov_tenths,
            stretch,
            ..Default::default()
        },
        clear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_sits_inside_the_frame_sprite() {
        // At 640×480 the monitor view is pixels (18.1, 192)–(214.2, 384); the frame sprite
        // (tbl 0x25e) is placed at (16, 192).
        let s = sub_view(&MONITOR_VIEWPORT, [0.0, 0.0, 640.0, 480.0], false, false);
        assert!((s.rect[0] - 18.06).abs() < 0.01 && s.rect[1] == 192.0);
        assert!((s.rect[3] - 192.0).abs() < 1e-4);
        // The projection keeps the original's aspect (1.0) inside a 196×192 px viewport.
        let m = s.projection.matrix(s.rect[2] / s.rect[3]);
        assert!((m.y_axis.y / m.x_axis.x - 1.0).abs() < 1e-5);
    }

    #[test]
    fn orbit_view_is_the_right_hand_block() {
        let s = sub_view(&ORBIT_VIEWPORT, [0.0, 0.0, 640.0, 480.0], false, true);
        assert!((s.rect[0] + s.rect[2] - 640.0).abs() < 1e-3);
        assert!((s.rect[1] - 100.8).abs() < 1e-3);
    }
}
