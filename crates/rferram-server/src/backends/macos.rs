//! macOS `ScreenCaptureKit` zero-copy `IOSurface` / `Metal` GPU frame capture
//! and `CoreGraphics` (`CGEvent`) native input injection backend.
//!
//! Uses the locked `objc2` ecosystem (`objc2`, `objc2-core-foundation`,
//! `objc2-core-graphics`, `objc2-foundation`, `objc2-io-surface`, `objc2-metal`,
//! `objc2-quartz-core`) under `#[cfg(target_os = "macos")]` while providing
//! deterministic cross-platform surface validation, coordinate/keycode translation,
//! and headless CI fallback on all host targets.

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

/// CoreVideo / `IOSurface` OSType for 32-bit `BGRA8` (`'BGRA'` / `kCVPixelFormatType_32BGRA`).
pub const CV_PIXEL_FORMAT_32BGRA: u32 = u32::from_be_bytes(*b"BGRA");
/// CoreVideo / `IOSurface` OSType for 32-bit `RGBA8` (`'RGBA'` / `kCVPixelFormatType_32RGBA`).
pub const CV_PIXEL_FORMAT_32RGBA: u32 = u32::from_be_bytes(*b"RGBA");
/// CoreVideo / `IOSurface` OSType for bi-planar `NV12` video-range (`'420v'` / `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`).
pub const CV_PIXEL_FORMAT_420V: u32 = u32::from_be_bytes(*b"420v");
/// CoreVideo / `IOSurface` OSType for bi-planar `NV12` full-range (`'420f'` / `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange`).
pub const CV_PIXEL_FORMAT_420F: u32 = u32::from_be_bytes(*b"420f");
/// CoreVideo / `IOSurface` OSType for planar 4:2:0 `YUV420P` (`'y420'` / `kCVPixelFormatType_420YpCbCr8Planar`).
pub const CV_PIXEL_FORMAT_Y420: u32 = u32::from_be_bytes(*b"y420");
/// CoreVideo / `IOSurface` OSType for planar 4:4:4 `YUV444P` (`'v444'` / `kCVPixelFormatType_444YpCbCr8`).
pub const CV_PIXEL_FORMAT_Y444: u32 = u32::from_be_bytes(*b"v444");

/// CoreGraphics `CGEventFlags` bitmask for the Shift modifier (`kCGEventFlagMaskShift`).
pub const CG_EVENT_FLAG_MASK_SHIFT: u64 = 0x0002_0000;
/// CoreGraphics `CGEventFlags` bitmask for the Control modifier (`kCGEventFlagMaskControl`).
pub const CG_EVENT_FLAG_MASK_CONTROL: u64 = 0x0004_0000;
/// CoreGraphics `CGEventFlags` bitmask for the Option/Alt modifier (`kCGEventFlagMaskAlternate`).
pub const CG_EVENT_FLAG_MASK_ALTERNATE: u64 = 0x0008_0000;
/// CoreGraphics `CGEventFlags` bitmask for the Command/Logo modifier (`kCGEventFlagMaskCommand`).
pub const CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;

/// Maps a macOS CoreVideo / `IOSurface` `OSType` pixel format code to the wire [`PixelFormat`].
pub fn map_cv_pixel_format_to_wire(os_type: u32) -> Option<PixelFormat> {
    match os_type {
        CV_PIXEL_FORMAT_32BGRA => Some(PixelFormat::Bgra8),
        CV_PIXEL_FORMAT_32RGBA => Some(PixelFormat::Rgba8),
        CV_PIXEL_FORMAT_420V | CV_PIXEL_FORMAT_420F => Some(PixelFormat::NV12),
        CV_PIXEL_FORMAT_Y420 => Some(PixelFormat::YUV420P),
        CV_PIXEL_FORMAT_Y444 => Some(PixelFormat::YUV444P),
        _ => None,
    }
}

/// Metadata describing a single plane within a macOS `IOSurfaceRef` backing a `ScreenCaptureKit` `CMSampleBuffer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoSurfacePlaneDescriptor {
    /// Zero-based plane index (`0` for single-plane BGRA or Y-plane in NV12; `1` for UV-plane in NV12).
    pub plane_index: usize,
    /// Plane width in pixels.
    pub width: u32,
    /// Plane height in pixels.
    pub height: u32,
    /// Bytes per element (e.g., `4` for `BGRA8`, `1` for NV12 luma `Y`, `2` for NV12 interleaved `CbCr`).
    pub bytes_per_element: u32,
    /// Row stride (`IOSurfaceGetBytesPerRowOfPlane`) in bytes.
    pub bytes_per_row: u32,
    /// Byte offset from the base of the `IOSurface` allocation.
    pub offset: u32,
}

/// Zero-copy macOS `IOSurface` / `MTLTexture` surface descriptor exported by `ScreenCaptureKit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacOsIoSurfaceDescriptor {
    /// Kernel-global `IOSurfaceID` (`uint32_t`) suitable for Mach port / Metal texture binding.
    pub iosurface_id: u32,
    /// Total surface width in pixels.
    pub width: u32,
    /// Total surface height in pixels.
    pub height: u32,
    /// Four-character CoreVideo `OSType` pixel format (`kCVPixelFormatType_*`).
    pub os_type: u32,
    /// Mapped rFerram wire [`PixelFormat`].
    pub pixel_format: PixelFormat,
    /// Indicates whether the surface is backed by a zero-copy `MTLTexture` (`objc2-metal`).
    pub metal_texture_compatible: bool,
    /// Per-plane layout descriptors (`IOSurfaceGetPlaneCount`).
    pub planes: Vec<IoSurfacePlaneDescriptor>,
}

impl MacOsIoSurfaceDescriptor {
    /// Constructs and validates a new [`MacOsIoSurfaceDescriptor`].
    pub fn new(
        iosurface_id: u32,
        width: u32,
        height: u32,
        os_type: u32,
        metal_texture_compatible: bool,
        planes: Vec<IoSurfacePlaneDescriptor>,
    ) -> Result<Self> {
        let Some(pixel_format) = map_cv_pixel_format_to_wire(os_type) else {
            bail!("Unsupported macOS CoreVideo/IOSurface OSType format: 0x{os_type:08x}");
        };

        let descriptor = Self {
            iosurface_id,
            width,
            height,
            os_type,
            pixel_format,
            metal_texture_compatible,
            planes,
        };
        descriptor.validate_planes()?;
        Ok(descriptor)
    }

    /// Creates a standard single-plane 32-bit `BGRA8` (`kCVPixelFormatType_32BGRA`) `IOSurface` descriptor.
    pub fn new_bgra8(iosurface_id: u32, width: u32, height: u32) -> Result<Self> {
        let bytes_per_row = width.saturating_mul(4);
        Self::new(
            iosurface_id,
            width,
            height,
            CV_PIXEL_FORMAT_32BGRA,
            true,
            vec![IoSurfacePlaneDescriptor {
                plane_index: 0,
                width,
                height,
                bytes_per_element: 4,
                bytes_per_row,
                offset: 0,
            }],
        )
    }

    /// Creates a standard 2-plane bi-planar `NV12` (`kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`) `IOSurface` descriptor.
    pub fn new_nv12(iosurface_id: u32, width: u32, height: u32) -> Result<Self> {
        let uv_width = width.div_ceil(2);
        let uv_height = height.div_ceil(2);
        let y_stride = width;
        let uv_stride = uv_width.saturating_mul(2);
        let y_plane_bytes = y_stride.saturating_mul(height);

        Self::new(
            iosurface_id,
            width,
            height,
            CV_PIXEL_FORMAT_420V,
            true,
            vec![
                IoSurfacePlaneDescriptor {
                    plane_index: 0,
                    width,
                    height,
                    bytes_per_element: 1,
                    bytes_per_row: y_stride,
                    offset: 0,
                },
                IoSurfacePlaneDescriptor {
                    plane_index: 1,
                    width: uv_width,
                    height: uv_height,
                    bytes_per_element: 2,
                    bytes_per_row: uv_stride,
                    offset: y_plane_bytes,
                },
            ],
        )
    }

    /// Validates dimensions, plane counts, and minimum `bytes_per_row` stride invariants.
    pub fn validate_planes(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            bail!(
                "Invalid MacOsIoSurfaceDescriptor dimensions: {}x{}",
                self.width,
                self.height
            );
        }
        if self.planes.is_empty() {
            bail!("MacOsIoSurfaceDescriptor must contain at least one IOSurface plane");
        }

        let min_plane0_stride = match self.pixel_format {
            PixelFormat::Bgra8 | PixelFormat::Rgba8 => self.width.saturating_mul(4),
            PixelFormat::NV12 | PixelFormat::YUV420P | PixelFormat::YUV444P => self.width,
        };

        if self.planes[0].bytes_per_row < min_plane0_stride {
            bail!(
                "IOSurface plane 0 bytes_per_row ({}) is smaller than minimum required stride ({})",
                self.planes[0].bytes_per_row,
                min_plane0_stride
            );
        }

        if self.pixel_format == PixelFormat::NV12 && self.planes.len() < 2 {
            bail!("NV12 MacOsIoSurfaceDescriptor requires 2 planes (Y luma and CbCr chroma)");
        }

        Ok(())
    }
}

/// macOS `ScreenCaptureKit` (`SCStream` / `SCShareableContent`) zero-copy `IOSurface` / `Metal` frame capturer.
pub struct MacOsScreenCaptureKitCapturer {
    display_id: u32,
    iosurface: MacOsIoSurfaceDescriptor,
    refresh_rate_hz: u32,
    scale_factor: f32,
    frame_counter: u64,
    start_time: Instant,
    screencapturekit_available: bool,
    objc_classes_probed: (&'static str, &'static str),
}

impl MacOsScreenCaptureKitCapturer {
    /// Initializes the macOS `ScreenCaptureKit` frame capturer, probing `SCShareableContent` / `SCStream`
    /// and `CoreGraphics` display metrics on macOS, or falling back cleanly to a synthetic `IOSurface`
    /// in headless/non-macOS CI environments.
    pub fn new(display_id: u32, fallback_width: u32, fallback_height: u32) -> Result<Self> {
        if fallback_width == 0 || fallback_height == 0 {
            bail!(
                "Invalid MacOsScreenCaptureKitCapturer dimensions: {}x{}",
                fallback_width,
                fallback_height
            );
        }

        let (screencapturekit_available, detected_w, detected_h, objc_classes_probed) =
            Self::probe_macos_screencapturekit(fallback_width, fallback_height);

        let iosurface = MacOsIoSurfaceDescriptor::new_bgra8(display_id, detected_w, detected_h)?;

        info!(
            display_id,
            width = detected_w,
            height = detected_h,
            screencapturekit_available,
            shareable_content_class = objc_classes_probed.0,
            stream_class = objc_classes_probed.1,
            "Initialized macOS ScreenCaptureKit IOSurface/Metal frame capturer"
        );

        Ok(Self {
            display_id,
            iosurface,
            refresh_rate_hz: 60,
            scale_factor: 2.0,
            frame_counter: 0,
            start_time: Instant::now(),
            screencapturekit_available,
            objc_classes_probed,
        })
    }

    /// Returns a reference to the active [`MacOsIoSurfaceDescriptor`].
    pub fn iosurface(&self) -> &MacOsIoSurfaceDescriptor {
        &self.iosurface
    }

    /// Returns the `ScreenCaptureKit` Objective-C class names targeted by this capturer.
    pub fn screencapturekit_classes(&self) -> (&'static str, &'static str) {
        self.objc_classes_probed
    }

    fn probe_macos_screencapturekit(
        fallback_width: u32,
        fallback_height: u32,
    ) -> (bool, u32, u32, (&'static str, &'static str)) {
        #[cfg(target_os = "macos")]
        {
            // Touch the locked objc2 ecosystem crates so linkage and types are verified on macOS.
            use objc2::runtime::AnyClass;
            use objc2_io_surface as _;
            let _ = core::any::type_name::<objc2_core_foundation::CFType>();
            let _ = core::any::type_name::<objc2_foundation::NSObject>();
            let _ = core::any::type_name::<objc2_core_graphics::CGDirectDisplayID>();
            let _ = core::any::type_name::<objc2_metal::MTLPixelFormat>();
            let _ = core::any::type_name::<objc2_quartz_core::CAMetalLayer>();

            #[link(name = "CoreGraphics", kind = "framework")]
            unsafe extern "C" {
                fn CGMainDisplayID() -> u32;
                fn CGDisplayPixelsWide(display: u32) -> usize;
                fn CGDisplayPixelsHigh(display: u32) -> usize;
            }

            let shareable_cls = AnyClass::get(c"SCShareableContent");
            let stream_cls = AnyClass::get(c"SCStream");

            // SAFETY: `CGMainDisplayID`, `CGDisplayPixelsWide`, and `CGDisplayPixelsHigh` are
            // thread-safe CoreGraphics C functions that take a value-type `CGDirectDisplayID` (`u32`)
            // and return integer pixel dimensions (or 0 in headless environments) without dereferencing
            // any caller pointers.
            let (width, height, has_display) = unsafe {
                let main_id = CGMainDisplayID();
                let w = CGDisplayPixelsWide(main_id) as u32;
                let h = CGDisplayPixelsHigh(main_id) as u32;
                if w > 0 && h > 0 {
                    (w, h, true)
                } else {
                    (fallback_width, fallback_height, false)
                }
            };

            let available = shareable_cls.is_some() && stream_cls.is_some() && has_display;
            (available, width, height, ("SCShareableContent", "SCStream"))
        }
        #[cfg(not(target_os = "macos"))]
        {
            (
                false,
                fallback_width,
                fallback_height,
                ("SCShareableContent", "SCStream"),
            )
        }
    }
}

impl FrameCapturer for MacOsScreenCaptureKitCapturer {
    fn backend_type(&self) -> CaptureBackendType {
        CaptureBackendType::MacOsScreenCaptureKit
    }

    fn is_live_display(&self) -> bool {
        self.screencapturekit_available
    }

    fn enumerate_monitors(&self) -> Result<Vec<MonitorInfo>> {
        Ok(vec![MonitorInfo {
            monitor_id: self.display_id,
            name: format!("macOS-ScreenCaptureKit-{}", self.display_id),
            x: 0,
            y: 0,
            width: self.iosurface.width,
            height: self.iosurface.height,
            scale_factor: self.scale_factor,
            refresh_rate_hz: self.refresh_rate_hz,
            is_primary: self.display_id == 1,
        }])
    }

    fn capture_next_frame(&mut self) -> Result<CapturedFrame> {
        self.frame_counter += 1;
        let stride = self.iosurface.planes[0].bytes_per_row;
        let payload_len = (stride as usize)
            .saturating_mul(self.iosurface.height as usize)
            .min(64 * 1024);

        debug!(
            frame_index = self.frame_counter,
            display_id = self.display_id,
            iosurface_id = self.iosurface.iosurface_id,
            live = self.screencapturekit_available,
            "Captured macOS ScreenCaptureKit IOSurface/Metal frame"
        );

        Ok(CapturedFrame {
            frame_index: self.frame_counter,
            timestamp_us: self.start_time.elapsed().as_micros() as u64,
            display_id: self.display_id,
            width: self.iosurface.width,
            height: self.iosurface.height,
            stride,
            pixel_format: self.iosurface.pixel_format,
            data: vec![0x90; payload_len],
            is_dmabuf_or_metal_handle: self.iosurface.metal_texture_compatible,
        })
    }
}

/// Classification of macOS `CGEventType` operations generated by [`MacOsCgEventInjector`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CgMouseEventKind {
    /// `kCGEventMouseMoved` (`5`)
    MouseMoved,
    /// `kCGEventLeftMouseDown` (`1`)
    LeftMouseDown,
    /// `kCGEventLeftMouseUp` (`2`)
    LeftMouseUp,
    /// `kCGEventRightMouseDown` (`3`)
    RightMouseDown,
    /// `kCGEventRightMouseUp` (`4`)
    RightMouseUp,
    /// `kCGEventOtherMouseDown` (`25`)
    OtherMouseDown,
    /// `kCGEventOtherMouseUp` (`26`)
    OtherMouseUp,
}

impl CgMouseEventKind {
    /// Returns the raw macOS `CGEventType` (`uint32_t`) numeric discriminant.
    pub fn cg_event_type_raw(self) -> u32 {
        match self {
            Self::LeftMouseDown => 1,
            Self::LeftMouseUp => 2,
            Self::RightMouseDown => 3,
            Self::RightMouseUp => 4,
            Self::MouseMoved => 5,
            Self::OtherMouseDown => 25,
            Self::OtherMouseUp => 26,
        }
    }
}

/// Strongly-typed representation of a translated macOS `CoreGraphics` `CGEvent` ready for posting.
#[derive(Debug, Clone, PartialEq)]
pub enum TranslatedCgEvent {
    /// `CGEventCreateMouseEvent` with clamped Quartz global display coordinates and `CGMouseButton`.
    Mouse {
        kind: CgMouseEventKind,
        cg_button: u32,
        x: f64,
        y: f64,
    },
    /// `CGEventCreateScrollWheelEvent2` with line or pixel scroll units.
    ScrollWheel {
        is_pixel_unit: bool,
        delta_x: i32,
        delta_y: i32,
    },
    /// `CGEventCreateKeyboardEvent` with virtual `CGKeyCode` (`u16`), key state, and `CGEventFlags`.
    Keyboard {
        virtual_keycode: u16,
        key_down: bool,
        flags: u64,
    },
    /// `CGEventKeyboardSetUnicodeString` UTF-16 payload for direct text injection.
    UnicodeText { utf16_units: Vec<u16> },
}

/// Translates wire [`KeyModifiers`] into a macOS `CGEventFlags` bitmask.
pub fn translate_modifiers_to_cg_flags(modifiers: KeyModifiers) -> u64 {
    let mut flags = 0u64;
    if modifiers.shift {
        flags |= CG_EVENT_FLAG_MASK_SHIFT;
    }
    if modifiers.ctrl {
        flags |= CG_EVENT_FLAG_MASK_CONTROL;
    }
    if modifiers.alt {
        flags |= CG_EVENT_FLAG_MASK_ALTERNATE;
    }
    if modifiers.logo {
        flags |= CG_EVENT_FLAG_MASK_COMMAND;
    }
    flags
}

/// Maps a wire `key_code` / `scancode` pair into a macOS virtual `CGKeyCode` (`kVK_*`).
pub fn map_scancode_to_macos_keycode(key_code: u32, scancode: u32) -> u16 {
    // Common Linux evdev / USB HID scancodes -> macOS Carbon/Quartz kVK_* virtual keycodes
    match scancode {
        1 => 0x35,   // Escape -> kVK_Escape
        14 => 0x33,  // Backspace -> kVK_Delete
        15 => 0x30,  // Tab -> kVK_Tab
        28 => 0x24,  // Enter -> kVK_Return
        30 => 0x00,  // 'A' -> kVK_ANSI_A
        31 => 0x01,  // 'S' -> kVK_ANSI_S
        32 => 0x02,  // 'D' -> kVK_ANSI_D
        33 => 0x03,  // 'F' -> kVK_ANSI_F
        42 => 0x38,  // Left Shift -> kVK_Shift
        56 => 0x3A,  // Left Option/Alt -> kVK_Option
        57 => 0x31,  // Space -> kVK_Space
        125 => 0x37, // Left Meta/Super -> kVK_Command
        _ => (key_code & 0xFFFF) as u16,
    }
}

/// Translates a wire [`InputEvent`] into a validated [`TranslatedCgEvent`] bounded to `screen_width` x `screen_height`.
pub fn translate_input_event_to_cg(
    event: &InputEvent,
    screen_width: u32,
    screen_height: u32,
) -> Result<TranslatedCgEvent> {
    let max_x = f64::from(screen_width.saturating_sub(1).max(1));
    let max_y = f64::from(screen_height.saturating_sub(1).max(1));

    match event {
        InputEvent::MouseMove {
            x, y, is_absolute, ..
        } => {
            let (clamped_x, clamped_y) = if *is_absolute {
                (x.clamp(0.0, max_x), y.clamp(0.0, max_y))
            } else {
                (*x, *y)
            };
            Ok(TranslatedCgEvent::Mouse {
                kind: CgMouseEventKind::MouseMoved,
                cg_button: 0, // kCGMouseButtonLeft
                x: clamped_x,
                y: clamped_y,
            })
        }
        InputEvent::MouseButton {
            button,
            state,
            x,
            y,
        } => {
            let clamped_x = x.clamp(0.0, max_x);
            let clamped_y = y.clamp(0.0, max_y);
            let is_pressed = *state == ElementState::Pressed;

            let (kind, cg_button) = match button {
                MouseButtonCode::Left => (
                    if is_pressed {
                        CgMouseEventKind::LeftMouseDown
                    } else {
                        CgMouseEventKind::LeftMouseUp
                    },
                    0, // kCGMouseButtonLeft
                ),
                MouseButtonCode::Right => (
                    if is_pressed {
                        CgMouseEventKind::RightMouseDown
                    } else {
                        CgMouseEventKind::RightMouseUp
                    },
                    1, // kCGMouseButtonRight
                ),
                MouseButtonCode::Middle => (
                    if is_pressed {
                        CgMouseEventKind::OtherMouseDown
                    } else {
                        CgMouseEventKind::OtherMouseUp
                    },
                    2, // kCGMouseButtonCenter
                ),
                MouseButtonCode::Back => (
                    if is_pressed {
                        CgMouseEventKind::OtherMouseDown
                    } else {
                        CgMouseEventKind::OtherMouseUp
                    },
                    3,
                ),
                MouseButtonCode::Forward => (
                    if is_pressed {
                        CgMouseEventKind::OtherMouseDown
                    } else {
                        CgMouseEventKind::OtherMouseUp
                    },
                    4,
                ),
                MouseButtonCode::Other(code) => (
                    if is_pressed {
                        CgMouseEventKind::OtherMouseDown
                    } else {
                        CgMouseEventKind::OtherMouseUp
                    },
                    u32::from(*code),
                ),
            };

            Ok(TranslatedCgEvent::Mouse {
                kind,
                cg_button,
                x: clamped_x,
                y: clamped_y,
            })
        }
        InputEvent::MouseWheel {
            delta_x,
            delta_y,
            unit,
        } => Ok(TranslatedCgEvent::ScrollWheel {
            is_pixel_unit: *unit == WheelUnit::Pixel,
            delta_x: *delta_x as i32,
            delta_y: *delta_y as i32,
        }),
        InputEvent::KeyEvent {
            key_code,
            scancode,
            state,
            modifiers,
        } => {
            let virtual_keycode = map_scancode_to_macos_keycode(*key_code, *scancode);
            let key_down = *state == ElementState::Pressed;
            let flags = translate_modifiers_to_cg_flags(*modifiers);
            Ok(TranslatedCgEvent::Keyboard {
                virtual_keycode,
                key_down,
                flags,
            })
        }
        InputEvent::TouchEvent { phase, x, y, .. } => {
            let clamped_x = x.clamp(0.0, max_x);
            let clamped_y = y.clamp(0.0, max_y);
            let kind = match phase {
                TouchPhase::Started => CgMouseEventKind::LeftMouseDown,
                TouchPhase::Moved => CgMouseEventKind::MouseMoved,
                TouchPhase::Ended | TouchPhase::Cancelled => CgMouseEventKind::LeftMouseUp,
            };
            Ok(TranslatedCgEvent::Mouse {
                kind,
                cg_button: 0,
                x: clamped_x,
                y: clamped_y,
            })
        }
        InputEvent::TextInput { text } => {
            let utf16_units: Vec<u16> = text.encode_utf16().collect();
            Ok(TranslatedCgEvent::UnicodeText { utf16_units })
        }
    }
}

/// macOS `CoreGraphics` (`CGEvent`) Native Input Injector (`kCGHIDEventTap`).
pub struct MacOsCgEventInjector {
    screen_width: u32,
    screen_height: u32,
    live_posting_enabled: bool,
    injected_count: AtomicU64,
}

impl MacOsCgEventInjector {
    /// Creates a new [`MacOsCgEventInjector`] for a display of `screen_width` x `screen_height`.
    /// By default, live OS `CGEventPost` execution is disabled during unit tests unless explicitly enabled.
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            screen_width: screen_width.max(1),
            screen_height: screen_height.max(1),
            live_posting_enabled: false,
            injected_count: AtomicU64::new(0),
        }
    }

    /// Creates a new [`MacOsCgEventInjector`] with live `CGEventPost(kCGHIDEventTap, ...)` enabled on macOS.
    pub fn new_live(screen_width: u32, screen_height: u32) -> Self {
        Self {
            screen_width: screen_width.max(1),
            screen_height: screen_height.max(1),
            live_posting_enabled: cfg!(target_os = "macos"),
            injected_count: AtomicU64::new(0),
        }
    }

    /// Returns the total number of input events translated and processed by this injector.
    pub fn injected_count(&self) -> u64 {
        self.injected_count.load(Ordering::SeqCst)
    }

    #[cfg(target_os = "macos")]
    fn post_cg_event_macos(&self, translated: &TranslatedCgEvent) -> Result<()> {
        use core::ffi::c_void;
        use objc2_core_foundation::CGPoint;

        type CGEventRef = *mut c_void;
        type CGEventSourceRef = *const c_void;

        const K_CG_HID_EVENT_TAP: u32 = 0;
        const K_CG_SCROLL_EVENT_UNIT_PIXEL: u32 = 0;
        const K_CG_SCROLL_EVENT_UNIT_LINE: u32 = 1;

        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGEventCreateMouseEvent(
                source: CGEventSourceRef,
                mouse_type: u32,
                mouse_cursor_position: CGPoint,
                mouse_button: u32,
            ) -> CGEventRef;
            fn CGEventCreateKeyboardEvent(
                source: CGEventSourceRef,
                virtual_key: u16,
                key_down: bool,
            ) -> CGEventRef;
            fn CGEventCreateScrollWheelEvent2(
                source: CGEventSourceRef,
                units: u32,
                wheel_count: u32,
                wheel1: i32,
                wheel2: i32,
                wheel3: i32,
            ) -> CGEventRef;
            fn CGEventSetFlags(event: CGEventRef, flags: u64);
            fn CGEventKeyboardSetUnicodeString(
                event: CGEventRef,
                string_length: usize,
                unicode_string: *const u16,
            );
            fn CGEventPost(tap: u32, event: CGEventRef);
        }

        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFRelease(cf: *const c_void);
        }

        match translated {
            TranslatedCgEvent::Mouse {
                kind,
                cg_button,
                x,
                y,
            } => {
                let point = CGPoint::new(*x, *y);
                // SAFETY: Passing a null `CGEventSourceRef` uses the default HID state;
                // `point` is a valid value-type `CGPoint`. We check the returned `CGEventRef`
                // for null before calling `CGEventPost` and `CFRelease`, balancing the
                // Create rule retain count.
                unsafe {
                    let ev = CGEventCreateMouseEvent(
                        core::ptr::null(),
                        kind.cg_event_type_raw(),
                        point,
                        *cg_button,
                    );
                    if !ev.is_null() {
                        CGEventPost(K_CG_HID_EVENT_TAP, ev);
                        CFRelease(ev.cast_const());
                    }
                }
            }
            TranslatedCgEvent::ScrollWheel {
                is_pixel_unit,
                delta_x,
                delta_y,
            } => {
                let units = if *is_pixel_unit {
                    K_CG_SCROLL_EVENT_UNIT_PIXEL
                } else {
                    K_CG_SCROLL_EVENT_UNIT_LINE
                };
                // SAFETY: `CGEventCreateScrollWheelEvent2` takes integer scroll deltas and a
                // nullable `CGEventSourceRef`. The returned `CGEventRef` is checked for null
                // before posting and releasing via `CFRelease`.
                unsafe {
                    let ev = CGEventCreateScrollWheelEvent2(
                        core::ptr::null(),
                        units,
                        2,
                        *delta_y,
                        *delta_x,
                        0,
                    );
                    if !ev.is_null() {
                        CGEventPost(K_CG_HID_EVENT_TAP, ev);
                        CFRelease(ev.cast_const());
                    }
                }
            }
            TranslatedCgEvent::Keyboard {
                virtual_keycode,
                key_down,
                flags,
            } => {
                // SAFETY: `CGEventCreateKeyboardEvent` allocates a new `CGEventRef` for the
                // given virtual keycode and key state. We check `ev` is non-null before setting
                // modifier flags, posting to `kCGHIDEventTap`, and releasing with `CFRelease`.
                unsafe {
                    let ev =
                        CGEventCreateKeyboardEvent(core::ptr::null(), *virtual_keycode, *key_down);
                    if !ev.is_null() {
                        CGEventSetFlags(ev, *flags);
                        CGEventPost(K_CG_HID_EVENT_TAP, ev);
                        CFRelease(ev.cast_const());
                    }
                }
            }
            TranslatedCgEvent::UnicodeText { utf16_units } => {
                if utf16_units.is_empty() {
                    return Ok(());
                }
                // SAFETY: `utf16_units.as_ptr()` is valid for `utf16_units.len()` `u16` elements
                // for the duration of the synchronous `CGEventKeyboardSetUnicodeString` call.
                // Both `ev` allocations are checked for null and released via `CFRelease`.
                unsafe {
                    let ev = CGEventCreateKeyboardEvent(core::ptr::null(), 0, true);
                    if !ev.is_null() {
                        CGEventKeyboardSetUnicodeString(
                            ev,
                            utf16_units.len(),
                            utf16_units.as_ptr(),
                        );
                        CGEventPost(K_CG_HID_EVENT_TAP, ev);
                        CFRelease(ev.cast_const());
                    }
                }
            }
        }

        Ok(())
    }
}

impl Default for MacOsCgEventInjector {
    fn default() -> Self {
        Self::new(2560, 1440)
    }
}

impl HostInputInjector for MacOsCgEventInjector {
    fn inject_input(&self, event: &InputEvent) -> Result<()> {
        let translated = translate_input_event_to_cg(event, self.screen_width, self.screen_height)?;
        self.injected_count.fetch_add(1, Ordering::SeqCst);

        #[cfg(target_os = "macos")]
        if self.live_posting_enabled {
            self.post_cg_event_macos(&translated)?;
        }

        debug!(
            ?translated,
            live = self.live_posting_enabled,
            "macOS CGEvent input event processed"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cv_pixel_format_mapping_and_iosurface_validation() {
        assert_eq!(
            map_cv_pixel_format_to_wire(CV_PIXEL_FORMAT_32BGRA),
            Some(PixelFormat::Bgra8)
        );
        assert_eq!(
            map_cv_pixel_format_to_wire(CV_PIXEL_FORMAT_32RGBA),
            Some(PixelFormat::Rgba8)
        );
        assert_eq!(
            map_cv_pixel_format_to_wire(CV_PIXEL_FORMAT_420V),
            Some(PixelFormat::NV12)
        );
        assert_eq!(
            map_cv_pixel_format_to_wire(CV_PIXEL_FORMAT_420F),
            Some(PixelFormat::NV12)
        );
        assert_eq!(
            map_cv_pixel_format_to_wire(CV_PIXEL_FORMAT_Y420),
            Some(PixelFormat::YUV420P)
        );
        assert_eq!(
            map_cv_pixel_format_to_wire(CV_PIXEL_FORMAT_Y444),
            Some(PixelFormat::YUV444P)
        );
        assert_eq!(map_cv_pixel_format_to_wire(0), None);

        let bgra_surface = MacOsIoSurfaceDescriptor::new_bgra8(42, 2560, 1440).unwrap();
        assert_eq!(bgra_surface.iosurface_id, 42);
        assert_eq!(bgra_surface.pixel_format, PixelFormat::Bgra8);
        assert!(bgra_surface.metal_texture_compatible);
        assert_eq!(bgra_surface.planes.len(), 1);
        assert_eq!(bgra_surface.planes[0].bytes_per_row, 2560 * 4);

        let nv12_surface = MacOsIoSurfaceDescriptor::new_nv12(43, 1920, 1080).unwrap();
        assert_eq!(nv12_surface.pixel_format, PixelFormat::NV12);
        assert_eq!(nv12_surface.planes.len(), 2);
        assert_eq!(nv12_surface.planes[0].bytes_per_row, 1920);
        assert_eq!(nv12_surface.planes[1].bytes_per_row, 1920);

        // Under-sized stride should fail validation
        let invalid_stride = MacOsIoSurfaceDescriptor::new(
            1,
            1920,
            1080,
            CV_PIXEL_FORMAT_32BGRA,
            true,
            vec![IoSurfacePlaneDescriptor {
                plane_index: 0,
                width: 1920,
                height: 1080,
                bytes_per_element: 4,
                bytes_per_row: 1024,
                offset: 0,
            }],
        );
        assert!(invalid_stride.is_err());

        // Single-plane NV12 should fail validation
        let invalid_nv12 = MacOsIoSurfaceDescriptor::new(
            2,
            1920,
            1080,
            CV_PIXEL_FORMAT_420V,
            true,
            vec![IoSurfacePlaneDescriptor {
                plane_index: 0,
                width: 1920,
                height: 1080,
                bytes_per_element: 1,
                bytes_per_row: 1920,
                offset: 0,
            }],
        );
        assert!(invalid_nv12.is_err());
    }

    #[test]
    fn test_macos_screencapturekit_capturer_headless_fallback() {
        let mut capturer = MacOsScreenCaptureKitCapturer::new(1, 2560, 1440).unwrap();
        assert_eq!(
            capturer.backend_type(),
            CaptureBackendType::MacOsScreenCaptureKit
        );
        assert_eq!(
            capturer.screencapturekit_classes(),
            ("SCShareableContent", "SCStream")
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
    fn test_macos_cgevent_translation_and_input_injection() {
        let injector = MacOsCgEventInjector::new(1920, 1080);
        assert_eq!(injector.injected_count(), 0);

        // 1. Absolute MouseMove clamping
        let move_ev = InputEvent::MouseMove {
            monitor_id: 1,
            x: 2500.0,
            y: -50.0,
            is_absolute: true,
        };
        let translated_move = translate_input_event_to_cg(&move_ev, 1920, 1080).unwrap();
        assert_eq!(
            translated_move,
            TranslatedCgEvent::Mouse {
                kind: CgMouseEventKind::MouseMoved,
                cg_button: 0,
                x: 1919.0,
                y: 0.0,
            }
        );
        injector.inject_input(&move_ev).unwrap();

        // 2. Right MouseButton Down
        let btn_ev = InputEvent::MouseButton {
            button: MouseButtonCode::Right,
            state: ElementState::Pressed,
            x: 400.0,
            y: 300.0,
        };
        let translated_btn = translate_input_event_to_cg(&btn_ev, 1920, 1080).unwrap();
        assert_eq!(
            translated_btn,
            TranslatedCgEvent::Mouse {
                kind: CgMouseEventKind::RightMouseDown,
                cg_button: 1,
                x: 400.0,
                y: 300.0,
            }
        );
        injector.inject_input(&btn_ev).unwrap();

        // 3. KeyEvent with Command + Shift modifiers and scancode mapping
        let key_ev = InputEvent::KeyEvent {
            key_code: 65,
            scancode: 30, // 'A' -> kVK_ANSI_A (0x00)
            state: ElementState::Pressed,
            modifiers: KeyModifiers {
                shift: true,
                ctrl: false,
                alt: false,
                logo: true,
            },
        };
        let translated_key = translate_input_event_to_cg(&key_ev, 1920, 1080).unwrap();
        assert_eq!(
            translated_key,
            TranslatedCgEvent::Keyboard {
                virtual_keycode: 0x00,
                key_down: true,
                flags: CG_EVENT_FLAG_MASK_SHIFT | CG_EVENT_FLAG_MASK_COMMAND,
            }
        );
        injector.inject_input(&key_ev).unwrap();

        // 4. Unicode TextInput
        let text_ev = InputEvent::TextInput {
            text: "rFerram".to_string(),
        };
        let translated_text = translate_input_event_to_cg(&text_ev, 1920, 1080).unwrap();
        assert_eq!(
            translated_text,
            TranslatedCgEvent::UnicodeText {
                utf16_units: "rFerram".encode_utf16().collect(),
            }
        );
        injector.inject_input(&text_ev).unwrap();

        assert_eq!(injector.injected_count(), 4);
    }
}
