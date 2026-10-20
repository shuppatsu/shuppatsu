//! Runs a [`GpuFrame`] (sprites/gpu.rs) on the GPU: one compute dispatch writes every pixel of
//! the overlay's canvas texture (docs/performance.md, "GPU 2D compositing").
//!
//! Data: the bank's tiles and tilemaps (uploaded once per art id), the 128 palettes (only the
//! changed ones are written), a pool of blobs (upscaled images, rasters, paper fills, text
//! parts; uploaded once and kept while they are used), and per frame the draw records with a
//! list of the draws reaching each 64 × 64 bin of the canvas.

use crate::sprites::PALETTE_COUNT;
use crate::sprites::gpu::{Blob, DRAW_WORDS, GpuFrame};
use shuppatsu_formats::cg::Palette;

/// Bins are `1 << BIN_SHIFT` pixels square (the shader has the same).
const BIN_SHIFT: u32 = 6;
/// Workgroup edge (the shader's `@workgroup_size`).
const GROUP: u32 = 16;
const GLOBALS_SIZE: u64 = 64;
const PALETTE_BYTES: u64 = 256 * 4;

/// The device can run the compositor: compute shaders, enough storage buffers, and texture
/// views of another format (the sRGB view of the canvas; not on GL).
pub fn supported(device: &wgpu::Device) -> bool {
    if device.adapter_info().backend == wgpu::Backend::Gl {
        return false;
    }
    let l = device.limits();
    l.max_compute_invocations_per_workgroup >= GROUP * GROUP
        && l.max_compute_workgroup_size_x >= GROUP
        && l.max_compute_workgroup_size_y >= GROUP
        && l.max_storage_buffers_per_shader_stage >= 4
        && l.max_storage_textures_per_shader_stage >= 1
}

struct Resident {
    id: (usize, usize),
    /// Kept so the data (and its address, the id) stays alive while resident.
    _blob: Blob,
    /// First word in the pool.
    word: u32,
    frame: u64,
}

pub struct Compositor {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    bank: Option<(u64, wgpu::Buffer)>,
    palettes: wgpu::Buffer,
    uploaded: Vec<Option<Palette>>,
    pool: wgpu::Buffer,
    /// Bytes used from the pool's start.
    top: u64,
    resident: Vec<Resident>,
    frame_buf: wgpu::Buffer,
    frames: u64,
    /// What each bin of the last composed canvas was composed from (a hash), and the
    /// canvas size: a bin whose inputs are the same is not composed again.
    bins: Vec<u64>,
    bins_size: (u32, u32),
}

fn storage(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16).next_multiple_of(4),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Writes `bytes` at `offset`, padding the end to a whole word.
fn write_padded(queue: &wgpu::Queue, buffer: &wgpu::Buffer, offset: u64, bytes: &[u8]) {
    let whole = bytes.len() / 4 * 4;
    if whole > 0 {
        queue.write_buffer(buffer, offset, &bytes[..whole]);
    }
    if whole < bytes.len() {
        let mut tail = [0u8; 4];
        tail[..bytes.len() - whole].copy_from_slice(&bytes[whole..]);
        queue.write_buffer(buffer, offset + whole as u64, &tail);
    }
}

impl Compositor {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("2d compose shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu2d.wgsl").into()),
        });
        let buffer = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let read = wgpu::BufferBindingType::Storage { read_only: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("2d compose"),
            entries: &[
                buffer(0, wgpu::BufferBindingType::Uniform),
                buffer(1, read),
                buffer(2, read),
                buffer(3, read),
                buffer(4, read),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("2d compose"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("2d compose"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("2d compose globals"),
            size: GLOBALS_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            layout,
            globals,
            bank: None,
            palettes: storage(device, "2d palettes", PALETTE_COUNT as u64 * PALETTE_BYTES),
            uploaded: vec![None; PALETTE_COUNT],
            pool: storage(device, "2d blobs", 16 << 20),
            top: 0,
            resident: Vec::new(),
            frame_buf: storage(device, "2d frame", 64 << 10),
            frames: 0,
            bins: Vec::new(),
            bins_size: (0, 0),
        }
    }

    /// Composes `frame` into `target` (a storage view of an `Rgba8Unorm` texture of the
    /// frame's size). With `keep`, the target holds this compositor's last canvas, and only
    /// the bins whose inputs changed are composed again. Returns false when the frame's data
    /// does not fit the device's limits.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &GpuFrame,
        target: &wgpu::TextureView,
        keep: bool,
    ) -> bool {
        self.frames += 1;
        let mut uploaded = 0u64;
        // The bank's art.
        if self.bank.as_ref().is_none_or(|(id, _)| *id != frame.art.id) {
            let bytes: &[u8] = bytemuck::cast_slice(&frame.art.words);
            let buf = storage(device, "2d bank", bytes.len() as u64);
            queue.write_buffer(&buf, 0, bytes);
            uploaded += bytes.len() as u64;
            self.bank = Some((frame.art.id, buf));
        }
        // Changed palettes.
        for (i, (old, new)) in self.uploaded.iter_mut().zip(&frame.palettes).enumerate() {
            if let Some(p) = new
                && old.as_ref() != Some(p)
            {
                queue.write_buffer(
                    &self.palettes,
                    i as u64 * PALETTE_BYTES,
                    bytemuck::cast_slice(&p.0),
                );
                uploaded += PALETTE_BYTES;
                old.clone_from(new);
            }
        }
        // The blobs: where each one is in the pool.
        let Some((words, up)) = self.place(device, queue, &frame.blobs) else {
            return false;
        };
        uploaded += up;
        let mut draws = frame.draws.clone();
        for &(d, k, b) in &frame.patches {
            draws[d as usize][usize::from(k)] = words[b as usize];
        }
        // Bins: the draws reaching each, in order.
        let (cw, ch) = frame.size;
        let bins_x = cw.div_ceil(1 << BIN_SHIFT);
        let bins_y = ch.div_ceil(1 << BIN_SHIFT);
        let nbins = (bins_x * bins_y) as usize;
        let range = |d: &[u32; DRAW_WORDS]| {
            let last = |v: u32| (v.max(1) - 1) >> BIN_SHIFT;
            (
                (d[1] >> BIN_SHIFT)..=last(d[3]).min(bins_x - 1),
                (d[2] >> BIN_SHIFT)..=last(d[4]).min(bins_y - 1),
            )
        };
        let mut counts = vec![0u32; nbins + 1];
        for d in &draws {
            let (xs, ys) = range(d);
            for by in ys {
                for bx in xs.clone() {
                    counts[(by * bins_x + bx) as usize + 1] += 1;
                }
            }
        }
        for i in 1..=nbins {
            counts[i] += counts[i - 1];
        }
        let total = counts[nbins] as usize;
        let mut entries = vec![0u32; total];
        let mut next = counts.clone();
        for (n, d) in draws.iter().enumerate() {
            let (xs, ys) = range(d);
            for by in ys {
                for bx in xs.clone() {
                    let k = &mut next[(by * bins_x + bx) as usize];
                    entries[*k as usize] = n as u32;
                    *k += 1;
                }
            }
        }
        // The bins to compose: those whose inputs changed.
        let hashes = bin_hashes(frame, &draws, &counts, &entries);
        let keep = keep && self.bins_size == frame.size && self.bins.len() == hashes.len();
        let dirty: Vec<u32> = (0..nbins as u32)
            .filter(|&i| !keep || self.bins[i as usize] != hashes[i as usize])
            .collect();
        self.bins = hashes;
        self.bins_size = frame.size;
        if dirty.is_empty() {
            crate::perf::count(crate::perf::Counter::Upload2dBytes, uploaded);
            return true;
        }
        let bins_off = (draws.len() * DRAW_WORDS) as u32;
        let entries_off = bins_off + counts.len() as u32;
        let dirty_off = entries_off + total as u32;
        let mut words_out: Vec<u32> = Vec::with_capacity(dirty_off as usize + dirty.len());
        words_out.extend(draws.iter().flatten());
        words_out.extend(&counts);
        words_out.extend(&entries);
        words_out.extend(&dirty);
        let bytes: &[u8] = bytemuck::cast_slice(&words_out);
        if bytes.len() as u64 > self.frame_buf.size() {
            if bytes.len() as u64 > device.limits().max_storage_buffer_binding_size {
                return false;
            }
            self.frame_buf = storage(device, "2d frame", (bytes.len() as u64).next_power_of_two());
        }
        queue.write_buffer(&self.frame_buf, 0, bytes);
        uploaded += bytes.len() as u64;
        let [bd_on, bd_rgb, bd_x0, bd_x1, bd_y1] = match frame.backdrop {
            Some([rgb, x0, x1, y1]) => [1, rgb, x0, x1, y1],
            None => [0; 5],
        };
        let globals: [u32; 16] = [
            cw,
            ch,
            frame.scale,
            bins_x,
            frame.art.tiles,
            0,
            bd_on,
            bd_rgb,
            bd_x0,
            bd_x1,
            bd_y1,
            bins_off,
            entries_off,
            dirty_off,
            0,
            0,
        ];
        queue.write_buffer(&self.globals, 0, bytemuck::cast_slice(&globals));
        crate::perf::count(crate::perf::Counter::Upload2dBytes, uploaded);
        let bank = &self.bank.as_ref().expect("uploaded above").1;
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("2d compose"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: bank.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.palettes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.pool.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.frame_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(target),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("2d compose"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("2d compose"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            // Each bin is (64 / 16)² workgroups; one row of them per dirty bin.
            let per = (1 << BIN_SHIFT) / GROUP;
            pass.dispatch_workgroups(per * per, dirty.len() as u32, 1);
        }
        queue.submit([encoder.finish()]);
        true
    }

    /// Makes every blob resident in the pool: the first word of each, and the bytes uploaded.
    /// When the pool is full, it is emptied (and grown if this frame alone needs more) and
    /// this frame's blobs are uploaded again.
    fn place(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blobs: &[Blob],
    ) -> Option<(Vec<u32>, u64)> {
        let size = |b: &Blob| (b.bytes().len() as u64).next_multiple_of(4).max(4);
        let missing: u64 = blobs
            .iter()
            .filter(|b| !self.resident.iter().any(|r| r.id == b.id()))
            .map(size)
            .sum();
        if self.top + missing > self.pool.size() {
            // Start again with only this frame's blobs.
            let need: u64 = blobs.iter().map(size).sum();
            let limit = device
                .limits()
                .max_storage_buffer_binding_size
                .min(device.limits().max_buffer_size);
            if need > limit {
                return None;
            }
            if need * 2 > self.pool.size() {
                self.pool = storage(device, "2d blobs", (need * 2).min(limit));
            }
            self.resident.clear();
            self.top = 0;
            self.bins.clear();
        }
        let mut uploaded = 0;
        let mut words = Vec::with_capacity(blobs.len());
        for b in blobs {
            let id = b.id();
            let word = match self.resident.iter_mut().find(|r| r.id == id) {
                Some(r) => {
                    r.frame = self.frames;
                    r.word
                }
                None => {
                    let bytes = b.bytes();
                    write_padded(queue, &self.pool, self.top, bytes);
                    uploaded += bytes.len() as u64;
                    let word = (self.top / 4) as u32;
                    self.top += size(b);
                    self.resident.push(Resident {
                        id,
                        _blob: b.clone(),
                        word,
                        frame: self.frames,
                    });
                    word
                }
            };
            words.push(word);
        }
        // Blobs unused for a while give their memory back at the next reset; drop the
        // references now so the data can be freed.
        let frames = self.frames;
        if self.resident.len() > 4096 {
            self.resident.retain(|r| frames - r.frame < 600);
            self.bins.clear();
        }
        Some((words, uploaded))
    }
}

/// What each bin's pixels are made from: the size and global inputs, then every record
/// reaching it in order, with the identity of the data it reads and the palette it colours
/// with. Equal hashes mean equal pixels (barring a 64-bit collision).
fn bin_hashes(
    frame: &GpuFrame,
    draws: &[[u32; DRAW_WORDS]],
    counts: &[u32],
    entries: &[u32],
) -> Vec<u64> {
    use crate::sprites::gpu::kind;
    use std::hash::{Hash, Hasher};
    let hasher = std::collections::hash_map::DefaultHasher::new;
    let mut palettes: Vec<Option<u64>> = vec![None; frame.palettes.len()];
    let mut palette = |p: u32| -> u64 {
        let p = p as usize;
        if palettes.get(p).is_some_and(Option::is_none) {
            let mut h = hasher();
            frame.palettes[p].as_ref().map(|q| q.0).hash(&mut h);
            palettes[p] = Some(h.finish());
        }
        palettes.get(p).copied().flatten().unwrap_or(0)
    };
    let mut blobs_of: Vec<Vec<(usize, usize)>> = vec![Vec::new(); draws.len()];
    for &(d, _, b) in &frame.patches {
        blobs_of[d as usize].push(frame.blobs[b as usize].id());
    }
    let per_draw: Vec<u64> = draws
        .iter()
        .zip(&blobs_of)
        .map(|(d, blobs)| {
            let mut h = hasher();
            d.hash(&mut h);
            blobs.hash(&mut h);
            match d[0] {
                kind::SLOT => palette(d[9 + 12]).hash(&mut h),
                kind::PART => palette(d[9 + 7]).hash(&mut h),
                _ => {}
            }
            h.finish()
        })
        .collect();
    let mut base = hasher();
    (frame.size, frame.scale, frame.backdrop, frame.art.id).hash(&mut base);
    let base = base.finish();
    counts
        .windows(2)
        .map(|w| {
            let mut h = hasher();
            base.hash(&mut h);
            for &e in &entries[w[0] as usize..w[1] as usize] {
                per_draw[e as usize].hash(&mut h);
            }
            h.finish()
        })
        .collect()
}
