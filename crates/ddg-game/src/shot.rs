//! Headless rendering of a single frame to a PPM file, for automated visual checks.

use anyhow::{Context, Result};
use ddg_render::Renderer;

/// Renders the scene at `pos` (`width`×`height`; black without a scene) with the canvas over
/// it and writes a binary PPM (P6).
pub fn render_to_ppm(
    scene: Option<(&crate::scene::RouteScene, ddg_formats::scene::TrackPos)>,
    (width, height): (u32, u32),
    configure: impl FnOnce(&mut Renderer),
    hud: Option<&[u8]>,
    path: &std::path::Path,
) -> Result<()> {
    render_to_ppm_with(
        (width, height),
        |device, queue, view, format| match scene {
            Some((scene, pos)) => {
                let mut renderer = Renderer::new(device, format);
                configure(&mut renderer);
                renderer.upload_pages(device, queue, &scene.library.pages);
                let mut frame = ddg_render::FrameGeometry::default();
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
    hud: Option<&[u8]>,
    path: &std::path::Path,
) -> Result<()> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = crate::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))?;
    let (device, queue) =
        crate::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
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
    if let Some(canvas) = hud {
        let overlay = ddg_render::overlay::Overlay::new(&device, format);
        overlay.upload(&queue, canvas);
        overlay.render(
            &device,
            &queue,
            &view,
            ddg_render::overlay::canvas_rect(width, height),
        );
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
