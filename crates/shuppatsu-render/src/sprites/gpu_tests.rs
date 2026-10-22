//! The GPU compositor against the CPU compose: random layers with every kind of draw must give
//! the same bytes. Skipped without a GPU that runs compute shaders.

use super::legend::{Glyph, GlyphShape, LegendPart};
use super::*;
use crate::rgba::RgbaImage;
use crate::text::tests::BoxGlyphs;
use crate::text::{Style, SubRect, TextOverride, TextPart};

fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter =
        block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    let (device, queue) =
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    crate::gpu2d::supported(&device).then_some((device, queue))
}

/// Composes `frame` on the GPU into `target` (made at the frame's size when it is not, else
/// only the changed bins are composed) and reads the canvas back (RGBA rows).
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    gpu: &mut crate::gpu2d::Compositor,
    frame: &gpu::GpuFrame,
    target: &mut Option<wgpu::Texture>,
) -> Vec<u8> {
    let (w, h) = frame.size;
    let keep = target
        .as_ref()
        .is_some_and(|t| (t.width(), t.height()) == (w, h));
    let texture = target.take().filter(|_| keep).unwrap_or_else(|| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    assert!(gpu.render(device, queue, frame, &view, keep));
    let row = (w * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range().expect("mapped");
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        out.extend_from_slice(&data[y * row as usize..][..w as usize * 4]);
    }
    *target = Some(texture);
    out
}

struct Rng(u32);

impl Rng {
    fn next(&mut self, n: u32) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 % n.max(1)
    }
}

/// A bank with small random maps, a 640 × 480 noise picture (map 2, paper fill) and a band
/// with a two-column pattern (map 3, repeat fill), fonts, and translated text over map 1.
fn bank(rng: &mut Rng, scale: usize, upscale: bool) -> SpriteBank {
    let mut tiles: Vec<Option<Box<[u8; TILE_PIXELS]>>> = vec![None];
    for _ in 0..8 {
        let t: [u8; TILE_PIXELS] = std::array::from_fn(|_| {
            if rng.next(5) == 0 {
                0
            } else {
                1 + rng.next(6) as u8
            }
        });
        tiles.push(Some(Box::new(t)));
    }
    let noise = tiles.len() as u16;
    let (mw, mh) = (40usize, 30usize);
    for ty in 0..mh {
        for tx in 0..mw {
            let tile: [u8; TILE_PIXELS] = std::array::from_fn(|i| {
                let (x, y) = (
                    tx * TILE_SIZE + i % TILE_SIZE,
                    ty * TILE_SIZE + i / TILE_SIZE,
                );
                let hash =
                    (x as u32).wrapping_mul(0x9e37_79b9) ^ (y as u32).wrapping_mul(0x85eb_ca6b);
                (60 + y / 4 + (hash.wrapping_mul(0xc2b2_ae35) >> 28) as usize) as u8
            });
            tiles.push(Some(Box::new(tile)));
        }
    }
    let maps = vec![
        Tilemap {
            width: 1,
            height: 1,
            tiles: vec![1],
        },
        Tilemap {
            width: 3,
            height: 2,
            tiles: vec![2, 3, 4, 0, 5, 6],
        },
        Tilemap {
            width: mw as u8,
            height: mh as u8,
            tiles: (noise..noise + (mw * mh) as u16).collect(),
        },
        Tilemap {
            width: 40,
            height: 2,
            tiles: (0..80).map(|i| 7 + (i % 2) as u16).collect(),
        },
    ];
    let mut b = SpriteBank {
        tiles,
        tilemaps: maps,
        generation: 0,
        art: next_art_id(),
        text: None,
        labels: None,
        display_labels: None,
        legend_font: None,
        wide_fill: Vec::new(),
        paper: Mutex::default(),
        paper_memo: Mutex::default(),
        map_art: Mutex::default(),
        upscale: false,
        hires: Mutex::default(),
        gpu_art: Mutex::default(),
    };
    b.set_label_font(Some(LabelFont::new(Box::new(BoxGlyphs))));
    b.set_legend_font(Some(LabelFont::new(Box::new(BoxGlyphs))));
    assert!(b.set_wide_paper(
        2,
        Paper {
            source_rows: (256, 480),
            source_cols: 128,
            ramp: 24,
            edge_skip: 1,
            edge_cols: 12,
        }
    ));
    assert!(b.set_wide_fill(3));
    let o = TextOverride {
        parts: vec![
            TextPart {
                region: Some([0, 0, 24, 16]),
                text: "Hi".into(),
                style: Style::Plain,
                outline: true,
                ..TextPart::default()
            },
            TextPart {
                region: Some([20, 16, 28, 16]),
                text: "Go".into(),
                style: Style::Plain,
                ..TextPart::default()
            },
        ],
    };
    let overlay =
        crate::text::TextOverlay::build_scaled(&b, [(1, SubRect::WHOLE, &o)], &BoxGlyphs, scale);
    b.set_text(Some(overlay));
    b.set_upscale(upscale);
    b
}

fn random_palette(rng: &mut Rng) -> Palette {
    Palette(std::array::from_fn(|i| {
        if i < 8 {
            rng.next(0x100_0000)
        } else {
            let v = i as u32;
            v | v << 8 | v << 16
        }
    }))
}

fn paint(rng: &mut Rng) -> Paint {
    Paint {
        palette: [0, 1, 3, 9][rng.next(4) as usize],
        index: rng.next(8) as u8,
    }
}

fn random_widget(rng: &mut Rng, image: &Arc<WidgetImage>, matte: &Arc<WidgetImage>) -> WidgetKind {
    match rng.next(7) {
        0 => WidgetKind::Fill(paint(rng)),
        1 => WidgetKind::DownArrow(paint(rng)),
        2 => WidgetKind::Sprite {
            tilemap: rng.next(3) as u16,
            src: [
                rng.next(8) as u16,
                rng.next(8) as u16,
                16 + rng.next(30) as u16,
                16,
            ],
            scale: [0.5, 0.75, 1.0, 1.3, 2.0, 0.37][rng.next(6) as usize],
            palette: [0, 1, 3][rng.next(3) as usize],
        },
        3 => WidgetKind::Text {
            text: ["Hi", "Hello there", "Go!"][rng.next(3) as usize].into(),
            style: TextStyle {
                outline: (rng.next(2) == 0).then(|| paint(rng)),
                backdrop: (rng.next(2) == 0).then(|| paint(rng)),
                align: [crate::text::Align::Left, crate::text::Align::Center][rng.next(2) as usize],
                vertical: rng.next(4) == 0,
                ..TextStyle::plain(
                    paint(rng),
                    [None, Some(5.0), Some(9.0)][rng.next(3) as usize],
                )
            },
        },
        4 => {
            let glyph = |shape| {
                LegendPart::Glyph(Glyph {
                    shape,
                    label: "A".into(),
                    fill: [0, 200, 40],
                    ink: [255, 255, 255],
                })
            };
            WidgetKind::Legend {
                parts: vec![
                    glyph(GlyphShape::Round),
                    LegendPart::Text("Confirm".into()),
                    LegendPart::Gap(1.0),
                    glyph(GlyphShape::Key),
                    glyph(GlyphShape::DPad {
                        lit: [true, false, false, true],
                    }),
                    glyph(GlyphShape::Stick),
                ],
                style: TextStyle {
                    backdrop: (rng.next(2) == 0).then(|| paint(rng)),
                    ..TextStyle::plain(paint(rng), Some(6.0))
                },
            }
        }
        _ => WidgetKind::Image(if rng.next(2) == 0 {
            image.clone()
        } else {
            matte.clone()
        }),
    }
}

#[test]
fn gpu_composes_the_same_pixels() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU with compute shaders: skipped");
        return;
    };
    let mut gpu = crate::gpu2d::Compositor::new(&device);
    let mut rng = Rng(0x2545_f491);
    let pic = RgbaImage {
        width: 5,
        height: 3,
        rgba: (0..15)
            .flat_map(|i| {
                [
                    i * 17,
                    255 - i * 9,
                    90,
                    [0, 60, 128, 200, 255][i as usize % 5],
                ]
            })
            .collect(),
    };
    let image = Arc::new(WidgetImage::new(pic.clone(), None));
    let matte = Arc::new(WidgetImage::new(pic, Some([10, 20, 30])));
    let mut checked = 0;
    let mut kinds = [0usize; 10];
    for (scale, window, upscale) in [
        (1, (640, 480), false),
        (2, (1280, 720), false),
        (2, (1280, 720), true),
        (3, (1920, 800), true),
        (4, (2560, 1440), false),
    ] {
        let b = bank(&mut rng, scale, upscale);
        let mut layer = SpriteLayer {
            scale,
            ..SpriteLayer::default()
        };
        layer.fit_window(window.0, window.1);
        for p in [0, 1, 3] {
            layer.palettes[p] = Some(random_palette(&mut rng));
        }
        let mut cache = gpu::GpuCache::default();
        let mut target = None;
        for step in 0..60 {
            match rng.next(8) {
                0..=2 => {
                    let i = rng.next(16) as usize;
                    let map = rng.next(5) as i16 - 1;
                    let (x, y) = if map >= 2 && rng.next(2) == 0 {
                        (0, rng.next(200) as i16)
                    } else {
                        (rng.next(700) as i16 - 30, rng.next(500) as i16 - 10)
                    };
                    layer.slots[i] = Slot {
                        col: if rng.next(4) == 0 { 1 } else { 0 },
                        w: if rng.next(4) == 0 { 2 } else { 0 },
                        ..Slot::new(
                            map,
                            x,
                            y,
                            [0, 1, 3, 9][rng.next(4) as usize],
                            rng.next(4) as u8,
                            false,
                        )
                    };
                    layer.anchors[i] =
                        [Anchor::Left, Anchor::Centre, Anchor::Right][rng.next(3) as usize];
                }
                3 => {
                    // A fade: every colour scaled.
                    let k = rng.next(256);
                    let p = layer.palettes[rng.next(2) as usize * 3]
                        .as_mut()
                        .expect("set");
                    for c in p.0.iter_mut() {
                        let [b0, r, g, x] = c.to_le_bytes();
                        let f = |v: u8| (u32::from(v) * k / 255) as u8;
                        *c = u32::from_le_bytes([f(b0), f(r), f(g), x]);
                    }
                }
                4 => {
                    layer.widgets.clear();
                    for _ in 0..rng.next(5) {
                        let kind = random_widget(&mut rng, &image, &matte);
                        let rect = [
                            rng.next(640) as f32 - 20.0,
                            rng.next(480) as f32 - 10.0,
                            [20.5, 80.0, 160.0][rng.next(3) as usize],
                            [13.25, 16.0, 30.0][rng.next(3) as usize],
                        ];
                        let anchor =
                            [Anchor::Left, Anchor::Centre, Anchor::Right][rng.next(3) as usize];
                        let l = rng.next(4) as u8;
                        layer.anchored(anchor, |layer| layer.add_widget(l, rect, kind));
                    }
                }
                5 => {
                    layer.fans.clear();
                    if rng.next(2) == 0 {
                        let (x, y) = (rng.next(600) as i32, rng.next(440) as i32);
                        layer.add_fan(
                            &[[x, y], [x + 30, y + 3], [x + 10, y + 25], [x - 5, y + 12]],
                            0xff_20_40_60,
                        );
                    }
                }
                6 => {
                    layer.shake = (rng.next(5) as i8 - 2, rng.next(5) as i8 - 2);
                }
                _ => {
                    layer.framed = (0..rng.next(3)).map(|_| rng.next(16) as usize).collect();
                }
            }
            let backdrop = (rng.next(3) == 0).then_some([9, 80, 200]);
            let c = layer.full_canvas();
            let slots = layer.prepare(&b, c);
            let frame = layer.gpu_frame(&b, &slots, c, &mut cache, backdrop);
            let mut want = layer.new_canvas();
            layer.draw(&b, &slots, c, &mut want);
            if let Some(rgb) = backdrop {
                layer.put_backdrop(&mut want, rgb, c.clip);
            }
            for d in &frame.draws {
                kinds[d[0] as usize] += 1;
            }
            let got = render(&device, &queue, &mut gpu, &frame, &mut target);
            if let Some(i) = want.iter().zip(&got).position(|(a, b)| a != b) {
                let p = i / 4;
                panic!(
                    "scale {scale} step {step}: pixel ({}, {}): CPU {:?}, GPU {:?}",
                    p % c.w,
                    p / c.w,
                    &want[p * 4..p * 4 + 4],
                    &got[p * 4..p * 4 + 4]
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 300);
    // Every kind of draw was tested.
    assert!(
        kinds[1..].iter().all(|&n| n > 0),
        "draws by kind: {kinds:?}"
    );
    eprintln!("draws by kind: {kinds:?}");
}
