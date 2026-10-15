//! Headless rendering of a single frame to a PPM file, for automated visual checks.

use anyhow::{Context, Result};
use ddg_render::Renderer;

/// Renders the scene at `pos` (`width`×`height`) and writes a binary PPM (P6).
pub fn render_to_ppm(
    scene: &crate::scene::RouteScene,
    pos: ddg_formats::scene::TrackPos,
    (width, height): (u32, u32),
    configure: impl FnOnce(&mut Renderer),
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
    let mut renderer = Renderer::new(&device, format);
    configure(&mut renderer);
    renderer.upload_pages(&device, &queue, &scene.library.pages);
    let mut frame = ddg_render::FrameGeometry::default();
    scene.compose(&mut frame, pos, false);
    renderer.set_geometry(&device, &queue, &frame);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render(
        &device,
        &queue,
        &view,
        (width, height),
        &scene.cab_camera(pos),
    );

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
