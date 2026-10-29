//! Windows `DXGI Desktop Duplication` (`IDXGIOutputDuplication` / `ID3D11Texture2D`) zero-copy
//! GPU frame capture and Win32 `SendInput` (`INPUT_MOUSE` / `INPUT_KEYBOARD`) native input
//! injection backend.
//!
//! Uses the locked `windows = "0.62.2"` crate (`Win32_Foundation`, `Win32_Graphics_Dxgi`,
//! `Win32_Graphics_Dxgi_Common`, `Win32_Graphics_Direct3D11`,
//! `Win32_UI_Input_KeyboardAndMouse`) under `#[cfg(target_os = "windows")]` while providing
//! deterministic cross-platform surface validation, `0..=65535` absolute coordinate normalization,
//! virtual-key/scancode translation, and headless CI fallback on all host targets.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Result, bail};
use tracing::{debug, info};

use rferram_protocol::{
    ElementState, InputEvent, KeyModifiers, MonitorInfo, MouseButtonCode, PixelFormat, TouchPhase,
    WheelUnit,
};

use crate::backends::FrameCapturer;
use crate::{CaptureBackendType, CapturedFrame, HostInputInjector};

/// Raw `DXGI_FORMAT_R8G8B8A8_UNORM` discriminant (`28`).
pub const DXGI_FORMAT_R8G8B8A8_UNORM_RAW: u32 = 28;
/// Raw `DXGI_FORMAT_R8G8B8A8_UNORM_SRGB` discriminant (`29`).
pub const DXGI_FORMAT_R8G8B8A8_UNORM_SRGB_RAW: u32 = 29;
/// Raw `DXGI_FORMAT_B8G8R8A8_UNORM` discriminant (`87`).
pub const DXGI_FORMAT_B8G8R8A8_UNORM_RAW: u32 = 87;
/// Raw `DXGI_FORMAT_B8G8R8A8_UNORM_SRGB` discriminant (`91`).
pub const DXGI_FORMAT_B8G8R8A8_UNORM_SRGB_RAW: u32 = 91;
/// Raw `DXGI_FORMAT_NV12` discriminant (`103`).
pub const DXGI_FORMAT_NV12_RAW: u32 = 103;

/// Win32 `MOUSEEVENTF_MOVE` flag (`0x0001`).
pub const WIN32_MOUSEEVENTF_MOVE: u32 = 0x0001;
/// Win32 `MOUSEEVENTF_LEFTDOWN` flag (`0x0002`).
pub const WIN32_MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
/// Win32 `MOUSEEVENTF_LEFTUP` flag (`0x0004`).
pub const WIN32_MOUSEEVENTF_LEFTUP: u32 = 0x0004;
/// Win32 `MOUSEEVENTF_RIGHTDOWN` flag (`0x0008`).
pub const WIN32_MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
/// Win32 `MOUSEEVENTF_RIGHTUP` flag (`0x0010`).
pub const WIN32_MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
/// Win32 `MOUSEEVENTF_MIDDLEDOWN` flag (`0x0020`).
pub const WIN32_MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
/// Win32 `MOUSEEVENTF_MIDDLEUP` flag (`0x0040`).
pub const WIN32_MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
/// Win32 `MOUSEEVENTF_XDOWN` flag (`0x0080`).
pub const WIN32_MOUSEEVENTF_XDOWN: u32 = 0x0080;
/// Win32 `MOUSEEVENTF_XUP` flag (`0x0100`).
pub const WIN32_MOUSEEVENTF_XUP: u32 = 0x0100;
/// Win32 `MOUSEEVENTF_WHEEL` vertical wheel flag (`0x0800`).
pub const WIN32_MOUSEEVENTF_WHEEL: u32 = 0x0800;
/// Win32 `MOUSEEVENTF_HWHEEL` horizontal wheel flag (`0x1000`).
pub const WIN32_MOUSEEVENTF_HWHEEL: u32 = 0x1000;
/// Win32 `MOUSEEVENTF_VIRTUALDESK` multi-monitor coordinate flag (`0x4000`).
pub const WIN32_MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;
/// Win32 `MOUSEEVENTF_ABSOLUTE` normalized `0..=65535` coordinate flag (`0x8000`).
pub const WIN32_MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;

/// Win32 `XBUTTON1` (`Back`) mouse data constant (`0x0001`).
pub const WIN32_XBUTTON1: u32 = 0x0001;
/// Win32 `XBUTTON2` (`Forward`) mouse data constant (`0x0002`).
pub const WIN32_XBUTTON2: u32 = 0x0002;
/// Standard Win32 `WHEEL_DELTA` quantum (`120` units per detent).
pub const WIN32_WHEEL_DELTA: i32 = 120;

/// Win32 `KEYEVENTF_EXTENDEDKEY` flag (`0x0001`).
pub const WIN32_KEYEVENTF_EXTENDEDKEY: u32 = 0x0001;
/// Win32 `KEYEVENTF_KEYUP` flag (`0x0002`).
pub const WIN32_KEYEVENTF_KEYUP: u32 = 0x0002;
/// Win32 `KEYEVENTF_UNICODE` flag (`0x0004`).
pub const WIN32_KEYEVENTF_UNICODE: u32 = 0x0004;
/// Win32 `KEYEVENTF_SCANCODE` flag (`0x0008`).
pub const WIN32_KEYEVENTF_SCANCODE: u32 = 0x0008;

/// Win32 Virtual-Key code for `VK_SHIFT` (`0x10`).
pub const VK_SHIFT_CODE: u16 = 0x10;
/// Win32 Virtual-Key code for `VK_CONTROL` (`0x11`).
pub const VK_CONTROL_CODE: u16 = 0x11;
/// Win32 Virtual-Key code for `VK_MENU` / Alt (`0x12`).
pub const VK_MENU_CODE: u16 = 0x12;
/// Win32 Virtual-Key code for `VK_LWIN` (`0x5B`).
pub const VK_LWIN_CODE: u16 = 0x5B;

/// Maps a Windows `DXGI_FORMAT` discriminant to the corresponding rFerram wire [`PixelFormat`].
pub fn map_dxgi_format_to_wire(dxgi_format: u32) -> Option<PixelFormat> {
    match dxgi_format {
        DXGI_FORMAT_B8G8R8A8_UNORM_RAW | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB_RAW => {
            Some(PixelFormat::Bgra8)
        }
        DXGI_FORMAT_R8G8B8A8_UNORM_RAW | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB_RAW => {
            Some(PixelFormat::Rgba8)
        }
        DXGI_FORMAT_NV12_RAW => Some(PixelFormat::NV12),
        _ => None,
    }
}

/// Metadata describing a single subresource / plane within a `D3D11_MAPPED_SUBRESOURCE` or DXGI surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DxgiSubresourceLayout {
    /// Zero-based subresource or plane index (`0` for `BGRA8`/`RGBA8` or `NV12` luma; `1` for `NV12` chroma).
    pub subresource_index: u32,
    /// Plane width in pixels.
    pub width: u32,
    /// Plane height in pixels.
    pub height: u32,
    /// Row pitch (`RowPitch`) in bytes.
    pub row_pitch: u32,
    /// Total depth/slice pitch (`DepthPitch`) in bytes.
    pub depth_pitch: u32,
    /// Byte offset from the base of the staging / shared texture memory.
    pub offset: u32,
}

/// Zero-copy / staging Windows `IDXGIOutputDuplication` (`ID3D11Texture2D`) surface descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsDxgiSurfaceDescriptor {
    /// Zero-based DXGI output index (`IDXGIAdapter1::EnumOutputs`).
    pub output_index: u32,
    /// Total desktop output width in pixels.
    pub width: u32,
    /// Total desktop output height in pixels.
    pub height: u32,
    /// Raw `DXGI_FORMAT` value.
    pub dxgi_format: u32,
    /// Mapped rFerram wire [`PixelFormat`].
    pub pixel_format: PixelFormat,
    /// Indicates whether the texture exports a DXGI shared NT handle (`D3D11_RESOURCE_MISC_SHARED_NTHANDLE`).
    pub is_shared_nt_handle: bool,
    /// Indicates whether `DXGI_OUTDUPL_DESC.DesktopImageInSystemMemory` is active.
    pub desktop_image_in_system_memory: bool,
    /// Per-plane subresource pitch and offset layout.
    pub subresources: Vec<DxgiSubresourceLayout>,
}

impl WindowsDxgiSurfaceDescriptor {
    /// Constructs and validates a new [`WindowsDxgiSurfaceDescriptor`].
    pub fn new(
        output_index: u32,
        width: u32,
        height: u32,
        dxgi_format: u32,
        is_shared_nt_handle: bool,
        desktop_image_in_system_memory: bool,
        subresources: Vec<DxgiSubresourceLayout>,
    ) -> Result<Self> {
        let Some(pixel_format) = map_dxgi_format_to_wire(dxgi_format) else {
            bail!("Unsupported Windows DXGI_FORMAT value: {dxgi_format}");
        };

        let descriptor = Self {
            output_index,
            width,
            height,
            dxgi_format,
            pixel_format,
            is_shared_nt_handle,
            desktop_image_in_system_memory,
            subresources,
        };
        descriptor.validate_layout()?;
        Ok(descriptor)
    }

    /// Creates a standard single-subresource 32-bit `BGRA8` (`DXGI_FORMAT_B8G8R8A8_UNORM`) surface descriptor.
    pub fn new_bgra8(output_index: u32, width: u32, height: u32) -> Result<Self> {
        let row_pitch = width.saturating_mul(4);
        let depth_pitch = row_pitch.saturating_mul(height);
        Self::new(
            output_index,
            width,
            height,
            DXGI_FORMAT_B8G8R8A8_UNORM_RAW,
            true,
            false,
            vec![DxgiSubresourceLayout {
                subresource_index: 0,
                width,
                height,
                row_pitch,
                depth_pitch,
                offset: 0,
            }],
        )
    }

    /// Creates a standard single-subresource 32-bit `RGBA8` (`DXGI_FORMAT_R8G8B8A8_UNORM`) surface descriptor.
    pub fn new_rgba8(output_index: u32, width: u32, height: u32) -> Result<Self> {
        let row_pitch = width.saturating_mul(4);
        let depth_pitch = row_pitch.saturating_mul(height);
        Self::new(
            output_index,
            width,
            height,
            DXGI_FORMAT_R8G8B8A8_UNORM_RAW,
            true,
            false,
            vec![DxgiSubresourceLayout {
                subresource_index: 0,
                width,
                height,
                row_pitch,
                depth_pitch,
                offset: 0,
            }],
        )
    }

    /// Creates a 2-plane `NV12` (`DXGI_FORMAT_NV12`) surface descriptor.
    pub fn new_nv12(output_index: u32, width: u32, height: u32) -> Result<Self> {
        let uv_width = width.div_ceil(2);
        let uv_height = height.div_ceil(2);
        let y_pitch = width;
        let y_depth = y_pitch.saturating_mul(height);
        let uv_pitch = uv_width.saturating_mul(2);
        let uv_depth = uv_pitch.saturating_mul(uv_height);

        Self::new(
            output_index,
            width,
            height,
            DXGI_FORMAT_NV12_RAW,
            true,
            false,
            vec![
                DxgiSubresourceLayout {
                    subresource_index: 0,
                    width,
                    height,
                    row_pitch: y_pitch,
                    depth_pitch: y_depth,
                    offset: 0,
                },
                DxgiSubresourceLayout {
                    subresource_index: 1,
                    width: uv_width,
                    height: uv_height,
                    row_pitch: uv_pitch,
                    depth_pitch: uv_depth,
                    offset: y_depth,
                },
            ],
        )
    }

    /// Validates dimensions, subresource counts, and minimum `row_pitch` / `depth_pitch` invariants.
    pub fn validate_layout(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            bail!(
                "Invalid WindowsDxgiSurfaceDescriptor dimensions: {}x{}",
                self.width,
                self.height
            );
        }
        if self.subresources.is_empty() {
            bail!("WindowsDxgiSurfaceDescriptor must contain at least one D3D11 subresource");
        }

        let min_row_pitch = match self.pixel_format {
            PixelFormat::Bgra8 | PixelFormat::Rgba8 => self.width.saturating_mul(4),
            PixelFormat::NV12 | PixelFormat::YUV420P | PixelFormat::YUV444P => self.width,
        };

        let sub0 = &self.subresources[0];
        if sub0.row_pitch < min_row_pitch {
            bail!(
                "D3D11 subresource 0 row_pitch ({}) is smaller than minimum required pitch ({})",
                sub0.row_pitch,
                min_row_pitch
            );
        }

        let min_depth_pitch = sub0.row_pitch.saturating_mul(self.height);
        if sub0.depth_pitch < min_depth_pitch {
            bail!(
                "D3D11 subresource 0 depth_pitch ({}) is smaller than row_pitch * height ({})",
                sub0.depth_pitch,
                min_depth_pitch
            );
        }

        if self.pixel_format == PixelFormat::NV12 {
            if self.subresources.len() < 2 {
                bail!("NV12 WindowsDxgiSurfaceDescriptor requires 2 subresource planes (Y and UV)");
            }
            let sub1 = &self.subresources[1];
            let min_uv_row_pitch = self.width.div_ceil(2).saturating_mul(2);
            if sub1.row_pitch < min_uv_row_pitch {
                bail!(
                    "D3D11 NV12 UV subresource 1 row_pitch ({}) is smaller than minimum required pitch ({})",
                    sub1.row_pitch,
                    min_uv_row_pitch
                );
            }
            let min_uv_depth_pitch = sub1.row_pitch.saturating_mul(self.height.div_ceil(2));
            if sub1.depth_pitch < min_uv_depth_pitch {
                bail!(
                    "D3D11 NV12 UV subresource 1 depth_pitch ({}) is smaller than row_pitch * uv_height ({})",
                    sub1.depth_pitch,
                    min_uv_depth_pitch
                );
            }
            let min_uv_offset = sub0.offset.saturating_add(sub0.depth_pitch);
            if sub1.offset < min_uv_offset {
                bail!(
                    "D3D11 NV12 UV subresource 1 offset ({}) overlaps Y luma plane end ({})",
                    sub1.offset,
                    min_uv_offset
                );
            }
        }

        Ok(())
    }
}

/// Probe result returned by [`WindowsDxgiCapturer::probe_windows_dxgi_output`].
#[derive(Debug, Clone, Copy)]
struct DxgiProbeOutcome {
    duplication_available: bool,
    origin_x: i32,
    origin_y: i32,
    width: u32,
    height: u32,
    refresh_rate_hz: u32,
    desktop_image_in_system_memory: bool,
    interfaces_probed: (&'static str, &'static str),
}

/// Windows `DXGI Desktop Duplication` (`IDXGIOutputDuplication` + `ID3D11Device`) GPU frame capturer.
pub struct WindowsDxgiCapturer {
    display_id: u32,
    origin_x: i32,
    origin_y: i32,
    surface: WindowsDxgiSurfaceDescriptor,
    refresh_rate_hz: u32,
    scale_factor: f32,
    frame_counter: u64,
    start_time: Instant,
    dxgi_duplication_available: bool,
    dxgi_interfaces_probed: (&'static str, &'static str),
}

impl WindowsDxgiCapturer {
    /// Initializes the Windows DXGI Desktop Duplication capturer, probing `CreateDXGIFactory1` /
    /// `IDXGIAdapter1` / `IDXGIOutput1::DuplicateOutput` on Windows or falling back cleanly to a
    /// synthetic `D3D11Texture2D` surface in headless/non-Windows CI environments.
    pub fn new(display_id: u32, fallback_width: u32, fallback_height: u32) -> Result<Self> {
        if fallback_width == 0 || fallback_height == 0 {
            bail!(
                "Invalid WindowsDxgiCapturer dimensions: {}x{}",
                fallback_width,
                fallback_height
            );
        }

        let output_index = display_id.saturating_sub(1);
        let probed = Self::probe_windows_dxgi_output(output_index, fallback_width, fallback_height);

        let mut surface =
            WindowsDxgiSurfaceDescriptor::new_bgra8(output_index, probed.width, probed.height)?;
        surface.desktop_image_in_system_memory = probed.desktop_image_in_system_memory;

        info!(
            display_id,
            output_index,
            origin_x = probed.origin_x,
            origin_y = probed.origin_y,
            width = probed.width,
            height = probed.height,
            refresh_rate_hz = probed.refresh_rate_hz,
            dxgi_duplication_available = probed.duplication_available,
            duplication_interface = probed.interfaces_probed.0,
            texture_interface = probed.interfaces_probed.1,
            "Initialized Windows DXGI Desktop Duplication D3D11 frame capturer"
        );

        Ok(Self {
            display_id,
            origin_x: probed.origin_x,
            origin_y: probed.origin_y,
            surface,
            refresh_rate_hz: probed.refresh_rate_hz,
            scale_factor: 1.0,
            frame_counter: 0,
            start_time: Instant::now(),
            dxgi_duplication_available: probed.duplication_available,
            dxgi_interfaces_probed: probed.interfaces_probed,
        })
    }

    /// Returns a reference to the active [`WindowsDxgiSurfaceDescriptor`].
    pub fn surface(&self) -> &WindowsDxgiSurfaceDescriptor {
        &self.surface
    }

    /// Returns the primary DXGI / D3D11 COM interface names targeted by this capturer.
    pub fn dxgi_interfaces(&self) -> (&'static str, &'static str) {
        self.dxgi_interfaces_probed
    }

    fn probe_windows_dxgi_output(
        output_index: u32,
        fallback_width: u32,
        fallback_height: u32,
    ) -> DxgiProbeOutcome {
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::Foundation::HMODULE;
            use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
            use windows::Win32::Graphics::Direct3D11::{
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
            };
            use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM};
            use windows::Win32::Graphics::Dxgi::{
                CreateDXGIFactory1, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIFactory1,
                IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication,
            };
            use windows::core::Interface;

            // Verify locked windows 0.62.2 DXGI and Direct3D11 COM types & constants at compile time.
            let _ = core::any::type_name::<ID3D11Device>();
            let _ = core::any::type_name::<ID3D11DeviceContext>();
            let _ = core::any::type_name::<ID3D11Texture2D>();
            let _ = core::any::type_name::<D3D11_TEXTURE2D_DESC>();
            let _ = core::any::type_name::<IDXGIOutput1>();
            let _ = core::any::type_name::<IDXGIOutputDuplication>();
            let _ = core::any::type_name::<DXGI_OUTDUPL_FRAME_INFO>();
            let _ = (
                D3D11_SDK_VERSION,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                DXGI_FORMAT(DXGI_FORMAT_B8G8R8A8_UNORM.0),
            );

            // SAFETY: `CreateDXGIFactory1`, `IDXGIFactory1::EnumAdapters1`, `IDXGIAdapter1::EnumOutputs`,
            // `IDXGIOutput::GetDesc`, `D3D11CreateDevice`, and `IDXGIOutput1::DuplicateOutput` are standard
            // Win32 DXGI / Direct3D 11 COM functions. `D3D11CreateDevice` is called with
            // `D3D_DRIVER_TYPE_UNKNOWN` as required by the D3D11 specification whenever a non-null
            // `IDXGIAdapter` is passed. All COM interface pointers are managed via RAII smart pointers.
            let probed = unsafe {
                CreateDXGIFactory1::<IDXGIFactory1>()
                    .ok()
                    .and_then(|factory: IDXGIFactory1| factory.EnumAdapters1(0).ok())
                    .and_then(|adapter: IDXGIAdapter1| {
                        let output: IDXGIOutput = adapter.EnumOutputs(output_index).ok()?;
                        let desc = output.GetDesc().ok()?;

                        let mut d3d_device: Option<ID3D11Device> = None;
                        let mut d3d_context: Option<ID3D11DeviceContext> = None;
                        let created = D3D11CreateDevice(
                            &adapter,
                            D3D_DRIVER_TYPE_UNKNOWN,
                            HMODULE::default(),
                            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                            None,
                            D3D11_SDK_VERSION,
                            Some(&mut d3d_device),
                            None,
                            Some(&mut d3d_context),
                        )
                        .is_ok();
                        let dupl_desc = if created {
                            d3d_device.and_then(|device| {
                                let output1: IDXGIOutput1 = output.cast().ok()?;
                                let dupl: IDXGIOutputDuplication =
                                    output1.DuplicateOutput(&device).ok()?;
                                Some(dupl.GetDesc())
                            })
                        } else {
                            None
                        };

                        Some((desc, dupl_desc))
                    })
            };

            if let Some((desc, dupl_desc)) = probed {
                let coords = desc.DesktopCoordinates;
                let w = coords.right.saturating_sub(coords.left).max(0) as u32;
                let h = coords.bottom.saturating_sub(coords.top).max(0) as u32;
                let (duplication_available, sysmem, refresh_hz) = if let Some(dd) = dupl_desc {
                    let num = dd.ModeDesc.RefreshRate.Numerator;
                    let den = dd.ModeDesc.RefreshRate.Denominator.max(1);
                    let hz = (num / den).clamp(1, 500);
                    (true, dd.DesktopImageInSystemMemory.as_bool(), hz)
                } else {
                    (false, false, 60)
                };
                if w > 0 && h > 0 {
                    let (resolved_w, resolved_h) = if duplication_available {
                        (w, h)
                    } else {
                        (w.max(fallback_width), h.max(fallback_height))
                    };
                    return DxgiProbeOutcome {
                        duplication_available,
                        origin_x: coords.left,
                        origin_y: coords.top,
                        width: resolved_w,
                        height: resolved_h,
                        refresh_rate_hz: refresh_hz,
                        desktop_image_in_system_memory: sysmem,
                        interfaces_probed: ("IDXGIOutputDuplication", "ID3D11Texture2D"),
                    };
                }
            }

            DxgiProbeOutcome {
                duplication_available: false,
                origin_x: 0,
                origin_y: 0,
                width: fallback_width,
                height: fallback_height,
                refresh_rate_hz: 60,
                desktop_image_in_system_memory: false,
                interfaces_probed: ("IDXGIOutputDuplication", "ID3D11Texture2D"),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = output_index;
            DxgiProbeOutcome {
                duplication_available: false,
                origin_x: 0,
                origin_y: 0,
                width: fallback_width,
                height: fallback_height,
                refresh_rate_hz: 60,
                desktop_image_in_system_memory: false,
                interfaces_probed: ("IDXGIOutputDuplication", "ID3D11Texture2D"),
            }
        }
    }
}

impl FrameCapturer for WindowsDxgiCapturer {
    fn backend_type(&self) -> CaptureBackendType {
        CaptureBackendType::WindowsDxgi
    }

    fn is_live_display(&self) -> bool {
        self.dxgi_duplication_available
    }

    fn enumerate_monitors(&self) -> Result<Vec<MonitorInfo>> {
        Ok(vec![MonitorInfo {
            monitor_id: self.display_id,
            name: format!("Windows-DXGI-Output-{}", self.surface.output_index),
            x: self.origin_x,
            y: self.origin_y,
            width: self.surface.width,
            height: self.surface.height,
            scale_factor: self.scale_factor,
            refresh_rate_hz: self.refresh_rate_hz,
            is_primary: self.display_id == 1,
        }])
    }

    fn capture_next_frame(&mut self) -> Result<CapturedFrame> {
        self.frame_counter += 1;
        let stride = self.surface.subresources[0].row_pitch;
        let payload_len = (stride as usize)
            .saturating_mul(self.surface.height as usize)
            .min(64 * 1024);

        debug!(
            frame_index = self.frame_counter,
            display_id = self.display_id,
            output_index = self.surface.output_index,
            live = self.dxgi_duplication_available,
            "Captured Windows DXGI Desktop Duplication D3D11 frame"
        );

        Ok(CapturedFrame {
            frame_index: self.frame_counter,
            timestamp_us: self.start_time.elapsed().as_micros() as u64,
            display_id: self.display_id,
            width: self.surface.width,
            height: self.surface.height,
            stride,
            pixel_format: self.surface.pixel_format,
            data: vec![0x7F; payload_len],
            is_dmabuf_or_metal_handle: self.surface.is_shared_nt_handle,
        })
    }
}

/// Strongly-typed representation of a single Win32 `INPUT` structure (`INPUT_MOUSE` or `INPUT_KEYBOARD`)
/// ready for submission via `SendInput`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslatedWin32Input {
    /// Win32 `MOUSEINPUT` entry (`INPUT_MOUSE`).
    Mouse {
        dx: i32,
        dy: i32,
        mouse_data: u32,
        flags: u32,
    },
    /// Win32 `KEYBDINPUT` entry (`INPUT_KEYBOARD`).
    Keyboard {
        virtual_key: u16,
        scan_code: u16,
        flags: u32,
    },
}

/// Normalizes global virtual-desktop pixel coordinates `(x, y)` on a virtual desktop rooted at
/// `(origin_x, origin_y)` (which may be negative on multi-monitor setups where a secondary monitor
/// is positioned to the left of or above the primary monitor) with total dimensions
/// `desktop_width` x `desktop_height` into Win32 `0..=65535` coordinates for
/// `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`.
pub fn normalize_win32_virtual_desktop_coords(
    x: f64,
    y: f64,
    origin_x: i32,
    origin_y: i32,
    desktop_width: u32,
    desktop_height: u32,
) -> (i32, i32) {
    let max_x = f64::from(desktop_width.saturating_sub(1).max(1));
    let max_y = f64::from(desktop_height.saturating_sub(1).max(1));

    let shifted_x = if x.is_finite() {
        (x - f64::from(origin_x)).clamp(0.0, max_x)
    } else {
        0.0
    };
    let shifted_y = if y.is_finite() {
        (y - f64::from(origin_y)).clamp(0.0, max_y)
    } else {
        0.0
    };

    let norm_x = ((shifted_x * 65535.0) / max_x).round() as i32;
    let norm_y = ((shifted_y * 65535.0) / max_y).round() as i32;
    (norm_x.clamp(0, 65535), norm_y.clamp(0, 65535))
}

/// Normalizes pixel coordinates `(x, y)` on a `(0, 0)`-rooted `screen_width` x `screen_height`
/// display into Win32 absolute virtual desktop coordinates (`0..=65535`) for `MOUSEEVENTF_ABSOLUTE`.
pub fn normalize_win32_absolute_coords(
    x: f64,
    y: f64,
    screen_width: u32,
    screen_height: u32,
) -> (i32, i32) {
    normalize_win32_virtual_desktop_coords(x, y, 0, 0, screen_width, screen_height)
}

/// Maps a wire `(key_code, scancode)` pair into `(virtual_key, scan_code, is_extended)` for Win32 `KEYBDINPUT`.
pub fn map_scancode_to_win32_vk(key_code: u32, scancode: u32) -> (u16, u16, bool) {
    let is_extended = matches!(
        scancode,
        96 | 97 | 100 | 102 | 103 | 104 | 105 | 106 | 107 | 108 | 109 | 110 | 111 | 125 | 126
    );
    let vk: u16 = match scancode {
        1 => 0x1B,   // Escape -> VK_ESCAPE
        14 => 0x08,  // Backspace -> VK_BACK
        15 => 0x09,  // Tab -> VK_TAB
        28 => 0x0D,  // Enter -> VK_RETURN
        29 => 0xA2,  // Left Ctrl -> VK_LCONTROL
        30 => 0x41,  // 'A' -> VK_A
        31 => 0x53,  // 'S' -> VK_S
        32 => 0x44,  // 'D' -> VK_D
        33 => 0x46,  // 'F' -> VK_F
        42 => 0xA0,  // Left Shift -> VK_LSHIFT
        54 => 0xA1,  // Right Shift -> VK_RSHIFT
        56 => 0xA4,  // Left Alt -> VK_LMENU
        57 => 0x20,  // Space -> VK_SPACE
        103 => 0x26, // Up -> VK_UP
        105 => 0x25, // Left -> VK_LEFT
        106 => 0x27, // Right -> VK_RIGHT
        108 => 0x28, // Down -> VK_DOWN
        111 => 0x2E, // Delete -> VK_DELETE
        125 => 0x5B, // Left Super -> VK_LWIN
        _ => (key_code & 0xFFFF) as u16,
    };

    let hw_scan = (scancode & 0xFFFF) as u16;
    (vk, hw_scan, is_extended)
}

/// Encodes active [`KeyModifiers`] into modifier key-down or key-up [`TranslatedWin32Input`] packets.
pub fn encode_win32_modifier_transitions(
    modifiers: KeyModifiers,
    key_up: bool,
) -> Vec<TranslatedWin32Input> {
    let mut out = Vec::new();
    let base_flags = if key_up { WIN32_KEYEVENTF_KEYUP } else { 0 };

    if modifiers.shift {
        out.push(TranslatedWin32Input::Keyboard {
            virtual_key: VK_SHIFT_CODE,
            scan_code: 42,
            flags: base_flags,
        });
    }
    if modifiers.ctrl {
        out.push(TranslatedWin32Input::Keyboard {
            virtual_key: VK_CONTROL_CODE,
            scan_code: 29,
            flags: base_flags,
        });
    }
    if modifiers.alt {
        out.push(TranslatedWin32Input::Keyboard {
            virtual_key: VK_MENU_CODE,
            scan_code: 56,
            flags: base_flags,
        });
    }
    if modifiers.logo {
        out.push(TranslatedWin32Input::Keyboard {
            virtual_key: VK_LWIN_CODE,
            scan_code: 0x5B,
            flags: base_flags | WIN32_KEYEVENTF_EXTENDEDKEY,
        });
    }

    out
}

/// Translates a wire [`InputEvent`] into a sequence of validated [`TranslatedWin32Input`] structs
/// bounded to `screen_width` x `screen_height`.
pub fn translate_input_event_to_win32(
    event: &InputEvent,
    screen_width: u32,
    screen_height: u32,
) -> Result<Vec<TranslatedWin32Input>> {
    match event {
        InputEvent::MouseMove {
            x, y, is_absolute, ..
        } => {
            if *is_absolute {
                let (dx, dy) = normalize_win32_absolute_coords(*x, *y, screen_width, screen_height);
                Ok(vec![TranslatedWin32Input::Mouse {
                    dx,
                    dy,
                    mouse_data: 0,
                    flags: WIN32_MOUSEEVENTF_MOVE
                        | WIN32_MOUSEEVENTF_ABSOLUTE
                        | WIN32_MOUSEEVENTF_VIRTUALDESK,
                }])
            } else {
                let dx = if x.is_finite() { *x as i32 } else { 0 };
                let dy = if y.is_finite() { *y as i32 } else { 0 };
                Ok(vec![TranslatedWin32Input::Mouse {
                    dx,
                    dy,
                    mouse_data: 0,
                    flags: WIN32_MOUSEEVENTF_MOVE,
                }])
            }
        }
        InputEvent::MouseButton {
            button,
            state,
            x,
            y,
        } => {
            let (dx, dy) = normalize_win32_absolute_coords(*x, *y, screen_width, screen_height);
            let is_pressed = *state == ElementState::Pressed;

            let (btn_flag, mouse_data) = match button {
                MouseButtonCode::Left => (
                    if is_pressed {
                        WIN32_MOUSEEVENTF_LEFTDOWN
                    } else {
                        WIN32_MOUSEEVENTF_LEFTUP
                    },
                    0,
                ),
                MouseButtonCode::Right => (
                    if is_pressed {
                        WIN32_MOUSEEVENTF_RIGHTDOWN
                    } else {
                        WIN32_MOUSEEVENTF_RIGHTUP
                    },
                    0,
                ),
                MouseButtonCode::Middle => (
                    if is_pressed {
                        WIN32_MOUSEEVENTF_MIDDLEDOWN
                    } else {
                        WIN32_MOUSEEVENTF_MIDDLEUP
                    },
                    0,
                ),
                MouseButtonCode::Back => (
                    if is_pressed {
                        WIN32_MOUSEEVENTF_XDOWN
                    } else {
                        WIN32_MOUSEEVENTF_XUP
                    },
                    WIN32_XBUTTON1,
                ),
                MouseButtonCode::Forward => (
                    if is_pressed {
                        WIN32_MOUSEEVENTF_XDOWN
                    } else {
                        WIN32_MOUSEEVENTF_XUP
                    },
                    WIN32_XBUTTON2,
                ),
                MouseButtonCode::Other(code) => (
                    if is_pressed {
                        WIN32_MOUSEEVENTF_XDOWN
                    } else {
                        WIN32_MOUSEEVENTF_XUP
                    },
                    if u32::from(*code) == WIN32_XBUTTON2 {
                        WIN32_XBUTTON2
                    } else {
                        WIN32_XBUTTON1
                    },
                ),
            };

            Ok(vec![TranslatedWin32Input::Mouse {
                dx,
                dy,
                mouse_data,
                flags: WIN32_MOUSEEVENTF_MOVE
                    | WIN32_MOUSEEVENTF_ABSOLUTE
                    | WIN32_MOUSEEVENTF_VIRTUALDESK
                    | btn_flag,
            }])
        }
        InputEvent::MouseWheel {
            delta_x,
            delta_y,
            unit,
        } => {
            let scale = match unit {
                WheelUnit::Line => f32::from(WIN32_WHEEL_DELTA as i16),
                WheelUnit::Pixel => 1.0,
            };
            let mut inputs = Vec::new();
            let scaled_y = if delta_y.is_finite() {
                (*delta_y * scale).round() as i32
            } else {
                0
            };
            let scaled_x = if delta_x.is_finite() {
                (*delta_x * scale).round() as i32
            } else {
                0
            };

            if scaled_y != 0 {
                inputs.push(TranslatedWin32Input::Mouse {
                    dx: 0,
                    dy: 0,
                    mouse_data: scaled_y as u32,
                    flags: WIN32_MOUSEEVENTF_WHEEL,
                });
            }
            if scaled_x != 0 {
                inputs.push(TranslatedWin32Input::Mouse {
                    dx: 0,
                    dy: 0,
                    mouse_data: scaled_x as u32,
                    flags: WIN32_MOUSEEVENTF_HWHEEL,
                });
            }
            if inputs.is_empty() {
                inputs.push(TranslatedWin32Input::Mouse {
                    dx: 0,
                    dy: 0,
                    mouse_data: 0,
                    flags: WIN32_MOUSEEVENTF_WHEEL,
                });
            }
            Ok(inputs)
        }
        InputEvent::KeyEvent {
            key_code,
            scancode,
            state,
            modifiers,
        } => {
            let (virtual_key, scan_code, is_extended) =
                map_scancode_to_win32_vk(*key_code, *scancode);
            let is_released = *state == ElementState::Released;

            let mut flags = if scan_code != 0 {
                WIN32_KEYEVENTF_SCANCODE
            } else {
                0
            };
            if is_extended {
                flags |= WIN32_KEYEVENTF_EXTENDEDKEY;
            }
            if is_released {
                flags |= WIN32_KEYEVENTF_KEYUP;
            }

            let mut sequence = Vec::new();
            if !is_released {
                sequence.extend(encode_win32_modifier_transitions(*modifiers, false));
            }
            sequence.push(TranslatedWin32Input::Keyboard {
                virtual_key,
                scan_code,
                flags,
            });
            if is_released {
                sequence.extend(encode_win32_modifier_transitions(*modifiers, true));
            }
            Ok(sequence)
        }
        InputEvent::TouchEvent { phase, x, y, .. } => {
            let (dx, dy) = normalize_win32_absolute_coords(*x, *y, screen_width, screen_height);
            let phase_flag = match phase {
                TouchPhase::Started => WIN32_MOUSEEVENTF_LEFTDOWN,
                TouchPhase::Moved => 0,
                TouchPhase::Ended | TouchPhase::Cancelled => WIN32_MOUSEEVENTF_LEFTUP,
            };
            Ok(vec![TranslatedWin32Input::Mouse {
                dx,
                dy,
                mouse_data: 0,
                flags: WIN32_MOUSEEVENTF_MOVE
                    | WIN32_MOUSEEVENTF_ABSOLUTE
                    | WIN32_MOUSEEVENTF_VIRTUALDESK
                    | phase_flag,
            }])
        }
        InputEvent::TextInput { text } => {
            let mut sequence = Vec::with_capacity(text.len() * 2);
            for unit in text.encode_utf16() {
                sequence.push(TranslatedWin32Input::Keyboard {
                    virtual_key: 0,
                    scan_code: unit,
                    flags: WIN32_KEYEVENTF_UNICODE,
                });
                sequence.push(TranslatedWin32Input::Keyboard {
                    virtual_key: 0,
                    scan_code: unit,
                    flags: WIN32_KEYEVENTF_UNICODE | WIN32_KEYEVENTF_KEYUP,
                });
            }
            Ok(sequence)
        }
    }
}

/// Windows Win32 `SendInput` Native Input Injector (`INPUT_MOUSE` / `INPUT_KEYBOARD`).
pub struct WindowsSendInputInjector {
    screen_width: u32,
    screen_height: u32,
    live_injection_enabled: bool,
    injected_count: AtomicU64,
}

impl WindowsSendInputInjector {
    /// Creates a new [`WindowsSendInputInjector`] for a display of `screen_width` x `screen_height`.
    /// By default, live OS `SendInput` execution is disabled during unit tests unless explicitly enabled.
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            screen_width: screen_width.max(1),
            screen_height: screen_height.max(1),
            live_injection_enabled: false,
            injected_count: AtomicU64::new(0),
        }
    }

    /// Creates a new [`WindowsSendInputInjector`] with live Win32 `SendInput` enabled on Windows.
    pub fn new_live(screen_width: u32, screen_height: u32) -> Self {
        Self {
            screen_width: screen_width.max(1),
            screen_height: screen_height.max(1),
            live_injection_enabled: cfg!(target_os = "windows"),
            injected_count: AtomicU64::new(0),
        }
    }

    /// Returns the total number of Win32 `INPUT` packets translated and processed by this injector.
    pub fn injected_count(&self) -> u64 {
        self.injected_count.load(Ordering::SeqCst)
    }

    #[cfg(target_os = "windows")]
    fn dispatch_win32_send_input(&self, translated: &[TranslatedWin32Input]) -> Result<()> {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
            MOUSE_EVENT_FLAGS, MOUSEINPUT, SendInput, VIRTUAL_KEY,
        };

        if translated.is_empty() {
            return Ok(());
        }

        let mut raw_inputs = Vec::with_capacity(translated.len());
        for item in translated {
            match item {
                TranslatedWin32Input::Mouse {
                    dx,
                    dy,
                    mouse_data,
                    flags,
                } => {
                    raw_inputs.push(INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 {
                            mi: MOUSEINPUT {
                                dx: *dx,
                                dy: *dy,
                                mouseData: *mouse_data,
                                dwFlags: MOUSE_EVENT_FLAGS(*flags),
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    });
                }
                TranslatedWin32Input::Keyboard {
                    virtual_key,
                    scan_code,
                    flags,
                } => {
                    raw_inputs.push(INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VIRTUAL_KEY(*virtual_key),
                                wScan: *scan_code,
                                dwFlags: KEYBD_EVENT_FLAGS(*flags),
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    });
                }
            }
        }

        let cb_size = core::mem::size_of::<INPUT>() as i32;
        // SAFETY: `raw_inputs` is a contiguous slice of properly initialized Win32 `INPUT`
        // structures (`INPUT_MOUSE` and `INPUT_KEYBOARD` union variants matched to `r#type`),
        // and `cb_size` is the exact byte size of `INPUT` (`core::mem::size_of::<INPUT>()`).
        // `SendInput` synchronously copies the array into the kernel input stream before returning.
        let inserted = unsafe { SendInput(&raw_inputs, cb_size) };
        if inserted == 0 {
            bail!(
                "Win32 SendInput failed to inject any events (0 of {} inserted; possible UIPI or secure desktop block)",
                raw_inputs.len()
            );
        }
        debug!(
            requested = raw_inputs.len(),
            inserted, "Dispatched Win32 SendInput event batch"
        );

        Ok(())
    }
}

impl Default for WindowsSendInputInjector {
    fn default() -> Self {
        Self::new(2560, 1440)
    }
}

impl HostInputInjector for WindowsSendInputInjector {
    fn inject_input(&self, event: &InputEvent) -> Result<()> {
        let translated =
            translate_input_event_to_win32(event, self.screen_width, self.screen_height)?;
        self.injected_count
            .fetch_add(translated.len() as u64, Ordering::SeqCst);

        #[cfg(target_os = "windows")]
        if self.live_injection_enabled {
            self.dispatch_win32_send_input(&translated)?;
        }

        debug!(
            count = translated.len(),
            live = self.live_injection_enabled,
            "Windows SendInput event batch processed"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dxgi_format_mapping_and_surface_validation() {
        assert_eq!(
            map_dxgi_format_to_wire(DXGI_FORMAT_B8G8R8A8_UNORM_RAW),
            Some(PixelFormat::Bgra8)
        );
        assert_eq!(
            map_dxgi_format_to_wire(DXGI_FORMAT_B8G8R8A8_UNORM_SRGB_RAW),
            Some(PixelFormat::Bgra8)
        );
        assert_eq!(
            map_dxgi_format_to_wire(DXGI_FORMAT_R8G8B8A8_UNORM_RAW),
            Some(PixelFormat::Rgba8)
        );
        assert_eq!(
            map_dxgi_format_to_wire(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB_RAW),
            Some(PixelFormat::Rgba8)
        );
        assert_eq!(
            map_dxgi_format_to_wire(DXGI_FORMAT_NV12_RAW),
            Some(PixelFormat::NV12)
        );
        assert_eq!(map_dxgi_format_to_wire(0), None);

        let bgra = WindowsDxgiSurfaceDescriptor::new_bgra8(0, 2560, 1440).unwrap();
        assert_eq!(bgra.output_index, 0);
        assert_eq!(bgra.pixel_format, PixelFormat::Bgra8);
        assert!(bgra.is_shared_nt_handle);
        assert_eq!(bgra.subresources.len(), 1);
        assert_eq!(bgra.subresources[0].row_pitch, 2560 * 4);
        assert_eq!(bgra.subresources[0].depth_pitch, 2560 * 4 * 1440);

        let rgba = WindowsDxgiSurfaceDescriptor::new_rgba8(1, 1920, 1080).unwrap();
        assert_eq!(rgba.pixel_format, PixelFormat::Rgba8);
        assert_eq!(rgba.subresources[0].row_pitch, 1920 * 4);

        let nv12 = WindowsDxgiSurfaceDescriptor::new_nv12(0, 1920, 1080).unwrap();
        assert_eq!(nv12.pixel_format, PixelFormat::NV12);
        assert_eq!(nv12.subresources.len(), 2);
        assert_eq!(nv12.subresources[0].row_pitch, 1920);
        assert_eq!(nv12.subresources[1].row_pitch, 1920);

        // Zero dimensions should fail
        assert!(WindowsDxgiSurfaceDescriptor::new_bgra8(0, 0, 1080).is_err());
        assert!(WindowsDxgiSurfaceDescriptor::new_bgra8(0, 1920, 0).is_err());

        // Under-sized row_pitch should fail
        let bad_pitch = WindowsDxgiSurfaceDescriptor::new(
            0,
            1920,
            1080,
            DXGI_FORMAT_B8G8R8A8_UNORM_RAW,
            true,
            false,
            vec![DxgiSubresourceLayout {
                subresource_index: 0,
                width: 1920,
                height: 1080,
                row_pitch: 1024,
                depth_pitch: 1024 * 1080,
                offset: 0,
            }],
        );
        assert!(bad_pitch.is_err());

        // Single-plane NV12 should fail
        let bad_nv12 = WindowsDxgiSurfaceDescriptor::new(
            0,
            1920,
            1080,
            DXGI_FORMAT_NV12_RAW,
            true,
            false,
            vec![DxgiSubresourceLayout {
                subresource_index: 0,
                width: 1920,
                height: 1080,
                row_pitch: 1920,
                depth_pitch: 1920 * 1080,
                offset: 0,
            }],
        );
        assert!(bad_nv12.is_err());

        // NV12 with under-sized UV chroma row_pitch or overlapping offset should fail
        let bad_nv12_uv_pitch = WindowsDxgiSurfaceDescriptor::new(
            0,
            1920,
            1080,
            DXGI_FORMAT_NV12_RAW,
            true,
            false,
            vec![
                DxgiSubresourceLayout {
                    subresource_index: 0,
                    width: 1920,
                    height: 1080,
                    row_pitch: 1920,
                    depth_pitch: 1920 * 1080,
                    offset: 0,
                },
                DxgiSubresourceLayout {
                    subresource_index: 1,
                    width: 960,
                    height: 540,
                    row_pitch: 960, // too small: UV interleaved requires >= 1920
                    depth_pitch: 960 * 540,
                    offset: 1920 * 1080,
                },
            ],
        );
        assert!(bad_nv12_uv_pitch.is_err());

        let bad_nv12_uv_offset = WindowsDxgiSurfaceDescriptor::new(
            0,
            1920,
            1080,
            DXGI_FORMAT_NV12_RAW,
            true,
            false,
            vec![
                DxgiSubresourceLayout {
                    subresource_index: 0,
                    width: 1920,
                    height: 1080,
                    row_pitch: 1920,
                    depth_pitch: 1920 * 1080,
                    offset: 0,
                },
                DxgiSubresourceLayout {
                    subresource_index: 1,
                    width: 960,
                    height: 540,
                    row_pitch: 1920,
                    depth_pitch: 1920 * 540,
                    offset: 1024, // overlaps Y luma plane!
                },
            ],
        );
        assert!(bad_nv12_uv_offset.is_err());
    }

    #[test]
    fn test_windows_dxgi_capturer_headless_fallback() {
        assert!(WindowsDxgiCapturer::new(1, 0, 1440).is_err());

        let mut capturer = WindowsDxgiCapturer::new(1, 2560, 1440).unwrap();
        assert_eq!(capturer.backend_type(), CaptureBackendType::WindowsDxgi);
        assert_eq!(
            capturer.dxgi_interfaces(),
            ("IDXGIOutputDuplication", "ID3D11Texture2D")
        );

        let monitors = capturer.enumerate_monitors().unwrap();
        assert_eq!(monitors.len(), 1);
        assert_eq!(monitors[0].monitor_id, 1);
        assert!(monitors[0].width >= 1);
        assert!(monitors[0].height >= 1);
        assert!(monitors[0].is_primary);

        let f1 = capturer.capture_next_frame().unwrap();
        assert_eq!(f1.frame_index, 1);
        assert_eq!(f1.pixel_format, PixelFormat::Bgra8);
        assert!(f1.is_dmabuf_or_metal_handle);

        let f2 = capturer.capture_next_frame().unwrap();
        assert_eq!(f2.frame_index, 2);
    }

    #[test]
    fn test_win32_sendinput_translation_and_injection() {
        let injector = WindowsSendInputInjector::new(1920, 1080);
        assert_eq!(injector.injected_count(), 0);

        // 1. Absolute MouseMove normalization to 0..=65535 + clamping of out-of-bounds/NaN
        assert_eq!(
            normalize_win32_absolute_coords(0.0, 0.0, 1920, 1080),
            (0, 0)
        );
        assert_eq!(
            normalize_win32_absolute_coords(1919.0, 1079.0, 1920, 1080),
            (65535, 65535)
        );
        assert_eq!(
            normalize_win32_absolute_coords(-200.0, 5000.0, 1920, 1080),
            (0, 65535)
        );
        assert_eq!(
            normalize_win32_absolute_coords(f64::NAN, f64::INFINITY, 1920, 1080),
            (0, 0)
        );

        // Multi-monitor negative origin virtual desktop normalization:
        // Secondary monitor to the left of primary: origin (-1920, -540), total virtual desktop 3840x2160
        assert_eq!(
            normalize_win32_virtual_desktop_coords(-1920.0, -540.0, -1920, -540, 3840, 2160),
            (0, 0)
        );
        assert_eq!(
            normalize_win32_virtual_desktop_coords(1919.0, 1619.0, -1920, -540, 3840, 2160),
            (65535, 65535)
        );

        let move_ev = InputEvent::MouseMove {
            monitor_id: 1,
            x: 1919.0,
            y: 1079.0,
            is_absolute: true,
        };
        let translated_move = translate_input_event_to_win32(&move_ev, 1920, 1080).unwrap();
        assert_eq!(
            translated_move,
            vec![TranslatedWin32Input::Mouse {
                dx: 65535,
                dy: 65535,
                mouse_data: 0,
                flags: WIN32_MOUSEEVENTF_MOVE
                    | WIN32_MOUSEEVENTF_ABSOLUTE
                    | WIN32_MOUSEEVENTF_VIRTUALDESK,
            }]
        );
        injector.inject_input(&move_ev).unwrap();

        // 2. Back XButton press & MouseButtonCode::Other XBUTTON sanitization
        let btn_ev = InputEvent::MouseButton {
            button: MouseButtonCode::Back,
            state: ElementState::Pressed,
            x: 0.0,
            y: 0.0,
        };
        let translated_btn = translate_input_event_to_win32(&btn_ev, 1920, 1080).unwrap();
        assert_eq!(
            translated_btn,
            vec![TranslatedWin32Input::Mouse {
                dx: 0,
                dy: 0,
                mouse_data: WIN32_XBUTTON1,
                flags: WIN32_MOUSEEVENTF_MOVE
                    | WIN32_MOUSEEVENTF_ABSOLUTE
                    | WIN32_MOUSEEVENTF_VIRTUALDESK
                    | WIN32_MOUSEEVENTF_XDOWN,
            }]
        );
        injector.inject_input(&btn_ev).unwrap();

        let other_btn_ev = InputEvent::MouseButton {
            button: MouseButtonCode::Other(99),
            state: ElementState::Released,
            x: 0.0,
            y: 0.0,
        };
        let translated_other_btn =
            translate_input_event_to_win32(&other_btn_ev, 1920, 1080).unwrap();
        assert_eq!(
            translated_other_btn[0],
            TranslatedWin32Input::Mouse {
                dx: 0,
                dy: 0,
                mouse_data: WIN32_XBUTTON1,
                flags: WIN32_MOUSEEVENTF_MOVE
                    | WIN32_MOUSEEVENTF_ABSOLUTE
                    | WIN32_MOUSEEVENTF_VIRTUALDESK
                    | WIN32_MOUSEEVENTF_XUP,
            }
        );

        // 3. MouseWheel line unit scaling by WHEEL_DELTA (120)
        let wheel_ev = InputEvent::MouseWheel {
            delta_x: 1.0,
            delta_y: -2.0,
            unit: WheelUnit::Line,
        };
        let translated_wheel = translate_input_event_to_win32(&wheel_ev, 1920, 1080).unwrap();
        assert_eq!(translated_wheel.len(), 2);
        assert_eq!(
            translated_wheel[0],
            TranslatedWin32Input::Mouse {
                dx: 0,
                dy: 0,
                mouse_data: (-240i32) as u32,
                flags: WIN32_MOUSEEVENTF_WHEEL,
            }
        );
        assert_eq!(
            translated_wheel[1],
            TranslatedWin32Input::Mouse {
                dx: 0,
                dy: 0,
                mouse_data: 120,
                flags: WIN32_MOUSEEVENTF_HWHEEL,
            }
        );
        injector.inject_input(&wheel_ev).unwrap();

        // 4. KeyEvent with Ctrl+Shift modifiers and extended key handling
        let key_ev = InputEvent::KeyEvent {
            key_code: 0,
            scancode: 111, // Delete (extended)
            state: ElementState::Pressed,
            modifiers: KeyModifiers {
                shift: true,
                ctrl: true,
                alt: false,
                logo: false,
            },
        };
        let translated_key = translate_input_event_to_win32(&key_ev, 1920, 1080).unwrap();
        assert_eq!(translated_key.len(), 3); // Shift down, Ctrl down, Delete down
        assert_eq!(
            translated_key[2],
            TranslatedWin32Input::Keyboard {
                virtual_key: 0x2E, // VK_DELETE
                scan_code: 111,
                flags: WIN32_KEYEVENTF_SCANCODE | WIN32_KEYEVENTF_EXTENDEDKEY,
            }
        );
        injector.inject_input(&key_ev).unwrap();

        // 5. Unicode TextInput (down + up per UTF-16 code unit)
        let text_ev = InputEvent::TextInput {
            text: "Win".to_string(),
        };
        let translated_text = translate_input_event_to_win32(&text_ev, 1920, 1080).unwrap();
        assert_eq!(translated_text.len(), 6);
        assert_eq!(
            translated_text[0],
            TranslatedWin32Input::Keyboard {
                virtual_key: 0,
                scan_code: b'W' as u16,
                flags: WIN32_KEYEVENTF_UNICODE,
            }
        );
        assert_eq!(
            translated_text[1],
            TranslatedWin32Input::Keyboard {
                virtual_key: 0,
                scan_code: b'W' as u16,
                flags: WIN32_KEYEVENTF_UNICODE | WIN32_KEYEVENTF_KEYUP,
            }
        );
        injector.inject_input(&text_ev).unwrap();

        assert_eq!(injector.injected_count(), 1 + 1 + 2 + 3 + 6);
    }
}
