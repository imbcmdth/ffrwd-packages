//! The GPU build's device half: `ffrwd:gpu@0.1.0` beside the node world.
//! `ports.memory` holds `v` on the sidecar's device when `memory` is `gpu`,
//! `frames.planes` hands a frame's planes as textures, and one compute pass
//! reduces the frame to the grid of cell averages `downsample` makes on the
//! CPU, in the same integer arithmetic. The grid's 1024 cells come back to
//! host memory; the frame does not. `wit-gpu/` is the sidecar's `wit-gpu/`.

mod bindings {
    // `generate_all`: the world's interfaces come from other packages,
    // ffrwd:gpu and wasi:webgpu, and without it bindgen expects them to
    // have been generated somewhere else. The path is the
    // `simple_detector` crate's, so the `simple-detector-gpu` crate reaching
    // this file finds it too.
    wit_bindgen::generate!({
        path: "../simple_detector/wit-gpu",
        world: "ffrwd:gpu/gpu-module@0.1.0",
        generate_all,
    });
}

use bindings::exports::ffrwd::gpu::ports::Guest;
use bindings::ffrwd::gpu::frames;
use bindings::ffrwd::gpu::types::{FrameMemory, PortMemory};
use bindings::wasi::webgpu::webgpu::{
    get_gpu, GpuBindGroupDescriptor, GpuBindGroupEntry, GpuBindGroupLayout,
    GpuBindGroupLayoutDescriptor, GpuBindGroupLayoutEntry, GpuBindingResource, GpuBuffer,
    GpuBufferBinding, GpuBufferBindingLayout, GpuBufferBindingType, GpuBufferDescriptor,
    GpuBufferUsage, GpuComputePipeline, GpuComputePipelineDescriptor, GpuDevice, GpuLayoutMode,
    GpuMapMode, GpuPipelineLayoutDescriptor, GpuProgrammableStage, GpuShaderModuleDescriptor,
    GpuShaderStage, GpuTextureBindingLayout, GpuTextureSampleType, GpuTextureViewDimension,
};

use crate::{held_ports, GRID, PASS};

struct Ports;

impl Guest for Ports {
    fn memory(params: String) -> Vec<PortMemory> {
        held_ports(&params)
            .iter()
            .map(|port| PortMemory {
                port: (*port).to_owned(),
                memory: FrameMemory::Gpu,
            })
            .collect()
    }
}

bindings::export!(Ports with_types_in bindings);

/// Bytes of the grid as the pass writes it: one u32 a cell.
const CELLS_LEN: u64 = (GRID * GRID * 4) as u64;

/// The pass and what it keeps between frames, all made once at `init`.
pub struct Grid {
    device: GpuDevice,
    layout: GpuBindGroupLayout,
    pipeline: GpuComputePipeline,
    params: GpuBuffer,
    cells: GpuBuffer,
    readback: GpuBuffer,
}

impl Grid {
    /// Opens the device and makes the pass for `width` x `height` frames,
    /// read as rgba where `rgba` says so and as their first plane, the
    /// luma, otherwise.
    pub fn open(width: u32, height: u32, rgba: bool) -> Result<Grid, String> {
        let device = get_gpu()
            .request_adapter(None)
            .ok_or("simple_detector: no GPU adapter")?
            .request_device(None)
            .map_err(|_| "simple_detector: the adapter gave no device".to_owned())?;
        let buffer = |size: u64, usage| {
            device.create_buffer(&GpuBufferDescriptor {
                size,
                usage,
                mapped_at_creation: None,
                label: None,
            })
        };
        let params = buffer(16, GpuBufferUsage::uniform() | GpuBufferUsage::copy_dst());
        let words: Vec<u8> = [width, height, u32::from(rgba), 0]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        device
            .queue()
            .write_buffer_with_copy(&params, 0, &words, None, None)
            .map_err(|_| "simple_detector: writing the pass's parameters".to_owned())?;
        let cells = buffer(
            CELLS_LEN,
            GpuBufferUsage::storage() | GpuBufferUsage::copy_src(),
        );
        let readback = buffer(
            CELLS_LEN,
            GpuBufferUsage::copy_dst() | GpuBufferUsage::map_read(),
        );
        let entry = |binding| GpuBindGroupLayoutEntry {
            binding,
            visibility: GpuShaderStage::compute(),
            buffer: None,
            sampler: None,
            texture: None,
            storage_texture: None,
        };
        let buffer_layout = |kind| {
            Some(GpuBufferBindingLayout {
                type_: Some(kind),
                has_dynamic_offset: None,
                min_binding_size: None,
            })
        };
        let layout = device.create_bind_group_layout(&GpuBindGroupLayoutDescriptor {
            entries: vec![
                GpuBindGroupLayoutEntry {
                    texture: Some(GpuTextureBindingLayout {
                        sample_type: Some(GpuTextureSampleType::UnfilterableFloat),
                        view_dimension: Some(GpuTextureViewDimension::D2),
                        multisampled: Some(false),
                    }),
                    ..entry(0)
                },
                GpuBindGroupLayoutEntry {
                    buffer: buffer_layout(GpuBufferBindingType::Uniform),
                    ..entry(1)
                },
                GpuBindGroupLayoutEntry {
                    buffer: buffer_layout(GpuBufferBindingType::Storage),
                    ..entry(2)
                },
            ],
            label: None,
        });
        let pipeline_layout = device.create_pipeline_layout(&GpuPipelineLayoutDescriptor {
            bind_group_layouts: vec![Some(&layout)],
            label: None,
        });
        let module = device.create_shader_module(&GpuShaderModuleDescriptor {
            code: PASS.to_owned(),
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
        Ok(Grid {
            device,
            layout,
            pipeline,
            params,
            cells,
            readback,
        })
    }

    /// Frame `index` of stream `v` this tick reduced to its grid, written
    /// into `into` one byte a cell, row by row.
    pub fn cells(&self, v: u32, index: u32, into: &mut Vec<u8>) -> Result<(), String> {
        let plane = frames::planes(v, index)
            .map_err(|e| format!("simple_detector: {e}"))?
            .into_iter()
            .next()
            .ok_or("simple_detector: a frame with no planes")?;
        let view = plane.create_view(None);
        let buffer = |buffer| {
            GpuBindingResource::GpuBufferBinding(GpuBufferBinding {
                buffer,
                offset: None,
                size: None,
            })
        };
        let group = self.device.create_bind_group(&GpuBindGroupDescriptor {
            layout: &self.layout,
            entries: vec![
                GpuBindGroupEntry {
                    binding: 0,
                    resource: GpuBindingResource::GpuTextureView(&view),
                },
                GpuBindGroupEntry {
                    binding: 1,
                    resource: buffer(&self.params),
                },
                GpuBindGroupEntry {
                    binding: 2,
                    resource: buffer(&self.cells),
                },
            ],
            label: None,
        });
        let encoder = self.device.create_command_encoder(None);
        let pass = encoder.begin_compute_pass(None);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, Some(&group), None, None, None)
            .map_err(|_| "simple_detector: binding the pass's resources".to_owned())?;
        pass.dispatch_workgroups(GRID as u32, Some(GRID as u32), None);
        pass.end();
        encoder.copy_buffer_to_buffer(&self.cells, 0, &self.readback, 0, CELLS_LEN);
        self.device.queue().submit(&[&encoder.finish(None)]);
        self.readback
            .map_async(GpuMapMode::read(), None, None)
            .map_err(|_| "simple_detector: mapping the grid".to_owned())?;
        let mapped = self
            .readback
            .get_mapped_range_get_with_copy(None, None)
            .map_err(|_| "simple_detector: reading the grid".to_owned());
        self.readback
            .unmap()
            .map_err(|_| "simple_detector: unmapping the grid".to_owned())?;
        let mapped = mapped?;
        into.clear();
        // Each cell is an average of bytes, so it fits one.
        into.extend(
            mapped
                .as_chunks::<4>()
                .0
                .iter()
                .map(|word| u32::from_le_bytes(*word) as u8),
        );
        Ok(())
    }
}
