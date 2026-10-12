//! The device's half: `ffrwd:gpu@0.1.0` beside the node world, and the
//! passes on raw `wasi:webgpu`. `ports.memory` says which ports are held on
//! the GPU, `frames.planes` hands a frame of one as a texture, and
//! `frames.emit` hands a texture back as a frame. `wit/` is the sidecar's
//! `wit-gpu/`.

mod bindings {
    // `generate_all`: the world's interfaces come from other packages,
    // ffrwd:gpu and wasi:webgpu, and without it bindgen expects them to
    // have been generated somewhere else.
    wit_bindgen::generate!({
        path: "wit",
        world: "ffrwd:gpu/gpu-module@0.1.0",
        generate_all,
    });
}

use bindings::exports::ffrwd::gpu::ports::Guest;
use bindings::ffrwd::gpu::frames;
use bindings::ffrwd::gpu::types::{FrameMemory, PortMemory};
use bindings::wasi::webgpu::webgpu::{
    get_gpu, GpuBindGroup, GpuBindGroupDescriptor, GpuBindGroupEntry, GpuBindGroupLayout,
    GpuBindGroupLayoutDescriptor, GpuBindGroupLayoutEntry, GpuBindingResource, GpuBuffer,
    GpuBufferBinding, GpuBufferBindingLayout, GpuBufferBindingType, GpuBufferDescriptor,
    GpuBufferUsage, GpuCommandEncoder, GpuComputePipeline, GpuComputePipelineDescriptor, GpuDevice,
    GpuExtent3D, GpuLayoutMode, GpuMapMode, GpuPipelineLayoutDescriptor, GpuProgrammableStage,
    GpuShaderModuleDescriptor, GpuShaderStage, GpuStorageTextureAccess,
    GpuStorageTextureBindingLayout, GpuTexelCopyBufferInfo, GpuTexelCopyBufferLayout,
    GpuTexelCopyTextureInfo, GpuTexture, GpuTextureBindingLayout, GpuTextureDescriptor,
    GpuTextureFormat, GpuTextureSampleType, GpuTextureUsage, GpuTextureView,
    GpuTextureViewDimension,
};

use crate::{shaders, Effect, Memory, Plan};

/// A frame made on the device, as `frames.emit` takes it.
pub type Texture = GpuTexture;

struct Ports;

impl Guest for Ports {
    fn memory(params: String) -> Vec<PortMemory> {
        crate::held_ports(&params)
            .iter()
            .map(|port| PortMemory {
                port: (*port).to_owned(),
                memory: FrameMemory::Gpu,
            })
            .collect()
    }
}

bindings::export!(Ports with_types_in bindings);

/// Hands `texture` to the host as the frame at `pts` on `port`.
pub fn emit(port: &str, pts: i64, duration: Option<i64>, texture: Texture) -> Result<(), String> {
    frames::emit(port, pts, duration, texture).map_err(|e| format!("masked_gpu: {e}"))
}

/// What a pass binds, in binding order.
#[derive(Clone, Copy)]
enum Slot {
    /// The frame, sampled.
    V,
    /// The matte, sampled.
    Matte,
    /// The `Params` uniform.
    Params,
    /// The blur's weights, read.
    Weights,
    /// The blur's rows pass, written.
    RowsOut,
    /// The blur's rows pass, sampled.
    RowsIn,
    /// The mosaic's row sums, written.
    SumsOut,
    /// The mosaic's row sums, read.
    SumsIn,
    /// The mosaic's block averages, written.
    BlocksOut,
    /// The mosaic's block averages, read.
    BlocksIn,
    /// The composite, written.
    Dst,
}

/// How many invocations a pass is dispatched over, 8 by 8 a workgroup.
#[derive(Clone, Copy)]
enum Over {
    /// One a pixel.
    Pixels,
    /// One a block column and a row.
    BlockRows,
    /// One a block.
    Blocks,
}

/// One pass: its WGSL, what it binds, and what it runs over.
struct PassSpec {
    source: &'static str,
    slots: &'static [Slot],
    over: Over,
}

const BLUR: [PassSpec; 2] = [
    PassSpec {
        source: shaders::BLUR_ROWS,
        slots: &[Slot::V, Slot::Params, Slot::Weights, Slot::RowsOut],
        over: Over::Pixels,
    },
    PassSpec {
        source: shaders::BLUR_COLUMNS,
        slots: &[
            Slot::V,
            Slot::Matte,
            Slot::Params,
            Slot::Weights,
            Slot::RowsIn,
            Slot::Dst,
        ],
        over: Over::Pixels,
    },
];

const MOSAIC: [PassSpec; 3] = [
    PassSpec {
        source: shaders::MOSAIC_ROWS,
        slots: &[Slot::V, Slot::Params, Slot::SumsOut],
        over: Over::BlockRows,
    },
    PassSpec {
        source: shaders::MOSAIC_BLOCKS,
        slots: &[Slot::Params, Slot::SumsIn, Slot::BlocksOut],
        over: Over::Blocks,
    },
    PassSpec {
        source: shaders::MOSAIC_COMPOSITE,
        slots: &[
            Slot::V,
            Slot::Matte,
            Slot::Params,
            Slot::BlocksIn,
            Slot::Dst,
        ],
        over: Over::Pixels,
    },
];

struct Pass {
    spec: &'static PassSpec,
    layout: GpuBindGroupLayout,
    pipeline: GpuComputePipeline,
    /// Workgroups across and down.
    groups: (u32, u32),
    /// Made once where the pass binds no frame of its own.
    fixed: Option<GpuBindGroup>,
}

/// The host-memory side: the textures frames are uploaded into, the one
/// the composite is written to, the buffer it is read back through, and
/// the passes' bind groups over them, all made once.
struct Upload {
    v: GpuTexture,
    matte: GpuTexture,
    dst: GpuTexture,
    readback: GpuBuffer,
    /// A row of the readback, padded to the 256 bytes a copy's rows align
    /// to.
    padded_row: u32,
    groups: Vec<GpuBindGroup>,
}

/// What the passes keep between frames.
struct Kept {
    params: GpuBuffer,
    weights: Option<GpuBuffer>,
    /// The blur's rows pass, as `rgba32float`.
    rows: Option<GpuTextureView>,
    sums: Option<GpuBuffer>,
    blocks: Option<GpuBuffer>,
}

pub struct Device {
    device: GpuDevice,
    width: u32,
    height: u32,
    kept: Kept,
    passes: Vec<Pass>,
    upload: Option<Upload>,
}

fn extent(width: u32, height: u32) -> GpuExtent3D {
    GpuExtent3D {
        width,
        height: Some(height),
        depth_or_array_layers: None,
    }
}

fn whole(texture: &GpuTexture) -> GpuTexelCopyTextureInfo<'_> {
    GpuTexelCopyTextureInfo {
        texture,
        mip_level: None,
        origin: None,
        aspect: None,
    }
}

fn layout_entry(binding: u32, slot: Slot) -> GpuBindGroupLayoutEntry {
    let entry = GpuBindGroupLayoutEntry {
        binding,
        visibility: GpuShaderStage::compute(),
        buffer: None,
        sampler: None,
        texture: None,
        storage_texture: None,
    };
    let sampled = Some(GpuTextureBindingLayout {
        sample_type: Some(GpuTextureSampleType::UnfilterableFloat),
        view_dimension: Some(GpuTextureViewDimension::D2),
        multisampled: Some(false),
    });
    let buffer = |kind| {
        Some(GpuBufferBindingLayout {
            type_: Some(kind),
            has_dynamic_offset: None,
            min_binding_size: None,
        })
    };
    let written = |format| {
        Some(GpuStorageTextureBindingLayout {
            access: Some(GpuStorageTextureAccess::WriteOnly),
            format,
            view_dimension: Some(GpuTextureViewDimension::D2),
        })
    };
    match slot {
        Slot::V | Slot::Matte | Slot::RowsIn => GpuBindGroupLayoutEntry {
            texture: sampled,
            ..entry
        },
        Slot::Params => GpuBindGroupLayoutEntry {
            buffer: buffer(GpuBufferBindingType::Uniform),
            ..entry
        },
        Slot::Weights | Slot::SumsIn | Slot::BlocksIn => GpuBindGroupLayoutEntry {
            buffer: buffer(GpuBufferBindingType::ReadOnlyStorage),
            ..entry
        },
        Slot::SumsOut | Slot::BlocksOut => GpuBindGroupLayoutEntry {
            buffer: buffer(GpuBufferBindingType::Storage),
            ..entry
        },
        Slot::RowsOut => GpuBindGroupLayoutEntry {
            storage_texture: written(GpuTextureFormat::Rgba32float),
            ..entry
        },
        Slot::Dst => GpuBindGroupLayoutEntry {
            storage_texture: written(GpuTextureFormat::Rgba8unorm),
            ..entry
        },
    }
}

/// Whether a pass binds a frame: those are bound again for every frame on
/// the device.
fn binds_a_frame(spec: &PassSpec) -> bool {
    spec.slots
        .iter()
        .any(|slot| matches!(slot, Slot::V | Slot::Matte | Slot::Dst))
}

impl Device {
    /// Opens the device and makes everything the passes need for frames of
    /// the plan's size.
    pub fn open(plan: &Plan, memory: Memory) -> Result<Device, String> {
        let device = get_gpu()
            .request_adapter(None)
            .ok_or("masked_gpu: no GPU adapter")?
            .request_device(None)
            .map_err(|_| "masked_gpu: the adapter gave no device".to_owned())?;
        let (width, height) = (plan.width, plan.height);
        let (blocks_x, blocks_y) = plan.blocks();

        let buffer = |size: u64, usage| {
            device.create_buffer(&GpuBufferDescriptor {
                size,
                usage,
                mapped_at_creation: None,
                label: None,
            })
        };
        let queue = device.queue();
        let params = buffer(32, GpuBufferUsage::uniform() | GpuBufferUsage::copy_dst());
        queue
            .write_buffer_with_copy(&params, 0, &plan.uniform(), None, None)
            .map_err(|_| "masked_gpu: writing the passes' parameters".to_owned())?;
        let mut kept = Kept {
            params,
            weights: None,
            rows: None,
            sums: None,
            blocks: None,
        };
        let specs: &'static [PassSpec] = match plan.effect {
            Effect::Blur => {
                let bytes: Vec<u8> = plan.weights.iter().flat_map(|w| w.to_le_bytes()).collect();
                let weights = buffer(
                    bytes.len() as u64,
                    GpuBufferUsage::storage() | GpuBufferUsage::copy_dst(),
                );
                queue
                    .write_buffer_with_copy(&weights, 0, &bytes, None, None)
                    .map_err(|_| "masked_gpu: writing the blur's weights".to_owned())?;
                kept.weights = Some(weights);
                let rows = device.create_texture(&GpuTextureDescriptor {
                    size: extent(width, height),
                    mip_level_count: None,
                    sample_count: None,
                    dimension: None,
                    format: GpuTextureFormat::Rgba32float,
                    usage: GpuTextureUsage::storage_binding() | GpuTextureUsage::texture_binding(),
                    view_formats: None,
                    label: None,
                });
                kept.rows = Some(rows.create_view(None));
                &BLUR
            }
            Effect::Mosaic => {
                let cell = 16u64;
                kept.sums = Some(buffer(
                    cell * u64::from(blocks_x) * u64::from(height),
                    GpuBufferUsage::storage(),
                ));
                kept.blocks = Some(buffer(
                    cell * u64::from(blocks_x) * u64::from(blocks_y),
                    GpuBufferUsage::storage(),
                ));
                &MOSAIC
            }
        };

        let mut passes = Vec::with_capacity(specs.len());
        for spec in specs {
            let layout = device.create_bind_group_layout(&GpuBindGroupLayoutDescriptor {
                entries: spec
                    .slots
                    .iter()
                    .enumerate()
                    .map(|(binding, &slot)| layout_entry(binding as u32, slot))
                    .collect(),
                label: None,
            });
            let pipeline_layout = device.create_pipeline_layout(&GpuPipelineLayoutDescriptor {
                bind_group_layouts: vec![Some(&layout)],
                label: None,
            });
            let module = device.create_shader_module(&GpuShaderModuleDescriptor {
                code: shaders::source(spec.source),
                compilation_hints: None,
                label: None,
            });
            let pipeline = device.create_compute_pipeline(GpuComputePipelineDescriptor {
                compute: GpuProgrammableStage {
                    module: &module,
                    entry_point: Some("main".to_owned()),
                    constants: None,
                },
                layout: GpuLayoutMode::Specific(&pipeline_layout),
                label: None,
            });
            let (across, down) = match spec.over {
                Over::Pixels => (width, height),
                Over::BlockRows => (blocks_x, height),
                Over::Blocks => (blocks_x, blocks_y),
            };
            passes.push(Pass {
                spec,
                layout,
                pipeline,
                groups: (across.div_ceil(8), down.div_ceil(8)),
                fixed: None,
            });
        }

        let mut opened = Device {
            device,
            width,
            height,
            kept,
            passes,
            upload: None,
        };
        for at in 0..opened.passes.len() {
            if !binds_a_frame(opened.passes[at].spec) {
                let group = opened.bind(&opened.passes[at], None)?;
                opened.passes[at].fixed = Some(group);
            }
        }
        if memory == Memory::Host {
            opened.upload = Some(opened.open_upload()?);
        }
        Ok(opened)
    }

    /// The host side's textures and readback, and the bind groups over
    /// them.
    fn open_upload(&self) -> Result<Upload, String> {
        let texture = |usage| {
            self.device.create_texture(&GpuTextureDescriptor {
                size: extent(self.width, self.height),
                mip_level_count: None,
                sample_count: None,
                dimension: None,
                format: GpuTextureFormat::Rgba8unorm,
                usage,
                view_formats: None,
                label: None,
            })
        };
        let upload = GpuTextureUsage::texture_binding() | GpuTextureUsage::copy_dst();
        let (v, matte) = (texture(upload), texture(upload));
        let dst = texture(GpuTextureUsage::storage_binding() | GpuTextureUsage::copy_src());
        let padded_row = (self.width * 4).div_ceil(256) * 256;
        let readback = self.device.create_buffer(&GpuBufferDescriptor {
            size: u64::from(padded_row) * u64::from(self.height),
            usage: GpuBufferUsage::copy_dst() | GpuBufferUsage::map_read(),
            mapped_at_creation: None,
            label: None,
        });
        let views = (
            v.create_view(None),
            matte.create_view(None),
            dst.create_view(None),
        );
        let groups = self.frame_groups(&views.0, &views.1, &views.2)?;
        Ok(Upload {
            v,
            matte,
            dst,
            readback,
            padded_row,
            groups,
        })
    }

    /// A bind group of `pass`, over `frame`'s views (`v`, the matte, the
    /// composite's target) where it binds them.
    fn bind(
        &self,
        pass: &Pass,
        frame: Option<(&GpuTextureView, &GpuTextureView, &GpuTextureView)>,
    ) -> Result<GpuBindGroup, String> {
        let missing = || "masked_gpu: a pass binds what its effect did not make".to_owned();
        let buffer = |buffer| {
            GpuBindingResource::GpuBufferBinding(GpuBufferBinding {
                buffer,
                offset: None,
                size: None,
            })
        };
        let mut entries = Vec::with_capacity(pass.spec.slots.len());
        for (binding, slot) in pass.spec.slots.iter().enumerate() {
            let resource = match slot {
                Slot::V => GpuBindingResource::GpuTextureView(frame.ok_or_else(missing)?.0),
                Slot::Matte => GpuBindingResource::GpuTextureView(frame.ok_or_else(missing)?.1),
                Slot::Dst => GpuBindingResource::GpuTextureView(frame.ok_or_else(missing)?.2),
                Slot::Params => buffer(&self.kept.params),
                Slot::Weights => buffer(self.kept.weights.as_ref().ok_or_else(missing)?),
                Slot::RowsOut | Slot::RowsIn => {
                    GpuBindingResource::GpuTextureView(self.kept.rows.as_ref().ok_or_else(missing)?)
                }
                Slot::SumsOut | Slot::SumsIn => {
                    buffer(self.kept.sums.as_ref().ok_or_else(missing)?)
                }
                Slot::BlocksOut | Slot::BlocksIn => {
                    buffer(self.kept.blocks.as_ref().ok_or_else(missing)?)
                }
            };
            entries.push(GpuBindGroupEntry {
                binding: binding as u32,
                resource,
            });
        }
        Ok(self.device.create_bind_group(&GpuBindGroupDescriptor {
            layout: &pass.layout,
            entries,
            label: None,
        }))
    }

    /// The bind groups of the passes that bind a frame, in pass order.
    fn frame_groups(
        &self,
        v: &GpuTextureView,
        matte: &GpuTextureView,
        dst: &GpuTextureView,
    ) -> Result<Vec<GpuBindGroup>, String> {
        self.passes
            .iter()
            .filter(|pass| binds_a_frame(pass.spec))
            .map(|pass| self.bind(pass, Some((v, matte, dst))))
            .collect()
    }

    /// Every pass in order, each with its fixed bind group or the next of
    /// `frame`'s.
    fn encode(&self, frame: &[GpuBindGroup]) -> Result<GpuCommandEncoder, String> {
        let encoder = self.device.create_command_encoder(None);
        let mut frame = frame.iter();
        for pass in &self.passes {
            let group = match &pass.fixed {
                Some(group) => group,
                None => frame
                    .next()
                    .ok_or("masked_gpu: a pass without its bind group")?,
            };
            let compute = encoder.begin_compute_pass(None);
            compute.set_pipeline(&pass.pipeline);
            compute
                .set_bind_group(0, Some(group), None, None, None)
                .map_err(|_| "masked_gpu: binding a pass's resources".to_owned())?;
            compute.dispatch_workgroups(pass.groups.0, Some(pass.groups.1), None);
            compute.end();
        }
        Ok(encoder)
    }

    /// One frame held on the device: frame `v.1` of stream `v.0` and frame
    /// `matte.1` of stream `matte.0` composited into a new texture, the
    /// frame that leaves.
    pub fn on_device(&self, v: (u32, u32), matte: (u32, u32)) -> Result<Texture, String> {
        let plane = |(id, index): (u32, u32)| {
            frames::planes(id, index)
                .map_err(|e| format!("masked_gpu: {e}"))?
                .into_iter()
                .next()
                .ok_or_else(|| "masked_gpu: an rgba frame has one plane".to_owned())
        };
        let (v, matte) = (plane(v)?, plane(matte)?);
        let dst = self.device.create_texture(&GpuTextureDescriptor {
            size: extent(self.width, self.height),
            mip_level_count: None,
            sample_count: None,
            dimension: None,
            format: GpuTextureFormat::Rgba8unorm,
            usage: GpuTextureUsage::storage_binding()
                | GpuTextureUsage::texture_binding()
                | GpuTextureUsage::copy_src(),
            view_formats: None,
            label: None,
        });
        let groups = self.frame_groups(
            &v.create_view(None),
            &matte.create_view(None),
            &dst.create_view(None),
        )?;
        let encoder = self.encode(&groups)?;
        self.device.queue().submit(&[&encoder.finish(None)]);
        Ok(dst)
    }

    /// One frame in host memory: `v` and `matte`, rgba bytes, uploaded,
    /// composited, and read back as rgba bytes.
    pub fn on_host(&self, v: &[u8], matte: &[u8]) -> Result<Vec<u8>, String> {
        let upload = self
            .upload
            .as_ref()
            .ok_or("masked_gpu: memory=host opens its textures at init")?;
        let (width, height) = (self.width, self.height);
        let bytes = (width * height * 4) as usize;
        for (name, frame) in [("v", v), ("the matte", matte)] {
            if frame.len() != bytes {
                return Err(format!(
                    "masked_gpu: a frame of {name} is {} bytes, and {width}x{height} rgba is {bytes}",
                    frame.len()
                ));
            }
        }
        let queue = self.device.queue();
        for (texture, frame) in [(&upload.v, v), (&upload.matte, matte)] {
            queue.write_texture_with_copy(
                &whole(texture),
                frame,
                GpuTexelCopyBufferLayout {
                    offset: None,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                extent(width, height),
            );
        }
        let encoder = self.encode(&upload.groups)?;
        encoder.copy_texture_to_buffer(
            &whole(&upload.dst),
            &GpuTexelCopyBufferInfo {
                buffer: &upload.readback,
                offset: None,
                bytes_per_row: Some(upload.padded_row),
                rows_per_image: Some(height),
            },
            extent(width, height),
        );
        queue.submit(&[&encoder.finish(None)]);
        upload
            .readback
            .map_async(GpuMapMode::read(), None, None)
            .map_err(|_| "masked_gpu: mapping the readback".to_owned())?;
        let mapped = upload
            .readback
            .get_mapped_range_get_with_copy(None, None)
            .map_err(|_| "masked_gpu: reading the readback".to_owned());
        upload
            .readback
            .unmap()
            .map_err(|_| "masked_gpu: unmapping the readback".to_owned())?;
        let mapped = mapped?;
        let row = (width * 4) as usize;
        let mut out = Vec::with_capacity(bytes);
        for line in mapped
            .chunks(upload.padded_row as usize)
            .take(height as usize)
        {
            out.extend_from_slice(&line[..row]);
        }
        Ok(out)
    }
}
