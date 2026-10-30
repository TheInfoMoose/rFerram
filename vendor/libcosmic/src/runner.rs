//! `winit` (`0.30.13`) `ApplicationHandler` & `wgpu` (`30.0.1`) Surface Presentation Runner
//!
//! Implements [`cosmic::app::run`](crate::app::run) and [`CosmicAppRunner`]:
//! - Opens a native desktop window via `winit` (`0.30.13`) `ApplicationHandler`
//! - Lays out [`Application::view`](crate::Application::view) via [`crate::layout`]
//! - Rasterizes the [`LayoutScene`](crate::layout::LayoutScene) into an RGBA8 [`tiny_skia::Pixmap`] via [`crate::raster`]
//! - Packs the RGBA8 framebuffer to `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (`256`-byte) row alignment and
//!   uploads/presents it on a `wgpu` (`30.0.1`) [`wgpu::Surface`] (falling back gracefully when surface
//!   acquisition is `Occluded`, `Outdated`, `Lost`, `Timeout`, or `Validation`)
//! - Dispatches pointer clicks, text input typing, dropdown selection, scrolling, and resize events
//!   to [`Application::update`](crate::Application::update)
//! - Executes returned [`Task<A::Message>`](crate::app::Task) futures asynchronously and forwards
//!   completed messages via [`winit::event_loop::EventLoopProxy`]

use std::borrow::Cow;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::app::{Core, Settings, Task};
use crate::layout::{LayoutBox, LayoutScene};
use crate::raster::{GLYPH_ADVANCE, Pixmap, rasterize_scene};
use crate::{Application, WidgetNode};

/// Mandatory `wgpu` row pitch alignment in bytes (`256`) for `Queue::write_texture`.
pub const WGPU_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// Default vertical scroll step in logical pixels per mouse wheel line tick.
pub const SCROLL_PIXELS_PER_LINE: f32 = 36.0;

/// Built-in WGSL shader for presenting the rasterized RGBA8 UI framebuffer onto a `wgpu::Surface`.
pub const RUNNER_SURFACE_SHADER_WGSL: &str = r#"
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32((in_vertex_index << 1u) & 2u);
    let y = f32(in_vertex_index & 2u);
    out.clip_position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.tex_coords = vec2<f32>(x, y);
    return out;
}

@group(0) @binding(0)
var frame_tex: texture_2d<f32>;
@group(0) @binding(1)
var frame_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(frame_tex, frame_sampler, in.tex_coords);
}
"#;

/// Aligns an unpadded row byte stride up to the nearest multiple of [`WGPU_ROW_ALIGNMENT`] (`256`).
pub const fn align_bytes_per_row(unpadded_bytes_per_row: u32) -> u32 {
    let align = WGPU_ROW_ALIGNMENT;
    let rem = unpadded_bytes_per_row % align;
    if rem == 0 {
        unpadded_bytes_per_row
    } else {
        unpadded_bytes_per_row.saturating_add(align - rem)
    }
}

/// Packs a tightly-packed RGBA8 framebuffer (`width * height * 4` bytes) into a `256`-byte
/// row-aligned buffer suitable for `wgpu::Queue::write_texture`.
///
/// Returns `(aligned_buffer, aligned_bytes_per_row)`.
pub fn pack_rgba8_to_wgpu_alignment(rgba_data: &[u8], width: u32, height: u32) -> (Vec<u8>, u32) {
    let safe_w = width.max(1);
    let safe_h = height.max(1);
    let unpadded_row = (safe_w * 4) as usize;
    let aligned_row_u32 = align_bytes_per_row(safe_w * 4);
    let aligned_row = aligned_row_u32 as usize;
    let total_expected = unpadded_row * (safe_h as usize);

    if unpadded_row == aligned_row && rgba_data.len() >= total_expected {
        return (rgba_data[..total_expected].to_vec(), aligned_row_u32);
    }

    let mut aligned = vec![0u8; aligned_row * (safe_h as usize)];
    for row in 0..(safe_h as usize) {
        let src_start = row * unpadded_row;
        if src_start >= rgba_data.len() {
            break;
        }
        let copy_len = unpadded_row.min(rgba_data.len() - src_start);
        let dst_start = row * aligned_row;
        aligned[dst_start..dst_start + copy_len]
            .copy_from_slice(&rgba_data[src_start..src_start + copy_len]);
    }

    (aligned, aligned_row_u32)
}

/// Outcome of acquiring and presenting a frame on a `wgpu::Surface`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfacePresentStatus {
    /// Frame was rendered and presented cleanly.
    Presented,
    /// Frame was rendered and presented on a suboptimal swapchain, which was then reconfigured.
    PresentedSuboptimal,
    /// Surface is occluded (e.g. minimized or hidden); frame presentation was gracefully skipped.
    SkippedOccluded,
    /// Surface acquisition timed out; frame presentation was gracefully skipped.
    SkippedTimeout,
    /// Surface configuration was outdated; surface was reconfigured and frame skipped.
    SkippedOutdated,
    /// Surface was lost; surface was reconfigured and frame skipped.
    SkippedLost,
    /// Surface reported a validation status; frame presentation was gracefully skipped.
    SkippedValidation,
}

impl SurfacePresentStatus {
    /// Returns `true` if a frame was presented to the swapchain (`Presented` or `PresentedSuboptimal`).
    pub const fn was_presented(self) -> bool {
        matches!(self, Self::Presented | Self::PresentedSuboptimal)
    }

    /// Returns `true` if surface acquisition required reconfiguring the `wgpu::Surface`.
    pub const fn needs_reconfigure(self) -> bool {
        matches!(
            self,
            Self::PresentedSuboptimal | Self::SkippedOutdated | Self::SkippedLost
        )
    }
}

/// Maps a [`wgpu::CurrentSurfaceTexture`] variant into an optional [`wgpu::SurfaceTexture`]
/// and its corresponding [`SurfacePresentStatus`], allowing graceful fallback when occluded/outdated.
pub fn classify_surface_acquisition(
    current: wgpu::CurrentSurfaceTexture,
) -> (Option<wgpu::SurfaceTexture>, SurfacePresentStatus) {
    match current {
        wgpu::CurrentSurfaceTexture::Success(frame) => {
            (Some(frame), SurfacePresentStatus::Presented)
        }
        wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
            (Some(frame), SurfacePresentStatus::PresentedSuboptimal)
        }
        wgpu::CurrentSurfaceTexture::Occluded => (None, SurfacePresentStatus::SkippedOccluded),
        wgpu::CurrentSurfaceTexture::Timeout => (None, SurfacePresentStatus::SkippedTimeout),
        wgpu::CurrentSurfaceTexture::Outdated => (None, SurfacePresentStatus::SkippedOutdated),
        wgpu::CurrentSurfaceTexture::Lost => (None, SurfacePresentStatus::SkippedLost),
        wgpu::CurrentSurfaceTexture::Validation => (None, SurfacePresentStatus::SkippedValidation),
    }
}

/// `wgpu` (`30.0.1`) RGBA8 Texture Uploader and Full-Screen Quad Presenter.
pub struct WgpuFramePresenter {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub render_pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
    pub sampler: wgpu::Sampler,
    pub frame_texture: wgpu::Texture,
    pub width: u32,
    pub height: u32,
    pub target_format: wgpu::TextureFormat,
}

impl WgpuFramePresenter {
    /// Creates a new [`WgpuFramePresenter`] targeting `target_format` at `(width, height)`.
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u32,
        height: u32,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        let safe_w = width.max(1);
        let safe_h = height.max(1);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cosmic_runner_surface_shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(RUNNER_SURFACE_SHADER_WGSL)),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cosmic_runner_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cosmic_runner_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cosmic_runner_render_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cosmic_runner_frame_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let (frame_texture, bind_group) = Self::create_texture_and_bind_group(
            &device,
            &bind_group_layout,
            &sampler,
            safe_w,
            safe_h,
        );

        Self {
            device,
            queue,
            render_pipeline,
            bind_group_layout,
            bind_group,
            sampler,
            frame_texture,
            width: safe_w,
            height: safe_h,
            target_format,
        }
    }

    fn create_texture_and_bind_group(
        device: &wgpu::Device,
        bind_group_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::BindGroup) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cosmic_runner_rgba8_frame_texture"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cosmic_runner_frame_bind_group"),
            layout: bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        (texture, bind_group)
    }

    /// Recreates the backing RGBA8 GPU texture and bind group when the viewport dimensions change.
    pub fn resize(&mut self, width: u32, height: u32) {
        let safe_w = width.max(1);
        let safe_h = height.max(1);
        if self.width == safe_w && self.height == safe_h {
            return;
        }
        let (texture, bind_group) = Self::create_texture_and_bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.sampler,
            safe_w,
            safe_h,
        );
        self.frame_texture = texture;
        self.bind_group = bind_group;
        self.width = safe_w;
        self.height = safe_h;
    }

    /// Packs `rgba_data` to `256`-byte row alignment and uploads it to `self.frame_texture`
    /// via `wgpu::Queue::write_texture`, returning the aligned row stride in bytes.
    pub fn upload_aligned_rgba8(&mut self, rgba_data: &[u8], width: u32, height: u32) -> u32 {
        let safe_w = width.max(1);
        let safe_h = height.max(1);
        self.resize(safe_w, safe_h);

        let (aligned_buf, aligned_bytes_per_row) =
            pack_rgba8_to_wgpu_alignment(rgba_data, safe_w, safe_h);

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.frame_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &aligned_buf,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(aligned_bytes_per_row),
                rows_per_image: Some(safe_h),
            },
            wgpu::Extent3d {
                width: safe_w,
                height: safe_h,
                depth_or_array_layers: 1,
            },
        );

        aligned_bytes_per_row
    }

    /// Renders the uploaded RGBA8 texture into `target_view` using the full-screen WGSL pipeline.
    pub fn render_to_view(&self, target_view: &wgpu::TextureView) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("cosmic_runner_render_encoder"),
            });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cosmic_runner_surface_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
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
            rpass.set_pipeline(&self.render_pipeline);
            rpass.set_bind_group(0, &self.bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Uploads the `256`-byte row-aligned RGBA8 framebuffer and presents it onto `surface`,
    /// falling back gracefully if `surface.get_current_texture()` reports `Occluded`, `Outdated`,
    /// `Lost`, `Timeout`, or `Validation`.
    pub fn present_to_surface(
        &mut self,
        surface: &wgpu::Surface<'_>,
        surface_config: &wgpu::SurfaceConfiguration,
        rgba_data: &[u8],
        width: u32,
        height: u32,
    ) -> SurfacePresentStatus {
        let _ = self.upload_aligned_rgba8(rgba_data, width, height);

        let (mut maybe_frame, mut status) =
            classify_surface_acquisition(surface.get_current_texture());
        if maybe_frame.is_none()
            && status.needs_reconfigure()
            && surface_config.width > 0
            && surface_config.height > 0
        {
            surface.configure(&self.device, surface_config);
            let (retry_frame, retry_status) =
                classify_surface_acquisition(surface.get_current_texture());
            if retry_frame.is_some() {
                maybe_frame = retry_frame;
                status = retry_status;
            }
        }

        let Some(surface_texture) = maybe_frame else {
            return status;
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.render_to_view(&view);
        self.queue.present(surface_texture);

        if status.needs_reconfigure() && surface_config.width > 0 && surface_config.height > 0 {
            surface.configure(&self.device, surface_config);
        }

        status
    }
}

/// Active native window and `wgpu::Surface` state managed by [`CosmicAppRunner`].
pub struct WgpuWindowSurface {
    pub window: Arc<Window>,
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub surface: wgpu::Surface<'static>,
    pub surface_config: wgpu::SurfaceConfiguration,
    pub presenter: WgpuFramePresenter,
}

impl WgpuWindowSurface {
    /// Creates a [`WgpuWindowSurface`] bound to `window` with initial dimensions `(width, height)`.
    pub fn new(window: Arc<Window>, width: u32, height: u32) -> Result<Self, String> {
        let safe_w = width.max(1);
        let safe_h = height.max(1);

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(|e| format!("Failed to create wgpu surface: {e}"))?;

        let adapter = block_on_future(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .map_err(|e| format!("Failed to request compatible wgpu adapter: {e}"))?;

        let (device, queue) =
            block_on_future(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| format!("Failed to request wgpu device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| caps.formats.first().copied())
            .unwrap_or(wgpu::TextureFormat::Bgra8UnormSrgb);

        let present_mode = caps
            .present_modes
            .first()
            .copied()
            .unwrap_or(wgpu::PresentMode::Fifo);

        let alpha_mode = caps
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);

        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if caps.usages.contains(wgpu::TextureUsages::COPY_DST) {
            usage |= wgpu::TextureUsages::COPY_DST;
        }

        let surface_config = wgpu::SurfaceConfiguration {
            usage,
            format,
            width: safe_w,
            height: safe_h,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: Vec::new(),
            color_space: wgpu::SurfaceColorSpace::Auto,
        };

        surface.configure(&device, &surface_config);

        let presenter = WgpuFramePresenter::new(device, queue, safe_w, safe_h, format);

        Ok(Self {
            window,
            instance,
            adapter,
            surface,
            surface_config,
            presenter,
        })
    }

    /// Reconfigures the `wgpu::Surface` and internal RGBA8 texture for a new physical window size.
    pub fn resize(&mut self, width: u32, height: u32) {
        let safe_w = width.max(1);
        let safe_h = height.max(1);
        self.surface_config.width = safe_w;
        self.surface_config.height = safe_h;
        self.surface
            .configure(&self.presenter.device, &self.surface_config);
        self.presenter.resize(safe_w, safe_h);
    }

    /// Uploads `rgba_data` (packed to 256-byte row alignment) and presents it to the window surface.
    pub fn present_rgba8(
        &mut self,
        rgba_data: &[u8],
        width: u32,
        height: u32,
    ) -> SurfacePresentStatus {
        let safe_w = width.max(1);
        let safe_h = height.max(1);
        if self.surface_config.width != safe_w || self.surface_config.height != safe_h {
            self.resize(safe_w, safe_h);
        }
        self.presenter.present_to_surface(
            &self.surface,
            &self.surface_config,
            rgba_data,
            safe_w,
            safe_h,
        )
    }
}

struct ThreadParkWaker(std::thread::Thread);

impl Wake for ThreadParkWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls a future to completion on the current thread without requiring an ambient async runtime.
fn block_on_future<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let waker = Waker::from(Arc::new(ThreadParkWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(val) => return val,
            Poll::Pending => std::thread::park_timeout(Duration::from_millis(5)),
        }
    }
}

/// Executes a boxed [`Task`] future on a background thread (with a `tokio` runtime context enabled
/// for timer/channel futures) and invokes `on_complete` if a message `M` is produced.
fn spawn_task_future<M, F>(
    fut: Pin<Box<dyn Future<Output = Option<M>> + Send + 'static>>,
    on_complete: F,
) where
    M: Send + 'static,
    F: FnOnce(M) + Send + 'static,
{
    std::thread::spawn(move || {
        #[cfg(feature = "tokio")]
        let output = if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            rt.block_on(fut)
        } else {
            block_on_future(fut)
        };

        #[cfg(not(feature = "tokio"))]
        let output = block_on_future(fut);

        if let Some(msg) = output {
            on_complete(msg);
        }
    });
}

/// Stateful `winit` (`0.30.13`) `ApplicationHandler` + `wgpu` (`30.0.1`) surface runner for a [`cosmic::Application`].
pub struct CosmicAppRunner<A: Application> {
    pub app: A,
    pub settings: Settings,
    pub cursor_position: (f32, f32),
    pub focused_widget_id: Option<usize>,
    pub hovered_widget_id: Option<usize>,
    pub caret_position: Option<usize>,
    pub scroll_offset_y: f32,
    pub needs_redraw: bool,
    pub last_present_status: Option<SurfacePresentStatus>,
    event_loop_proxy: Option<EventLoopProxy<A::Message>>,
    pending_task_messages: Arc<Mutex<VecDeque<A::Message>>>,
    in_flight_tasks: Arc<Mutex<usize>>,
    gpu_surface: Option<WgpuWindowSurface>,
}

impl<A: Application> CosmicAppRunner<A> {
    /// Creates a new [`CosmicAppRunner`], initializing `A::init(core, flags)` and spawning any initial [`Task`] futures.
    pub fn new(
        settings: Settings,
        flags: A::Flags,
        event_loop_proxy: Option<EventLoopProxy<A::Message>>,
    ) -> Self {
        let core = Core {
            window_width: settings.width.max(1),
            window_height: settings.height.max(1),
            scale_factor: settings.scale_factor.max(0.5),
            nav_bar_active: true,
            show_context_drawer: false,
            theme: settings.theme.clone(),
        };

        let (app, init_task) = A::init(core, flags);
        let mut runner = Self {
            app,
            settings,
            cursor_position: (0.0, 0.0),
            focused_widget_id: None,
            hovered_widget_id: None,
            caret_position: None,
            scroll_offset_y: 0.0,
            needs_redraw: true,
            last_present_status: None,
            event_loop_proxy,
            pending_task_messages: Arc::new(Mutex::new(VecDeque::new())),
            in_flight_tasks: Arc::new(Mutex::new(0)),
            gpu_surface: None,
        };
        runner.spawn_task(init_task);
        runner
    }

    /// Creates a headless [`CosmicAppRunner`] for deterministic UI layout, rasterization, and event testing.
    pub fn headless(settings: Settings, flags: A::Flags) -> Self {
        Self::new(settings, flags, None)
    }

    /// Returns the current viewport dimensions `(width, height)` in pixels (`>= 1x1`).
    pub fn viewport_size(&self) -> (u32, u32) {
        let core = self.app.core();
        let w = if core.window_width > 0 {
            core.window_width
        } else {
            self.settings.width.max(1)
        };
        let h = if core.window_height > 0 {
            core.window_height
        } else {
            self.settings.height.max(1)
        };
        (w.max(1), h.max(1))
    }

    /// Lays out `self.app.view()` at the current viewport dimensions via [`crate::layout`],
    /// applying focus, hover, caret, and scroll state.
    pub fn layout_scene(&self) -> LayoutScene<A::Message> {
        let (w, h) = self.viewport_size();
        let view = self.app.view();
        LayoutScene::from_element(&view, w as f32, h as f32)
            .with_focused_widget(self.focused_widget_id)
            .with_hovered_widget(self.hovered_widget_id)
            .with_caret_position(self.caret_position)
            .with_scroll_offset(self.scroll_offset_y)
    }

    /// Rasterizes `self.app.view()` into an RGBA8 [`tiny_skia::Pixmap`] via [`crate::raster`].
    pub fn rasterize_frame(&self) -> Pixmap {
        let scene = self.layout_scene();
        rasterize_scene(&scene, &self.app.core().theme)
    }

    /// Rasterizes `self.app.view()` and packs the RGBA8 pixels into a `256`-byte row-aligned buffer.
    ///
    /// Returns `(aligned_rgba_bytes, width, height, aligned_bytes_per_row)`.
    pub fn pack_aligned_frame(&self) -> (Vec<u8>, u32, u32, u32) {
        let pixmap = self.rasterize_frame();
        let width = pixmap.width();
        let height = pixmap.height();
        let (aligned, stride) = pack_rgba8_to_wgpu_alignment(pixmap.data(), width, height);
        (aligned, width, height, stride)
    }

    /// Dispatches `message` to `self.app.update(message)` and executes any returned [`Task`] futures.
    pub fn dispatch_message(&mut self, message: A::Message) {
        let task = self.app.update(message);
        self.needs_redraw = true;
        self.spawn_task(task);
    }

    /// Spawns all futures inside `task`, routing completed `A::Message` outputs through
    /// `EventLoopProxy::send_event` (when attached to a `winit` event loop) or the internal
    /// pending message queue.
    pub fn spawn_task(&mut self, task: Task<A::Message>) {
        let futures = task.into_futures();
        if futures.is_empty() {
            return;
        }

        for fut in futures {
            if let Ok(mut guard) = self.in_flight_tasks.lock() {
                *guard += 1;
            }
            let proxy = self.event_loop_proxy.clone();
            let queue = Arc::clone(&self.pending_task_messages);
            let in_flight = Arc::clone(&self.in_flight_tasks);

            spawn_task_future(fut, move |msg| {
                let sent_via_proxy = proxy
                    .as_ref()
                    .is_some_and(|p| p.send_event(msg.clone()).is_ok());
                if !sent_via_proxy && let Ok(mut q) = queue.lock() {
                    q.push_back(msg);
                }
                if let Ok(mut guard) = in_flight.lock() {
                    *guard = guard.saturating_sub(1);
                }
            });
        }
    }

    /// Drains and dispatches any completed background [`Task`] messages queued in `pending_task_messages`.
    /// Returns the number of messages processed.
    pub fn drain_pending_tasks(&mut self) -> usize {
        let mut processed = 0usize;
        loop {
            let next_msg = self
                .pending_task_messages
                .lock()
                .ok()
                .and_then(|mut q| q.pop_front());
            let Some(msg) = next_msg else {
                break;
            };
            self.dispatch_message(msg);
            processed += 1;
        }
        processed
    }

    /// Waits up to `timeout` for all currently in-flight [`Task`] futures to finish and dispatches
    /// their resulting messages to `A::update()`. Returns the total number of messages processed.
    pub fn wait_and_drain_tasks(&mut self, timeout: Duration) -> usize {
        let start = Instant::now();
        let mut total_processed = 0usize;
        loop {
            total_processed += self.drain_pending_tasks();
            let active = self.in_flight_tasks.lock().map_or(0, |g| *g);
            let queued = self.pending_task_messages.lock().map_or(0, |q| q.len());
            if active == 0 && queued == 0 {
                break;
            }
            if start.elapsed() >= timeout {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        total_processed += self.drain_pending_tasks();
        total_processed
    }

    /// Updates the cursor position and hovered interactive widget ID (`hovered_widget_id`).
    /// Returns `true` if the hovered widget changed.
    pub fn handle_cursor_moved(&mut self, x: f32, y: f32) -> bool {
        self.cursor_position = (x, y);
        let scene = self.layout_scene();
        let new_hover = scene.hit_test_widget_id(x, y);
        if self.hovered_widget_id != new_hover {
            self.hovered_widget_id = new_hover;
            self.needs_redraw = true;
            return true;
        }
        false
    }

    /// Handles a primary pointer click at `(x, y)`:
    /// - Dispatches `Button` (`on_press`) and `Toggler` (`on_toggle`) messages to `A::update()`
    /// - Focuses a clicked `TextInput` and positions the caret at the clicked character column
    /// - Advances and dispatches selection on a clicked `Dropdown`
    pub fn handle_pointer_click(&mut self, x: f32, y: f32) -> bool {
        self.cursor_position = (x, y);
        let scene = self.layout_scene();
        let Some(hit) = scene.hit_test(x, y) else {
            if self.focused_widget_id.take().is_some() | self.caret_position.take().is_some() {
                self.needs_redraw = true;
            }
            return false;
        };

        match &hit.node {
            WidgetNode::Button { .. } | WidgetNode::Toggler { .. } => {
                self.focused_widget_id = None;
                self.caret_position = None;
                if let Some(msg) = hit.message {
                    self.dispatch_message(msg);
                    return true;
                }
                false
            }
            WidgetNode::TextInput { value, .. } => {
                self.focused_widget_id = Some(hit.widget_id);
                let rel_x = (x - (hit.bounds.x + 8.0)).max(0.0);
                let char_idx =
                    ((rel_x / (GLYPH_ADVANCE as f32)).round() as usize).min(value.chars().count());
                self.caret_position = Some(char_idx);
                self.needs_redraw = true;
                true
            }
            WidgetNode::Dropdown {
                options, selected, ..
            } => {
                self.focused_widget_id = None;
                self.caret_position = None;
                if options.is_empty() {
                    return false;
                }
                let next_idx = selected.map_or(0, |idx| (idx + 1) % options.len());
                self.handle_dropdown_select(hit.widget_id, next_idx)
            }
            _ => {
                self.focused_widget_id = None;
                self.caret_position = None;
                if let Some(msg) = hit.message {
                    self.dispatch_message(msg);
                    return true;
                }
                false
            }
        }
    }

    /// Selects `selected_index` on the `Dropdown` identified by `widget_id` (or the topmost
    /// `Dropdown` in the scene if `widget_id` is not found) and dispatches the resulting message
    /// to `A::update()`.
    pub fn handle_dropdown_select(&mut self, widget_id: usize, selected_index: usize) -> bool {
        let scene = self.layout_scene();
        let top_z = scene.boxes.iter().map(|b| b.z_index).max().unwrap_or(0);
        let target_box = scene
            .boxes
            .iter()
            .find(|b| {
                b.id == widget_id
                    && b.z_index == top_z
                    && matches!(b.node, WidgetNode::Dropdown { .. })
            })
            .or_else(|| {
                let dd_top_z = scene
                    .boxes
                    .iter()
                    .filter(|b| matches!(b.node, WidgetNode::Dropdown { .. }))
                    .map(|b| b.z_index)
                    .max()?;
                scene.boxes.iter().find(|b| {
                    b.z_index == dd_top_z && matches!(b.node, WidgetNode::Dropdown { .. })
                })
            });

        let Some(lbox) = target_box else {
            return false;
        };

        let WidgetNode::Dropdown {
            options, on_select, ..
        } = &lbox.node
        else {
            return false;
        };

        if options.is_empty() {
            return false;
        }

        let clamped_idx = selected_index.min(options.len() - 1);
        let box_id = lbox.id;
        let selected_label = options[clamped_idx].clone();
        let widget_msg = on_select.as_ref().map(|cb| (cb.0)(clamped_idx));

        let mut dispatched = false;
        if let Some(msg) = widget_msg {
            self.dispatch_message(msg);
            dispatched = true;
        }
        if let Some(hook_msg) = self
            .app
            .on_dropdown_select(box_id, clamped_idx, &selected_label)
        {
            self.dispatch_message(hook_msg);
            dispatched = true;
        }

        self.needs_redraw = true;
        dispatched
    }

    fn resolve_focused_text_input<'a>(
        scene: &'a LayoutScene<A::Message>,
        focused_id: Option<usize>,
    ) -> Option<&'a LayoutBox<A::Message>> {
        let top_z = scene
            .boxes
            .iter()
            .filter(|b| matches!(b.node, WidgetNode::TextInput { .. }))
            .map(|b| b.z_index)
            .max()?;

        if let Some(fid) = focused_id
            && let Some(found) = scene.boxes.iter().find(|b| {
                b.id == fid && b.z_index == top_z && matches!(b.node, WidgetNode::TextInput { .. })
            })
        {
            return Some(found);
        }

        scene
            .boxes
            .iter()
            .find(|b| {
                b.z_index == top_z
                    && matches!(&b.node, WidgetNode::TextInput { value, .. } if !value.is_empty())
            })
            .or_else(|| {
                scene
                    .boxes
                    .iter()
                    .find(|b| b.z_index == top_z && matches!(b.node, WidgetNode::TextInput { .. }))
            })
    }

    /// Inserts printable characters into the currently focused `TextInput` at `caret_position`
    /// and dispatches the updated text value to `A::update()`.
    pub fn handle_text_input(&mut self, inserted_text: &str) -> bool {
        let printable: String = inserted_text.chars().filter(|c| !c.is_control()).collect();
        if printable.is_empty() {
            return false;
        }

        let scene = self.layout_scene();
        let Some(lbox) = Self::resolve_focused_text_input(&scene, self.focused_widget_id) else {
            return false;
        };

        let WidgetNode::TextInput {
            placeholder,
            value,
            on_input,
        } = &lbox.node
        else {
            return false;
        };

        let mut chars: Vec<char> = value.chars().collect();
        let caret = self.caret_position.unwrap_or(chars.len()).min(chars.len());
        let insert_chars: Vec<char> = printable.chars().collect();
        let inserted_len = insert_chars.len();
        chars.splice(caret..caret, insert_chars);

        let new_value: String = chars.into_iter().collect();
        let box_id = lbox.id;
        let placeholder_str = placeholder.clone();
        let widget_msg = on_input.as_ref().map(|cb| (cb.0)(new_value.clone()));

        self.focused_widget_id = Some(box_id);
        self.caret_position = Some(caret + inserted_len);
        self.needs_redraw = true;

        if let Some(msg) = widget_msg {
            self.dispatch_message(msg);
        }
        if let Some(hook_msg) = self.app.on_text_input(box_id, &placeholder_str, new_value) {
            self.dispatch_message(hook_msg);
        }

        true
    }

    /// Processes a keyboard event (`Backspace`, `Delete`, `ArrowLeft`, `ArrowRight`, `Home`, `End`,
    /// `Tab`, or printable character input) against the focused `TextInput`.
    pub fn handle_key_event(
        &mut self,
        logical_key: &Key,
        text: Option<&str>,
        is_pressed: bool,
    ) -> bool {
        if !is_pressed {
            return false;
        }

        let scene = self.layout_scene();

        if let Key::Named(named) = logical_key {
            match named {
                NamedKey::Tab => {
                    let top_z = scene
                        .boxes
                        .iter()
                        .filter(|b| matches!(b.node, WidgetNode::TextInput { .. }))
                        .map(|b| b.z_index)
                        .max();
                    if let Some(z) = top_z {
                        let inputs: Vec<&LayoutBox<A::Message>> = scene
                            .boxes
                            .iter()
                            .filter(|b| {
                                b.z_index == z && matches!(b.node, WidgetNode::TextInput { .. })
                            })
                            .collect();
                        if !inputs.is_empty() {
                            let next_box = match self
                                .focused_widget_id
                                .and_then(|fid| inputs.iter().position(|b| b.id == fid))
                            {
                                Some(cur_pos) => inputs[(cur_pos + 1) % inputs.len()],
                                None => inputs[0],
                            };
                            self.focused_widget_id = Some(next_box.id);
                            if let WidgetNode::TextInput { value, .. } = &next_box.node {
                                self.caret_position = Some(value.chars().count());
                            }
                            self.needs_redraw = true;
                            return true;
                        }
                    }
                    return false;
                }
                NamedKey::Backspace
                | NamedKey::Delete
                | NamedKey::ArrowLeft
                | NamedKey::ArrowRight
                | NamedKey::Home
                | NamedKey::End => {
                    let Some(lbox) =
                        Self::resolve_focused_text_input(&scene, self.focused_widget_id)
                    else {
                        return false;
                    };
                    let WidgetNode::TextInput {
                        placeholder,
                        value,
                        on_input,
                    } = &lbox.node
                    else {
                        return false;
                    };

                    let mut chars: Vec<char> = value.chars().collect();
                    let caret = self.caret_position.unwrap_or(chars.len()).min(chars.len());
                    let box_id = lbox.id;
                    self.focused_widget_id = Some(box_id);

                    match named {
                        NamedKey::ArrowLeft => {
                            self.caret_position = Some(caret.saturating_sub(1));
                            self.needs_redraw = true;
                            return true;
                        }
                        NamedKey::ArrowRight => {
                            self.caret_position = Some((caret + 1).min(chars.len()));
                            self.needs_redraw = true;
                            return true;
                        }
                        NamedKey::Home => {
                            self.caret_position = Some(0);
                            self.needs_redraw = true;
                            return true;
                        }
                        NamedKey::End => {
                            self.caret_position = Some(chars.len());
                            self.needs_redraw = true;
                            return true;
                        }
                        NamedKey::Backspace => {
                            if caret == 0 {
                                return false;
                            }
                            chars.remove(caret - 1);
                            self.caret_position = Some(caret - 1);
                        }
                        NamedKey::Delete => {
                            if caret >= chars.len() {
                                return false;
                            }
                            chars.remove(caret);
                            self.caret_position = Some(caret);
                        }
                        _ => return false,
                    }

                    let new_value: String = chars.into_iter().collect();
                    let placeholder_str = placeholder.clone();
                    let widget_msg = on_input.as_ref().map(|cb| (cb.0)(new_value.clone()));
                    self.needs_redraw = true;

                    if let Some(msg) = widget_msg {
                        self.dispatch_message(msg);
                    }
                    if let Some(hook_msg) =
                        self.app.on_text_input(box_id, &placeholder_str, new_value)
                    {
                        self.dispatch_message(hook_msg);
                    }
                    return true;
                }
                NamedKey::Space => {
                    return self.handle_text_input(" ");
                }
                _ => {}
            }
        }

        if let Some(txt) = text
            && !txt.is_empty()
        {
            return self.handle_text_input(txt);
        }

        if let Key::Character(ch) = logical_key {
            return self.handle_text_input(ch.as_str());
        }

        false
    }

    /// Handles a scroll gesture `(delta_x, delta_y)` in logical pixels (following `winit`
    /// `MouseScrollDelta` convention where negative `delta_y` scrolls down and positive `delta_y`
    /// scrolls up):
    /// - Adjusts `scroll_offset_y` for overflowing `Scrollable` containers
    /// - Invokes `A::on_scroll` and dispatches any returned message to `A::update()`
    pub fn handle_scroll(&mut self, delta_x: f32, delta_y: f32) -> bool {
        let base_scene = {
            let (w, h) = self.viewport_size();
            let view = self.app.view();
            LayoutScene::from_element(&view, w as f32, h as f32)
        };
        let max_scroll = base_scene.max_scroll_offset();
        let next_offset = (self.scroll_offset_y - delta_y).clamp(0.0, max_scroll);

        let mut changed = (next_offset - self.scroll_offset_y).abs() > f32::EPSILON;
        self.scroll_offset_y = next_offset;

        if let Some(msg) = self.app.on_scroll(delta_x, delta_y) {
            self.dispatch_message(msg);
            changed = true;
        }

        if changed {
            self.needs_redraw = true;
        }
        changed
    }

    /// Sets the explicit vertical scroll offset in logical pixels (clamped to the scene's maximum overflow)
    /// and notifies `A::on_scroll`.
    pub fn set_scroll_offset(&mut self, scroll_offset_y: f32) -> f32 {
        let base_scene = {
            let (w, h) = self.viewport_size();
            let view = self.app.view();
            LayoutScene::from_element(&view, w as f32, h as f32)
        };
        let max_scroll = base_scene.max_scroll_offset();
        let clamped = scroll_offset_y.clamp(0.0, max_scroll);
        let delta_y = clamped - self.scroll_offset_y;
        self.scroll_offset_y = clamped;
        self.needs_redraw = true;
        if let Some(msg) = self.app.on_scroll(0.0, delta_y) {
            self.dispatch_message(msg);
        }
        self.scroll_offset_y
    }

    /// Handles a window resize event to `(width, height)`:
    /// - Updates `Core::window_width` and `Core::window_height`
    /// - Reconfigures the `wgpu::Surface` and `WgpuFramePresenter` texture
    /// - Invokes `A::on_resize` and dispatches any returned message to `A::update()`
    pub fn handle_resize(&mut self, width: u32, height: u32) -> bool {
        let safe_w = width.max(1);
        let safe_h = height.max(1);

        self.app.core_mut().window_width = safe_w;
        self.app.core_mut().window_height = safe_h;
        self.settings.width = safe_w;
        self.settings.height = safe_h;

        if let Some(gpu) = &mut self.gpu_surface {
            gpu.resize(safe_w, safe_h);
        }

        if let Some(msg) = self.app.on_resize(safe_w, safe_h) {
            self.dispatch_message(msg);
        }

        if self.scroll_offset_y > 0.0 {
            let max_scroll = {
                let (w, h) = self.viewport_size();
                let view = self.app.view();
                LayoutScene::from_element(&view, w as f32, h as f32).max_scroll_offset()
            };
            self.scroll_offset_y = self.scroll_offset_y.clamp(0.0, max_scroll);
        }

        self.needs_redraw = true;
        true
    }

    /// Handles a window HiDPI scale factor change.
    pub fn handle_scale_factor_changed(&mut self, scale_factor: f32) -> bool {
        let safe_scale = scale_factor.clamp(0.5, 4.0);
        self.app.core_mut().scale_factor = safe_scale;
        self.settings.scale_factor = safe_scale;
        self.needs_redraw = true;
        true
    }

    /// Rasterizes the current `A::view()` and presents the `256`-byte row-aligned RGBA8 texture
    /// to the active `wgpu::Surface`, if a window surface is attached.
    pub fn render_and_present(&mut self) -> Option<SurfacePresentStatus> {
        let pixmap = self.rasterize_frame();
        let width = pixmap.width();
        let height = pixmap.height();
        let gpu = self.gpu_surface.as_mut()?;
        let status = gpu.present_rgba8(pixmap.data(), width, height);
        self.last_present_status = Some(status);
        if status.was_presented() || status == SurfacePresentStatus::SkippedOccluded {
            self.needs_redraw = false;
        }
        Some(status)
    }
}

impl<A: Application> ApplicationHandler<A::Message> for CosmicAppRunner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu_surface.is_some() {
            return;
        }

        let (width, height) = self.viewport_size();
        let window_icon = self
            .settings
            .window_icon
            .as_ref()
            .and_then(|(rgba, w, h)| winit::window::Icon::from_rgba(rgba.clone(), *w, *h).ok());
        let attrs = WindowAttributes::default()
            .with_title(A::APP_ID)
            .with_inner_size(LogicalSize::new(width, height))
            .with_decorations(self.settings.client_decorations)
            .with_window_icon(window_icon);

        match event_loop.create_window(attrs) {
            Ok(window) => {
                let window = Arc::new(window);
                let scale = window.scale_factor() as f32;
                if scale > 0.0 {
                    self.handle_scale_factor_changed(scale);
                }
                let phys = window.inner_size();
                let w = if phys.width > 0 { phys.width } else { width }.max(1);
                let h = if phys.height > 0 { phys.height } else { height }.max(1);
                self.app.core_mut().window_width = w;
                self.app.core_mut().window_height = h;
                self.settings.width = w;
                self.settings.height = h;

                match WgpuWindowSurface::new(Arc::clone(&window), w, h) {
                    Ok(mut surface) => {
                        let pixmap = self.rasterize_frame();
                        let status =
                            surface.present_rgba8(pixmap.data(), pixmap.width(), pixmap.height());
                        self.last_present_status = Some(status);
                        window.request_redraw();
                        self.gpu_surface = Some(surface);
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "Failed to initialize wgpu window surface");
                        event_loop.exit();
                    }
                }
            }
            Err(err) => {
                tracing::error!(error = %err, "Failed to create native winit window");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: A::Message) {
        self.dispatch_message(event);
        if let Some(gpu) = &self.gpu_surface {
            gpu.window.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(PhysicalSize { width, height }) => {
                if width > 0 && height > 0 {
                    self.handle_resize(width, height);
                    if let Some(gpu) = &self.gpu_surface {
                        gpu.window.request_redraw();
                    }
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.handle_scale_factor_changed(scale_factor as f32);
                let phys = self.gpu_surface.as_ref().map(|g| g.window.inner_size());
                if let Some(PhysicalSize { width, height }) = phys
                    && width > 0
                    && height > 0
                    && (width, height) != self.viewport_size()
                {
                    self.handle_resize(width, height);
                }
                if let Some(gpu) = &self.gpu_surface {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved {
                position: PhysicalPosition { x, y },
                ..
            } => {
                if self.handle_cursor_moved(x as f32, y as f32)
                    && let Some(gpu) = &self.gpu_surface
                {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let (cx, cy) = self.cursor_position;
                if self.handle_pointer_click(cx, cy)
                    && let Some(gpu) = &self.gpu_surface
                {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(lx, ly) => {
                        (lx * SCROLL_PIXELS_PER_LINE, ly * SCROLL_PIXELS_PER_LINE)
                    }
                    MouseScrollDelta::PixelDelta(PhysicalPosition { x, y }) => (x as f32, y as f32),
                };
                if self.handle_scroll(dx, dy)
                    && let Some(gpu) = &self.gpu_surface
                {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let is_pressed = event.state.is_pressed();
                let text = event.text.as_ref().map(|s| s.as_str());
                if self.handle_key_event(&event.logical_key, text, is_pressed)
                    && let Some(gpu) = &self.gpu_surface
                {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                let _ = self.render_and_present();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.drain_pending_tasks() > 0 {
            self.needs_redraw = true;
        }
        if self.needs_redraw
            && let Some(gpu) = &self.gpu_surface
        {
            gpu.window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::Wait);
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.gpu_surface = None;
    }
}

/// Opens a native desktop window and runs the `winit` (`0.30.13`) + `wgpu` (`30.0.1`) event loop
/// for the given [`cosmic::Application`].
pub fn run<A: Application>(settings: Settings, flags: A::Flags) -> Result<(), String> {
    let event_loop = EventLoop::<A::Message>::with_user_event()
        .build()
        .map_err(|e| format!("Failed to initialize winit event loop: {e}"))?;
    let proxy = event_loop.create_proxy();
    let mut runner = CosmicAppRunner::<A>::new(settings, flags, Some(proxy));
    event_loop
        .run_app(&mut runner)
        .map_err(|e| format!("COSMIC application event loop error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;
    use crate::widget::{
        button, column, container, dropdown_with_select, row, scrollable, text, text_input, toggler,
    };

    #[derive(Debug, Clone, PartialEq)]
    enum RunnerTestMsg {
        InitCompleted(String),
        ButtonClicked,
        TogglerChanged(bool),
        InputChanged(String),
        DropdownSelected(usize),
        Scrolled(f32, f32),
        Resized(u32, u32),
        AsyncChainFinished(u32),
    }

    struct RunnerTestApp {
        core: Core,
        init_tag: String,
        click_count: u32,
        toggler_state: bool,
        input_value: String,
        dropdown_options: Vec<String>,
        dropdown_selected: usize,
        scroll_events: Vec<(f32, f32)>,
        resize_events: Vec<(u32, u32)>,
        async_value: u32,
    }

    impl Application for RunnerTestApp {
        const APP_ID: &'static str = "com.theinfomoose.rFerram.RunnerTest";
        type Executor = ();
        type Flags = &'static str;
        type Message = RunnerTestMsg;

        fn core(&self) -> &Core {
            &self.core
        }

        fn core_mut(&mut self) -> &mut Core {
            &mut self.core
        }

        fn init(core: Core, flags: Self::Flags) -> (Self, Task<Self::Message>) {
            let app = Self {
                core,
                init_tag: String::new(),
                click_count: 0,
                toggler_state: false,
                input_value: String::new(),
                dropdown_options: vec![
                    "None".to_string(),
                    "WireGuard".to_string(),
                    "Tailscale".to_string(),
                    "UniFi VPN".to_string(),
                ],
                dropdown_selected: 0,
                scroll_events: Vec::new(),
                resize_events: Vec::new(),
                async_value: 0,
            };
            let flag_str = flags.to_string();
            (
                app,
                Task::perform(async move { flag_str }, RunnerTestMsg::InitCompleted),
            )
        }

        fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
            match message {
                RunnerTestMsg::InitCompleted(tag) => {
                    self.init_tag = tag;
                    Task::none()
                }
                RunnerTestMsg::ButtonClicked => {
                    self.click_count += 1;
                    let next = self.click_count * 10;
                    Task::batch([
                        Task::done(RunnerTestMsg::AsyncChainFinished(next)),
                        Task::perform(async move { next + 5 }, RunnerTestMsg::AsyncChainFinished),
                    ])
                }
                RunnerTestMsg::TogglerChanged(val) => {
                    self.toggler_state = val;
                    Task::none()
                }
                RunnerTestMsg::InputChanged(val) => {
                    self.input_value = val;
                    Task::none()
                }
                RunnerTestMsg::DropdownSelected(idx) => {
                    self.dropdown_selected = idx;
                    Task::none()
                }
                RunnerTestMsg::Scrolled(dx, dy) => {
                    self.scroll_events.push((dx, dy));
                    Task::none()
                }
                RunnerTestMsg::Resized(w, h) => {
                    self.resize_events.push((w, h));
                    Task::none()
                }
                RunnerTestMsg::AsyncChainFinished(v) => {
                    self.async_value = v;
                    Task::none()
                }
            }
        }

        fn view(&self) -> crate::Element<'_, Self::Message> {
            let mut scroll_rows = Vec::new();
            for i in 0..24 {
                scroll_rows.push(
                    button(format!("Row {i}"))
                        .on_press(RunnerTestMsg::ButtonClicked)
                        .into(),
                );
            }

            container(
                "rferram-root-shell",
                column(
                    10,
                    vec![
                        row(
                            8,
                            vec![
                                button("Action")
                                    .on_press(RunnerTestMsg::ButtonClicked)
                                    .into(),
                                toggler(
                                    "Auto Reconnect",
                                    self.toggler_state,
                                    RunnerTestMsg::TogglerChanged(!self.toggler_state),
                                ),
                                text_input("Host name or IP address", &self.input_value)
                                    .on_input(RunnerTestMsg::InputChanged)
                                    .into(),
                                dropdown_with_select(
                                    self.dropdown_options.clone(),
                                    Some(self.dropdown_selected),
                                    RunnerTestMsg::DropdownSelected,
                                ),
                            ],
                        ),
                        scrollable(column(6, scroll_rows)),
                        text(format!("Selected: {}", self.dropdown_selected)).into(),
                    ],
                ),
            )
        }

        fn on_scroll(&mut self, delta_x: f32, delta_y: f32) -> Option<Self::Message> {
            Some(RunnerTestMsg::Scrolled(delta_x, delta_y))
        }

        fn on_resize(&mut self, width: u32, height: u32) -> Option<Self::Message> {
            Some(RunnerTestMsg::Resized(width, height))
        }
    }

    #[test]
    fn test_rgba8_256_byte_row_alignment_and_packing() {
        assert_eq!(align_bytes_per_row(256), 256);
        assert_eq!(align_bytes_per_row(1), 256);
        assert_eq!(align_bytes_per_row(1440 * 4), 5888);
        assert_eq!(5888 % WGPU_ROW_ALIGNMENT, 0);

        // Unaligned width: 100 px -> 400 bytes/row -> aligned to 512 bytes/row
        let width = 100u32;
        let height = 4u32;
        let mut raw = vec![0u8; (width * height * 4) as usize];
        for row in 0..height as usize {
            for col in 0..(width * 4) as usize {
                raw[row * (width as usize * 4) + col] = ((row + 1) * 17 + (col % 13)) as u8;
            }
        }

        let (aligned, stride) = pack_rgba8_to_wgpu_alignment(&raw, width, height);
        assert_eq!(stride, 512);
        assert_eq!(stride % WGPU_ROW_ALIGNMENT, 0);
        assert_eq!(aligned.len(), (stride * height) as usize);

        for row in 0..height as usize {
            let src_slice = &raw[row * 400..(row + 1) * 400];
            let dst_slice = &aligned[row * 512..row * 512 + 400];
            assert_eq!(src_slice, dst_slice);
            // Trailing padding bytes in each row must be zero-initialized
            assert!(
                aligned[row * 512 + 400..(row + 1) * 512]
                    .iter()
                    .all(|&b| b == 0)
            );
        }

        // Exact multiple of 64 pixels (64 * 4 = 256 bytes/row) requires zero extra row padding
        let exact_raw = vec![42u8; 64 * 2 * 4];
        let (exact_aligned, exact_stride) = pack_rgba8_to_wgpu_alignment(&exact_raw, 64, 2);
        assert_eq!(exact_stride, 256);
        assert_eq!(exact_aligned, exact_raw);
    }

    #[test]
    fn test_runner_clicks_typing_dropdown_scrolling_resize_and_tasks() {
        let mut runner = CosmicAppRunner::<RunnerTestApp>::headless(
            Settings {
                width: 1024,
                height: 600,
                scale_factor: 1.0,
                theme: Theme::dark(),
                client_decorations: true,
                window_icon: None,
            },
            "boot-flag-ok",
        );

        // 1. Drain initial Task::perform future spawned by init()
        let drained_init = runner.wait_and_drain_tasks(Duration::from_secs(2));
        assert!(drained_init >= 1);
        assert_eq!(runner.app.init_tag, "boot-flag-ok");

        // 2. Click "Action" button via hit-test coordinates and verify Task::batch execution
        let scene = runner.layout_scene();
        let action_btn = scene
            .boxes
            .iter()
            .find(|b| matches!(&b.node, WidgetNode::Button { label, .. } if label == "Action"))
            .expect("missing Action button");
        let btn_cx = action_btn.x + action_btn.width * 0.5;
        let btn_cy = action_btn.y + action_btn.height * 0.5;

        assert!(runner.handle_cursor_moved(btn_cx, btn_cy));
        assert_eq!(runner.hovered_widget_id, Some(action_btn.id));
        assert!(runner.handle_pointer_click(btn_cx, btn_cy));
        assert_eq!(runner.app.click_count, 1);

        let drained_click = runner.wait_and_drain_tasks(Duration::from_secs(2));
        assert!(drained_click >= 2);
        assert!(runner.app.async_value == 10 || runner.app.async_value == 15);

        // 3. Click Toggler and verify state toggles via A::update()
        let scene = runner.layout_scene();
        let tog_box = scene
            .boxes
            .iter()
            .find(|b| matches!(b.node, WidgetNode::Toggler { .. }))
            .expect("missing Toggler");
        assert!(!runner.app.toggler_state);
        assert!(runner.handle_pointer_click(
            tog_box.x + tog_box.width * 0.5,
            tog_box.y + tog_box.height * 0.5,
        ));
        assert!(runner.app.toggler_state);

        // 4. Click TextInput, type characters, navigate caret, Backspace, Delete
        let scene = runner.layout_scene();
        let input_box = scene
            .boxes
            .iter()
            .find(|b| matches!(b.node, WidgetNode::TextInput { .. }))
            .expect("missing TextInput");
        assert!(
            runner.handle_pointer_click(input_box.x + 12.0, input_box.y + input_box.height * 0.5,)
        );
        assert_eq!(runner.focused_widget_id, Some(input_box.id));

        assert!(runner.handle_text_input("10.0.0.12"));
        assert_eq!(runner.app.input_value, "10.0.0.12");
        assert_eq!(runner.caret_position, Some(9));

        // Backspace removes '2', then type "5:4433"
        assert!(runner.handle_key_event(&Key::Named(NamedKey::Backspace), None, true));
        assert_eq!(runner.app.input_value, "10.0.0.1");
        assert!(runner.handle_text_input("5:4433"));
        assert_eq!(runner.app.input_value, "10.0.0.15:4433");

        // Home + Delete removes leading '1', then insert '1' back
        assert!(runner.handle_key_event(&Key::Named(NamedKey::Home), None, true));
        assert_eq!(runner.caret_position, Some(0));
        assert!(runner.handle_key_event(&Key::Named(NamedKey::Delete), None, true));
        assert_eq!(runner.app.input_value, "0.0.0.15:4433");
        assert!(runner.handle_text_input("1"));
        assert_eq!(runner.app.input_value, "10.0.0.15:4433");

        // 5. Click Dropdown to cycle selection and test explicit option index selection
        let scene = runner.layout_scene();
        let dd_box = scene
            .boxes
            .iter()
            .find(|b| matches!(b.node, WidgetNode::Dropdown { .. }))
            .expect("missing Dropdown");
        assert_eq!(runner.app.dropdown_selected, 0);
        assert!(runner.handle_pointer_click(
            dd_box.x + dd_box.width * 0.5,
            dd_box.y + dd_box.height * 0.5,
        ));
        assert_eq!(runner.app.dropdown_selected, 1);

        assert!(runner.handle_dropdown_select(dd_box.id, 3));
        assert_eq!(runner.app.dropdown_selected, 3);

        // 6. Scroll the overflowing Scrollable container (downward twice, then upward, then past top)
        assert!(runner.handle_scroll(0.0, -36.0));
        assert!((runner.scroll_offset_y - 36.0).abs() < f32::EPSILON);
        assert!(runner.handle_scroll(0.0, -36.0));
        assert!((runner.scroll_offset_y - 72.0).abs() < f32::EPSILON);
        assert!(runner.handle_scroll(0.0, 20.0));
        assert!((runner.scroll_offset_y - 52.0).abs() < f32::EPSILON);
        // Scrolling up by a delta larger than current scroll_offset_y must clamp to 0.0
        assert!(runner.handle_scroll(0.0, 200.0));
        assert!(runner.scroll_offset_y.abs() < f32::EPSILON);
        assert_eq!(runner.app.scroll_events.last().copied(), Some((0.0, 200.0)));

        // 7. Resize the window viewport and verify Core + A::update() + packed 256-byte row alignment
        assert!(runner.handle_resize(1440, 900));
        assert_eq!(runner.app.core().window_width, 1440);
        assert_eq!(runner.app.core().window_height, 900);
        assert_eq!(runner.app.resize_events.last().copied(), Some((1440, 900)));

        let (aligned_buf, w, h, stride) = runner.pack_aligned_frame();
        assert_eq!(w, 1440);
        assert_eq!(h, 900);
        assert_eq!(stride, 5888);
        assert_eq!(stride % WGPU_ROW_ALIGNMENT, 0);
        assert_eq!(aligned_buf.len(), (5888 * 900) as usize);
    }

    #[test]
    fn test_surface_acquisition_graceful_fallbacks() {
        let (frame, status) = classify_surface_acquisition(wgpu::CurrentSurfaceTexture::Occluded);
        assert!(frame.is_none());
        assert_eq!(status, SurfacePresentStatus::SkippedOccluded);
        assert!(!status.was_presented());
        assert!(!status.needs_reconfigure());

        let (frame, status) = classify_surface_acquisition(wgpu::CurrentSurfaceTexture::Outdated);
        assert!(frame.is_none());
        assert_eq!(status, SurfacePresentStatus::SkippedOutdated);
        assert!(!status.was_presented());
        assert!(status.needs_reconfigure());

        let (frame, status) = classify_surface_acquisition(wgpu::CurrentSurfaceTexture::Lost);
        assert!(frame.is_none());
        assert_eq!(status, SurfacePresentStatus::SkippedLost);
        assert!(status.needs_reconfigure());

        let (frame, status) = classify_surface_acquisition(wgpu::CurrentSurfaceTexture::Timeout);
        assert!(frame.is_none());
        assert_eq!(status, SurfacePresentStatus::SkippedTimeout);

        let (frame, status) = classify_surface_acquisition(wgpu::CurrentSurfaceTexture::Validation);
        assert!(frame.is_none());
        assert_eq!(status, SurfacePresentStatus::SkippedValidation);
    }

    #[test]
    fn test_wgpu_frame_presenter_upload_and_render_pass() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = block_on_future(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            ..Default::default()
        })) else {
            return;
        };

        let Ok((device, queue)) =
            block_on_future(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            return;
        };

        let mut runner = CosmicAppRunner::<RunnerTestApp>::headless(
            Settings {
                width: 1440,
                height: 900,
                scale_factor: 1.0,
                theme: Theme::dark(),
                client_decorations: true,
                window_icon: None,
            },
            "wgpu-test",
        );
        let _ = runner.wait_and_drain_tasks(Duration::from_secs(1));

        let pixmap = runner.rasterize_frame();
        assert_eq!(pixmap.width(), 1440);
        assert_eq!(pixmap.height(), 900);

        let target_format = wgpu::TextureFormat::Bgra8UnormSrgb;
        let mut presenter = WgpuFramePresenter::new(device, queue, 1440, 900, target_format);
        let aligned_stride = presenter.upload_aligned_rgba8(pixmap.data(), 1440, 900);
        assert_eq!(aligned_stride, 5888);
        assert_eq!(aligned_stride % WGPU_ROW_ALIGNMENT, 0);

        let render_target = presenter.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cosmic_test_render_target"),
            size: wgpu::Extent3d {
                width: 1440,
                height: 900,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = render_target.create_view(&wgpu::TextureViewDescriptor::default());
        presenter.render_to_view(&target_view);

        // Read back the first row from the GPU render target to verify WGSL shader output
        let readback_buf = presenter.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cosmic_test_readback_buf"),
            size: u64::from(aligned_stride),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut copy_enc =
            presenter
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("cosmic_test_copy_enc"),
                });
        copy_enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &render_target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback_buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(aligned_stride),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1440,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        presenter.queue.submit(std::iter::once(copy_enc.finish()));

        let slice = readback_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        let _ = presenter.device.poll(wgpu::PollType::wait_indefinitely());
        if let Ok(Ok(())) = rx.recv_timeout(Duration::from_secs(2)) {
            let mapped = slice.get_mapped_range().expect("mapped range");
            assert!(
                mapped[..1440 * 4].iter().any(|&b| b != 0),
                "expected non-zero rendered BGRA8 pixels from WGSL surface shader"
            );
            drop(mapped);
            readback_buf.unmap();
        }
    }
}
