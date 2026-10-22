//! Experimental checked-IR GPU execution. No native compute path is promoted by this module.
use revenant_core::{
    CancellationToken, Error, Result,
    compute::{Kernel, NumericBuffer, Output},
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use wgpu::util::DeviceExt;

/// Maximum resident passes per experiment; each buffer is separately bounded by the core IR.
pub const MAX_RESIDENT_PASSES: usize = 64;

/// CPU wall timings for preparation, enqueueing, completion and readback.
/// Upload/dispatch calls enqueue work: GPU execution time is included in synchronization.
#[derive(Clone, Copy, Debug, Default)]
pub struct ComputeTimings {
    /// Compiler extraction, adapter/device creation, pipeline compilation and validation.
    pub preparation_ms: f64,
    /// Buffer allocation and input/uniform upload enqueueing.
    pub upload_ms: f64,
    /// Command encoding and queue submission.
    pub dispatch_submit_ms: f64,
    /// Submission completion and map callback wait; includes queued transfers and GPU compute.
    pub synchronization_ms: f64,
    /// Mapped-byte copy and typed, finite-result decoding.
    pub readback_ms: f64,
    /// Complete run wall time; preparation is included only for the first run of this kernel.
    pub total_ms: f64,
}

/// Owned typed result and measured costs of a bounded GPU experiment.
pub struct GpuExecution {
    /// Typed output, checked through the same numeric buffer constructor as CPU output.
    pub output: NumericBuffer,
    /// Wall timing breakdown, including actual completion/readback.
    pub timings: ComputeTimings,
    /// Known allocated GPU buffer bytes; driver/pipeline allocations remain unknown.
    pub known_buffer_bytes: u64,
    /// Conservative transfer payload scratch bound, excluding containers/driver bookkeeping.
    /// Covers input encoding, mapped-byte copy and typed vector-to-Arc decoding overlap.
    pub transfer_scratch_bound_bytes: u64,
}

/// Pinned IR, shader, device and compiler for a headless experiment.
/// Each `run` keeps intermediate element buffers resident and reads the final result once.
pub struct GpuKernel {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    kernel: Kernel,
    adapter: String,
    preparation_ms: f64,
    first_run: bool,
    failed: bool,
    failure: Arc<Mutex<Option<String>>>,
    _instance: wgpu::Instance,
    // Outlives the pipeline, device and instance, including asynchronous backend references.
    #[cfg(windows)]
    _compiler: crate::compiler::BundledDxc,
}

fn error(message: impl std::fmt::Display) -> Error {
    Error::new("experimental_compute_gpu", message.to_string())
}

impl GpuKernel {
    /// Validates and compiles the generated shader on the available DX12/Vulkan adapter.
    /// Errors are explicit; callers may choose the core CPU executor as their fallback.
    pub async fn prepare(kernel: &Kernel) -> Result<Self> {
        let start = Instant::now();
        #[cfg(windows)]
        let compiler = crate::compiler::BundledDxc::prepare()?;
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = if cfg!(windows) {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::VULKAN
        };
        #[cfg(windows)]
        {
            descriptor.backend_options.dx12.shader_compiler = compiler.compiler();
        }
        let instance = wgpu::Instance::new(descriptor);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                ..Default::default()
            })
            .await
            .map_err(error)?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Revenant experimental compute"),
                ..Default::default()
            })
            .await
            .map_err(error)?;
        let failure = Arc::new(Mutex::new(None));
        let uncaptured = failure.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            *uncaptured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(e.to_string());
        }));
        let lost = failure.clone();
        device.set_device_lost_callback(move |reason, message| {
            *lost
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(format!("GPU device lost ({reason:?}): {message}"));
        });
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Checked Revenant compute IR"),
            source: wgpu::ShaderSource::Wgsl(kernel.wgsl().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Experimental Revenant compute"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(e) = validation.pop().await {
            return Err(error(e));
        }
        Ok(Self {
            device,
            queue,
            pipeline,
            kernel: kernel.clone(),
            adapter: adapter_name,
            preparation_ms: start.elapsed().as_secs_f64() * 1000.,
            first_run: true,
            failed: false,
            failure,
            _instance: instance,
            #[cfg(windows)]
            _compiler: compiler,
        })
    }

    /// Physical adapter used by the experiment.
    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// Performs at most 64 passes. Repeated passes require element output of the same type.
    /// Optional cancellation supports standalone measurements; task-owned callers supply a token.
    /// Already submitted work holds a cloned task token until submission completion.
    pub fn run(
        &mut self,
        input: &NumericBuffer,
        passes: usize,
        cancellation: Option<&CancellationToken>,
    ) -> Result<GpuExecution> {
        if self.failed {
            return Err(error(
                "GPU experiment previously failed; create a new prepared kernel",
            ));
        }
        if let Some(message) = self
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            self.failed = true;
            return Err(error(message));
        }
        if let Some(token) = cancellation {
            token.checkpoint()?;
        }
        if input.numeric_type() != self.kernel.numeric_type()
            || input.is_empty()
            || passes == 0
            || passes > MAX_RESIDENT_PASSES
            || (passes > 1 && self.kernel.output() != Output::Elements)
        {
            return Err(error(
                "GPU input must be nonempty, typed and bounded; resident passes require element output",
            ));
        }
        let dispatch = self.kernel.dispatch(input.len())?;
        let output_len = self.kernel.output_len(input.len())?;
        let total_start = Instant::now();
        let upload_start = Instant::now();
        let input_bytes = input.to_le_bytes();
        let input_size = input_bytes.len() as u64;
        let output_size = (output_len * 4) as u64;
        let a = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Bounded numeric input"),
                contents: &input_bytes,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            });
        let b = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bounded resident numeric output"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bounded numeric readback"),
            size: output_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut params = [0u8; 16];
        params[..4].copy_from_slice(&(input.len() as u32).to_le_bytes());
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Checked compute parameters"),
                contents: &params,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let layout = self.pipeline.get_bind_group_layout(0);
        let bind = |source: &wgpu::Buffer, target: &wgpu::Buffer| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Checked numeric bindings"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: source.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: target.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform.as_entire_binding(),
                    },
                ],
            })
        };
        let forward = bind(&a, &b);
        let reverse = if passes > 1 { Some(bind(&b, &a)) } else { None };
        let upload_ms = upload_start.elapsed().as_secs_f64() * 1000.;
        if let Some(token) = cancellation {
            token.checkpoint()?;
        }
        let submit_start = Instant::now();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Resident checked compute"),
            });
        for i in 0..passes {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Bounded element pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(
                0,
                if i % 2 == 0 {
                    &forward
                } else {
                    reverse.as_ref().expect("validated resident bindings")
                },
                &[],
            );
            pass.dispatch_workgroups(dispatch[0], dispatch[1], dispatch[2]);
        }
        encoder.copy_buffer_to_buffer(
            if passes.is_multiple_of(2) { &a } else { &b },
            0,
            &readback,
            0,
            output_size,
        );
        let submission = self.queue.submit([encoder.finish()]);
        if let Some(leases) = cancellation.cloned() {
            self.queue.on_submitted_work_done(move || drop(leases));
        }
        let dispatch_submit_ms = submit_start.elapsed().as_secs_f64() * 1000.;
        let sync_start = Instant::now();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = send.send(result);
            });
        if let Err(e) = self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(5)),
        }) {
            self.failed = true;
            return Err(error(e));
        }
        if let Err(e) = receive
            .recv_timeout(Duration::from_secs(1))
            .map_err(error)
            .and_then(|r| r.map_err(error))
        {
            self.failed = true;
            return Err(e);
        }
        let synchronization_ms = sync_start.elapsed().as_secs_f64() * 1000.;
        let read_start = Instant::now();
        let bytes = readback
            .slice(..)
            .get_mapped_range()
            .map_err(error)?
            .to_vec();
        readback.unmap();
        let output = NumericBuffer::from_le_bytes(input.numeric_type(), &bytes)?;
        let readback_ms = read_start.elapsed().as_secs_f64() * 1000.;
        if let Some(token) = cancellation {
            token.checkpoint()?;
        }
        let preparation_ms = if std::mem::replace(&mut self.first_run, false) {
            self.preparation_ms
        } else {
            0.
        };
        Ok(GpuExecution {
            output,
            known_buffer_bytes: input_size + output_size * 2 + 16,
            transfer_scratch_bound_bytes: input_size + output_size * 3,
            timings: ComputeTimings {
                preparation_ms,
                upload_ms,
                dispatch_submit_ms,
                synchronization_ms,
                readback_ms,
                total_ms: preparation_ms + total_start.elapsed().as_secs_f64() * 1000.,
            },
        })
    }
}
