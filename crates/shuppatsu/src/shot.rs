//! Headless rendering of a single frame to a PPM file, for automated visual checks.

use anyhow::{Context, Result};

use shuppatsu_render::Renderer;
use shuppatsu_render::overlay::UiScaling;

/// A composed sprite canvas (RGBA8) and its size in pixels.
pub type Canvas<'a> = (&'a [u8], (usize, usize));

/// How shots present the canvas (`--ui-scaling`, set once at startup by the subcommand, so
/// every shot path honours it without threading it through each caller).
static UI_SCALING: std::sync::Mutex<UiScaling> = std::sync::Mutex::new(UiScaling::Smooth);

/// Sets the presentation of the canvas in later shots.
pub fn set_ui_scaling(scaling: UiScaling) {
    *UI_SCALING.lock().unwrap_or_else(|e| e.into_inner()) = scaling;
}

fn ui_scaling() -> UiScaling {
    *UI_SCALING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Renders the scene at `pos` (`width`×`height`; black without a scene) with the canvas over
/// it and writes a binary PPM (P6).
pub fn render_to_ppm(
    scene: Option<(
        &crate::scene::RouteScene,
        shuppatsu_formats::scene::TrackPos,
    )>,
    (width, height): (u32, u32),
    configure: impl FnOnce(&mut Renderer),
    hud: Option<Canvas<'_>>,
    path: &std::path::Path,
) -> Result<()> {
    render_to_ppm_with(
        (width, height),
        |device, queue, view, format| match scene {
            Some((scene, pos)) => {
                let mut renderer = Renderer::new(device, format);
                configure(&mut renderer);
                renderer.upload_pages(device, queue, &scene.library.pages);
                let mut frame = shuppatsu_render::FrameGeometry::default();
                let band = (height as f32 * (1.0 - renderer.band_top)).max(1.0);
                scene.compose(&mut frame, pos, false, width as f32 / band);
                eprintln!(
                    "frame at engine index {}: {} triangles, {} missing parts",
                    pos.engine_index,
                    frame.triangle_count(),
                    frame.missing_parts
                );
                renderer.set_geometry(device, queue, &frame);
                renderer.render(device, queue, view, (width, height), &scene.cab_camera(pos));
            }
            None => clear_black(device, queue, view),
        },
        hud,
        path,
    )
}

/// Renders one frame with `draw` (device, queue, target view, target format) and the canvas
/// over it, and writes a binary PPM (P6).
pub fn render_to_ppm_with(
    (width, height): (u32, u32),
    draw: impl FnOnce(&wgpu::Device, &wgpu::Queue, &wgpu::TextureView, wgpu::TextureFormat),
    hud: Option<Canvas<'_>>,
    path: &std::path::Path,
) -> Result<()> {
    let (_instance, _, _adapter, device, queue) = crate::gpu_backend::open(
        wgpu::InstanceDescriptor::new_without_display_handle_from_env,
        |_| Ok(None),
    )?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shot"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    draw(&device, &queue, &view, format);
    if let Some((canvas, size)) = hud {
        let mut overlay = shuppatsu_render::overlay::Overlay::new(&device, format);
        overlay.resize(&device, size);
        overlay.upload(&queue, canvas);
        overlay.scaling = ui_scaling();
        overlay.render(&device, &queue, &view, overlay.rect(width, height));
    }

    // Rows must be 256-byte aligned for the copy.
    let row = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shot readback"),
        size: u64::from(row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback buffer"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .context("waiting for readback")?;
    let data = slice
        .get_mapped_range()
        .map_err(|e| anyhow::anyhow!("mapping readback buffer: {e:?}"))?;
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for y in 0..height {
        let start = (y * row) as usize;
        for px in data[start..start + width as usize * 4].as_chunks::<4>().0 {
            ppm.extend_from_slice(&px[..3]);
        }
    }
    std::fs::write(path, ppm).with_context(|| format!("writing {}", path.display()))
}

/// Clears `view` to black (the front-end screens have no 3D layer; the 640×480 canvas is drawn
/// over it with black bars either side).
pub fn clear_black(device: &wgpu::Device, queue: &wgpu::Queue, view: &wgpu::TextureView) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("clear"),
    });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    queue.submit([encoder.finish()]);
}

/// Whether shots also write the frame without the 2D layer and a text dump of the layer
/// (debug aid, env `SHUPPATSU_SHOT_LAYER=1`; read by `shuppatsu-tools upscale-sprites --frame`).
pub fn layer_dump_requested() -> bool {
    std::env::var_os("SHUPPATSU_SHOT_LAYER").is_some_and(|v| v != "0")
}

/// The sprite layer as text, in draw order: the canvas, every visible slot (layer 3 → 0, slot
/// high → low, with its anchor shift in canvas pixels), the palettes those slots use (256 hex
/// words, as stored: B, R, G) and the rect-layer fans. Widgets (translated text) are only
/// counted.
pub fn layer_dump(layer: &shuppatsu_render::sprites::SpriteLayer) -> String {
    use std::fmt::Write;
    let (w, h) = layer.canvas_size();
    let mut s = format!(
        "canvas {w} {h} scale {} wide {} shake {} {}\nwidgets {}\n",
        layer.scale,
        layer.wide,
        layer.shake.0,
        layer.shake.1,
        layer.widgets.len()
    );
    let mut used = Vec::new();
    for l in (0..4u8).rev() {
        for (i, slot) in layer.slots.iter().enumerate().rev() {
            if slot.layer != l || slot.tilemap < 0 {
                continue;
            }
            let shift = layer.anchor_shift(layer.anchors[i]);
            let _ = writeln!(
                s,
                "slot {i} tbl {} x {} y {} rect {} {} {} {} pal {} layer {l} shift {shift}",
                slot.tilemap, slot.x, slot.y, slot.col, slot.row, slot.w, slot.h, slot.palette
            );
            if !used.contains(&slot.palette) {
                used.push(slot.palette);
            }
        }
    }
    used.sort_unstable();
    for p in used {
        if let Some(Some(pal)) = layer.palettes.get(usize::from(p)) {
            let _ = write!(s, "palette {p}");
            for c in pal.0 {
                let _ = write!(s, " {c:08x}");
            }
            s.push('\n');
        }
    }
    for fan in &layer.fans {
        let _ = write!(
            s,
            "fan {} argb {:08x}",
            layer.anchor_shift(fan.anchor),
            fan.argb
        );
        for p in &fan.points[..usize::from(fan.count).min(4)] {
            let _ = write!(s, " {} {}", p[0], p[1]);
        }
        s.push('\n');
    }
    s
}

/// Writes [`layer_dump`] to `path`.
pub fn write_layer_dump(
    layer: &shuppatsu_render::sprites::SpriteLayer,
    path: &std::path::Path,
) -> Result<()> {
    std::fs::write(path, layer_dump(layer)).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shuppatsu_formats::cg::Palette;
    use shuppatsu_render::sprites::{Slot, SpriteLayer};

    #[test]
    fn layer_dump_lists_visible_slots_in_draw_order_with_their_palettes() {
        let mut layer = SpriteLayer::default();
        layer.slots[5] = Slot::new(0x54a, 16, 345, 4, 3, true);
        layer.slots[9] = Slot::new(0x60, 528, 113, 4, 2, true);
        layer.palettes[4] = Some(Palette([0x0012_3456; 256]));
        let d = layer_dump(&layer);
        let lines: Vec<&str> = d.lines().collect();
        assert_eq!(lines[0], "canvas 640 480 scale 1 wide 0 shake 0 0");
        // Layer 3 is drawn first.
        assert!(lines[2].starts_with("slot 5 tbl 1354 x 16 y 345 rect 0 0 0 0 pal 4 layer 3"));
        assert!(lines[3].starts_with("slot 9 tbl 96 "));
        assert!(lines[4].starts_with("palette 4 00123456 "));
        assert_eq!(lines.len(), 5);
    }
}
