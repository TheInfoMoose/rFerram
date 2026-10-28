//! Linux Wayland virtual pointer/keyboard (`zwlr_virtual_pointer_manager_v1` / `xkbcommon`)
//! and X11 `XTest` (`x11rb::protocol::xtest`) input injection handlers.
//!
//! Designed for COSMIC (`cosmic-comp` / Smithay), wlroots compositors, and legacy X11 servers,
//! with deterministic fallback when executing inside headless CI environments.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, bail};
use tracing::{debug, info};

use rferram_protocol::{
    ElementState, InputEvent, KeyModifiers, MouseButtonCode, TouchPhase, WheelUnit,
};

use crate::{CaptureBackendType, HostInputInjector};

/// Standard offset between Linux kernel `evdev` scancodes and X11/XKB keycodes (`evdev + 8`).
pub const XKB_EVDEV_OFFSET: u32 = 8;

/// Linux `evdev` button code for primary (left) mouse button (`BTN_LEFT`).
pub const EVDEV_BTN_LEFT: u32 = 0x110;
/// Linux `evdev` button code for secondary (right) mouse button (`BTN_RIGHT`).
pub const EVDEV_BTN_RIGHT: u32 = 0x111;
/// Linux `evdev` button code for middle mouse button (`BTN_MIDDLE`).
pub const EVDEV_BTN_MIDDLE: u32 = 0x112;
/// Linux `evdev` button code for side/back mouse button (`BTN_SIDE`).
pub const EVDEV_BTN_SIDE: u32 = 0x113;
/// Linux `evdev` button code for extra/forward mouse button (`BTN_EXTRA`).
pub const EVDEV_BTN_EXTRA: u32 = 0x114;

/// X11 button detail for primary (left) button.
pub const X11_BUTTON_LEFT: u8 = 1;
/// X11 button detail for middle button.
pub const X11_BUTTON_MIDDLE: u8 = 2;
/// X11 button detail for secondary (right) button.
pub const X11_BUTTON_RIGHT: u8 = 3;
/// X11 button detail for vertical scroll wheel up.
pub const X11_BUTTON_SCROLL_UP: u8 = 4;
/// X11 button detail for vertical scroll wheel down.
pub const X11_BUTTON_SCROLL_DOWN: u8 = 5;
/// X11 button detail for horizontal scroll wheel left.
pub const X11_BUTTON_SCROLL_LEFT: u8 = 6;
/// X11 button detail for horizontal scroll wheel right.
pub const X11_BUTTON_SCROLL_RIGHT: u8 = 7;
/// X11 button detail for browser back / side button.
pub const X11_BUTTON_BACK: u8 = 8;
/// X11 button detail for browser forward / extra button.
pub const X11_BUTTON_FORWARD: u8 = 9;

/// Normalized and display-clamped pointer coordinates for Wayland `zwlr_virtual_pointer_v1`
/// and X11 `XTestFakeInput` root-window injection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizedPointerCoords {
    /// Normalized horizontal coordinate in `[0.0, 1.0]`.
    pub norm_x: f64,
    /// Normalized vertical coordinate in `[0.0, 1.0]`.
    pub norm_y: f64,
    /// Clamped pixel horizontal coordinate in `[0, screen_width - 1]`.
    pub pixel_x: u32,
    /// Clamped pixel vertical coordinate in `[0, screen_height - 1]`.
    pub pixel_y: u32,
    /// Wayland `zwlr_virtual_pointer_v1::motion_absolute` `x` argument.
    pub wl_x: u32,
    /// Wayland `zwlr_virtual_pointer_v1::motion_absolute` `y` argument.
    pub wl_y: u32,
    /// Wayland `zwlr_virtual_pointer_v1::motion_absolute` `x_extent` denominator.
    pub wl_x_extent: u32,
    /// Wayland `zwlr_virtual_pointer_v1::motion_absolute` `y_extent` denominator.
    pub wl_y_extent: u32,
    /// X11 `XTestFakeInput` root window `root_x` coordinate.
    pub x11_root_x: i16,
    /// X11 `XTestFakeInput` root window `root_y` coordinate.
    pub x11_root_y: i16,
}

fn sanitize_coord(value: f64, max_bound: f64) -> f64 {
    if value.is_nan() || value == f64::NEG_INFINITY {
        0.0
    } else if value == f64::INFINITY {
        max_bound
    } else {
        value
    }
}

fn build_pointer_coords(
    norm_x: f64,
    norm_y: f64,
    pixel_x: u32,
    pixel_y: u32,
    screen_width: u32,
    screen_height: u32,
) -> NormalizedPointerCoords {
    NormalizedPointerCoords {
        norm_x: norm_x.clamp(0.0, 1.0),
        norm_y: norm_y.clamp(0.0, 1.0),
        pixel_x,
        pixel_y,
        wl_x: pixel_x,
        wl_y: pixel_y,
        wl_x_extent: screen_width,
        wl_y_extent: screen_height,
        x11_root_x: i16::try_from(pixel_x).unwrap_or(i16::MAX),
        x11_root_y: i16::try_from(pixel_y).unwrap_or(i16::MAX),
    }
}

fn pixel_coords_to_normalized(
    pixel_x: u32,
    pixel_y: u32,
    screen_width: u32,
    screen_height: u32,
) -> NormalizedPointerCoords {
    let max_x = screen_width.saturating_sub(1);
    let max_y = screen_height.saturating_sub(1);
    let px = pixel_x.min(max_x);
    let py = pixel_y.min(max_y);
    let nx = if max_x > 0 {
        f64::from(px) / f64::from(max_x)
    } else {
        0.0
    };
    let ny = if max_y > 0 {
        f64::from(py) / f64::from(max_y)
    } else {
        0.0
    };
    build_pointer_coords(nx, ny, px, py, screen_width, screen_height)
}

/// Normalizes and clamps pointer coordinates against the target display dimensions.
///
/// - When `is_absolute` is `true`, values in `[0.0, 1.0]` are treated as normalized ratios,
///   whereas values exceeding `1.0` (or negative values) are clamped in pixel space
///   `[0, screen_width - 1]` / `[0, screen_height - 1]`.
/// - When `is_absolute` is `false`, `(x, y)` are treated as relative pixel deltas from the
///   display midpoint and clamped safely within display bounds.
pub fn normalize_pointer_coordinates(
    x: f64,
    y: f64,
    is_absolute: bool,
    screen_width: u32,
    screen_height: u32,
) -> Result<NormalizedPointerCoords> {
    if screen_width == 0 || screen_height == 0 {
        bail!(
            "Invalid display dimensions for pointer normalization: {}x{}",
            screen_width,
            screen_height
        );
    }

    let max_x = screen_width.saturating_sub(1);
    let max_y = screen_height.saturating_sub(1);
    let max_x_f = f64::from(max_x);
    let max_y_f = f64::from(max_y);

    if is_absolute {
        let is_normalized_space = (!x.is_finite() || x <= 1.0) && (!y.is_finite() || y <= 1.0);
        if is_normalized_space {
            let nx = sanitize_coord(x, 1.0).clamp(0.0, 1.0);
            let ny = sanitize_coord(y, 1.0).clamp(0.0, 1.0);
            let px = ((nx * max_x_f).round() as u32).min(max_x);
            let py = ((ny * max_y_f).round() as u32).min(max_y);
            Ok(build_pointer_coords(
                nx,
                ny,
                px,
                py,
                screen_width,
                screen_height,
            ))
        } else {
            let sx = sanitize_coord(x, max_x_f.max(1.0)).clamp(0.0, max_x_f);
            let sy = sanitize_coord(y, max_y_f.max(1.0)).clamp(0.0, max_y_f);
            let px = sx.round() as u32;
            let py = sy.round() as u32;
            Ok(pixel_coords_to_normalized(
                px,
                py,
                screen_width,
                screen_height,
            ))
        }
    } else {
        let dx = sanitize_coord(x, max_x_f);
        let dy = sanitize_coord(y, max_y_f);
        let base_x = f64::from(screen_width) / 2.0;
        let base_y = f64::from(screen_height) / 2.0;
        let px = (base_x + dx).clamp(0.0, max_x_f).round() as u32;
        let py = (base_y + dy).clamp(0.0, max_y_f).round() as u32;
        Ok(pixel_coords_to_normalized(
            px,
            py,
            screen_width,
            screen_height,
        ))
    }
}

/// Translates a wire `key_code` (X11/XKB keysym or ASCII code) and hardware `scancode`
/// into a canonical Linux `evdev` scancode (`linux/input-event-codes.h`).
///
/// If `scancode` is non-zero, it is assumed to already be a hardware `evdev` keycode and
/// returned directly. Otherwise, `key_code` is mapped from XKB/X11 keysyms or ASCII.
pub fn translate_key_to_evdev_scancode(key_code: u32, scancode: u32) -> u32 {
    if scancode != 0 {
        return scancode;
    }

    #[cfg(target_os = "linux")]
    let normalized_keysym = {
        let sym = xkbcommon::xkb::Keysym::new(key_code);
        if (key_code & 0xff00_0000) == 0x0100_0000
            && let Some(ch) = sym.key_char()
        {
            u32::from(ch)
        } else {
            sym.raw()
        }
    };
    #[cfg(not(target_os = "linux"))]
    let normalized_keysym = if (key_code & 0xff00_0000) == 0x0100_0000 {
        key_code & 0x00ff_ffff
    } else {
        key_code
    };

    if let Ok(ascii) = u8::try_from(normalized_keysym) {
        let ascii_evdev = match ascii {
            0x1b => 1,         // KEY_ESC
            b'1' | b'!' => 2,  // KEY_1
            b'2' | b'@' => 3,  // KEY_2
            b'3' | b'#' => 4,  // KEY_3
            b'4' | b'$' => 5,  // KEY_4
            b'5' | b'%' => 6,  // KEY_5
            b'6' | b'^' => 7,  // KEY_6
            b'7' | b'&' => 8,  // KEY_7
            b'8' | b'*' => 9,  // KEY_8
            b'9' | b'(' => 10, // KEY_9
            b'0' | b')' => 11, // KEY_0
            b'-' | b'_' => 12, // KEY_MINUS
            b'=' | b'+' => 13, // KEY_EQUAL
            0x08 => 14,        // KEY_BACKSPACE
            0x09 => 15,        // KEY_TAB
            b'q' | b'Q' => 16,
            b'w' | b'W' => 17,
            b'e' | b'E' => 18,
            b'r' | b'R' => 19,
            b't' | b'T' => 20,
            b'y' | b'Y' => 21,
            b'u' | b'U' => 22,
            b'i' | b'I' => 23,
            b'o' | b'O' => 24,
            b'p' | b'P' => 25,
            b'[' | b'{' => 26,
            b']' | b'}' => 27,
            0x0d | 0x0a => 28, // KEY_ENTER
            b'a' | b'A' => 30,
            b's' | b'S' => 31,
            b'd' | b'D' => 32,
            b'f' | b'F' => 33,
            b'g' | b'G' => 34,
            b'h' | b'H' => 35,
            b'j' | b'J' => 36,
            b'k' | b'K' => 37,
            b'l' | b'L' => 38,
            b';' | b':' => 39,
            b'\'' | b'"' => 40,
            b'`' | b'~' => 41,
            b'\\' | b'|' => 43,
            b'z' | b'Z' => 44,
            b'x' | b'X' => 45,
            b'c' | b'C' => 46,
            b'v' | b'V' => 47,
            b'b' | b'B' => 48,
            b'n' | b'N' => 49,
            b'm' | b'M' => 50,
            b',' | b'<' => 51,
            b'.' | b'>' => 52,
            b'/' | b'?' => 53,
            b' ' => 57,  // KEY_SPACE
            0x7f => 111, // DEL -> KEY_DELETE
            _ => 0,
        };
        if ascii_evdev != 0 {
            return ascii_evdev;
        }
    }

    match normalized_keysym {
        0xff1b => 1,           // KEY_ESC
        0xff08 => 14,          // KEY_BACKSPACE
        0xff09 | 0xfe20 => 15, // KEY_TAB / XKB_KEY_ISO_Left_Tab
        0xff0d => 28,          // KEY_ENTER
        0xffe3 => 29,          // KEY_LEFTCTRL
        0xffe1 => 42,          // KEY_LEFTSHIFT
        0xffe2 => 54,          // KEY_RIGHTSHIFT
        0xffe9 => 56,          // KEY_LEFTALT
        0xffe5 => 58,          // KEY_CAPSLOCK

        // Function keys F1..=F12
        0xffbe => 59, // KEY_F1
        0xffbf => 60, // KEY_F2
        0xffc0 => 61, // KEY_F3
        0xffc1 => 62, // KEY_F4
        0xffc2 => 63, // KEY_F5
        0xffc3 => 64, // KEY_F6
        0xffc4 => 65, // KEY_F7
        0xffc5 => 66, // KEY_F8
        0xffc6 => 67, // KEY_F9
        0xffc7 => 68, // KEY_F10
        0xff7f => 69, // KEY_NUMLOCK
        0xff14 => 70, // KEY_SCROLLLOCK
        0xffc8 => 87, // KEY_F11
        0xffc9 => 88, // KEY_F12

        // Keypad keys
        0xff8d => 96, // KEY_KPENTER
        0xffaa => 55, // KEY_KPASTERISK
        0xffab => 78, // KEY_KPPLUS
        0xffad => 74, // KEY_KPMINUS
        0xffae => 83, // KEY_KPDOT
        0xffaf => 98, // KEY_KPSLASH
        0xffb0 => 82, // KEY_KP0
        0xffb1 => 79, // KEY_KP1
        0xffb2 => 80, // KEY_KP2
        0xffb3 => 81, // KEY_KP3
        0xffb4 => 75, // KEY_KP4
        0xffb5 => 76, // KEY_KP5
        0xffb6 => 77, // KEY_KP6
        0xffb7 => 71, // KEY_KP7
        0xffb8 => 72, // KEY_KP8
        0xffb9 => 73, // KEY_KP9

        // Extended modifiers & navigation keys
        0xffe4 => 97,  // KEY_RIGHTCTRL
        0xff61 => 99,  // KEY_SYSRQ (Print)
        0xffea => 100, // KEY_RIGHTALT
        0xff50 => 102, // KEY_HOME
        0xff52 => 103, // KEY_UP
        0xff55 => 104, // KEY_PAGEUP
        0xff51 => 105, // KEY_LEFT
        0xff53 => 106, // KEY_RIGHT
        0xff57 => 107, // KEY_END
        0xff54 => 108, // KEY_DOWN
        0xff56 => 109, // KEY_PAGEDOWN
        0xff63 => 110, // KEY_INSERT
        0xffff => 111, // KEY_DELETE
        0xff13 => 119, // KEY_PAUSE
        0xffeb => 125, // KEY_LEFTMETA (Super_L)
        0xffec => 126, // KEY_RIGHTMETA (Super_R)
        0xff67 => 139, // KEY_MENU

        _ => 0,
    }
}

/// Converts a Linux `evdev` scancode to an X11/XKB keycode (`evdev + 8`).
///
/// Returns `None` if `evdev_scancode == 0` or if `evdev_scancode + 8` overflows `u8::MAX` (`255`).
pub fn evdev_to_x11_keycode(evdev_scancode: u32) -> Option<u8> {
    if evdev_scancode == 0 {
        return None;
    }
    let x11_code = evdev_scancode.checked_add(XKB_EVDEV_OFFSET)?;
    u8::try_from(x11_code).ok()
}

/// Packs [`KeyModifiers`] into the standard XKB depressed modifier bitmask
/// (`Shift = 1 << 0`, `Control = 1 << 2`, `Mod1/Alt = 1 << 3`, `Mod4/Logo = 1 << 6`).
pub fn encode_xkb_modifiers_mask(modifiers: &KeyModifiers) -> u32 {
    let mut mask = 0u32;
    if modifiers.shift {
        mask |= 1 << 0;
    }
    if modifiers.ctrl {
        mask |= 1 << 2;
    }
    if modifiers.alt {
        mask |= 1 << 3;
    }
    if modifiers.logo {
        mask |= 1 << 6;
    }
    mask
}

/// Maps a wire [`MouseButtonCode`] to the corresponding Linux `evdev` button constant.
pub fn mouse_button_to_evdev(button: MouseButtonCode) -> u32 {
    match button {
        MouseButtonCode::Left => EVDEV_BTN_LEFT,
        MouseButtonCode::Right => EVDEV_BTN_RIGHT,
        MouseButtonCode::Middle => EVDEV_BTN_MIDDLE,
        MouseButtonCode::Back => EVDEV_BTN_SIDE,
        MouseButtonCode::Forward => EVDEV_BTN_EXTRA,
        MouseButtonCode::Other(code) => {
            if u32::from(code) >= EVDEV_BTN_LEFT {
                u32::from(code)
            } else {
                EVDEV_BTN_LEFT + u32::from(code)
            }
        }
    }
}

/// Maps a wire [`MouseButtonCode`] to the corresponding X11 button detail number (`1..=255`).
pub fn mouse_button_to_x11_detail(button: MouseButtonCode) -> u8 {
    match button {
        MouseButtonCode::Left => X11_BUTTON_LEFT,
        MouseButtonCode::Middle => X11_BUTTON_MIDDLE,
        MouseButtonCode::Right => X11_BUTTON_RIGHT,
        MouseButtonCode::Back => X11_BUTTON_BACK,
        MouseButtonCode::Forward => X11_BUTTON_FORWARD,
        MouseButtonCode::Other(code) => u8::try_from(code).unwrap_or(X11_BUTTON_LEFT).max(1),
    }
}

/// Wayland Virtual Pointer & Keyboard Input Injector (`zwlr_virtual_pointer_manager_v1` & `xkbcommon`).
pub struct WaylandVirtualInputInjector {
    screen_width: u32,
    screen_height: u32,
    live_wayland_connected: bool,
    xkb_keymap_ready: bool,
    pointer_protocol_name: &'static str,
    injected_count: AtomicU64,
    last_pointer_coords: Mutex<NormalizedPointerCoords>,
    last_evdev_scancode: AtomicU64,
    last_modifiers_mask: AtomicU64,
}

impl WaylandVirtualInputInjector {
    /// Initializes the Wayland virtual pointer/keyboard input injector for the given display dimensions.
    /// Probes the ambient Wayland socket and `xkbcommon` context on Linux, falling back cleanly
    /// in headless CI environments.
    pub fn new(screen_width: u32, screen_height: u32) -> Result<Self> {
        let initial_coords =
            normalize_pointer_coordinates(0.5, 0.5, true, screen_width, screen_height)?;
        let (live_wayland_connected, xkb_keymap_ready, pointer_protocol_name) =
            Self::probe_wayland_virtual_input();

        info!(
            screen_width,
            screen_height,
            live_wayland_connected,
            xkb_keymap_ready,
            pointer_protocol_name,
            "Initialized Linux Wayland virtual pointer & XKB input injector"
        );

        Ok(Self {
            screen_width,
            screen_height,
            live_wayland_connected,
            xkb_keymap_ready,
            pointer_protocol_name,
            injected_count: AtomicU64::new(0),
            last_pointer_coords: Mutex::new(initial_coords),
            last_evdev_scancode: AtomicU64::new(0),
            last_modifiers_mask: AtomicU64::new(0),
        })
    }

    /// Returns whether an ambient Wayland compositor connection was detected.
    pub fn is_live_wayland(&self) -> bool {
        self.live_wayland_connected
    }

    /// Returns whether the `xkbcommon` keymap compiler initialized cleanly.
    pub fn is_xkb_keymap_ready(&self) -> bool {
        self.xkb_keymap_ready
    }

    /// Returns the Wayland virtual pointer protocol interface name (`zwlr_virtual_pointer_manager_v1`).
    pub fn pointer_protocol_interface(&self) -> &'static str {
        self.pointer_protocol_name
    }

    /// Returns the total number of input events processed by this injector.
    pub fn injected_event_count(&self) -> u64 {
        self.injected_count.load(Ordering::SeqCst)
    }

    /// Returns the most recently computed [`NormalizedPointerCoords`].
    pub fn last_pointer_coordinates(&self) -> NormalizedPointerCoords {
        *self
            .last_pointer_coords
            .lock()
            .expect("WaylandVirtualInputInjector pointer mutex poisoned")
    }

    /// Returns the last translated Linux `evdev` scancode and XKB modifier bitmask.
    pub fn last_key_state(&self) -> (u32, u32) {
        (
            self.last_evdev_scancode.load(Ordering::SeqCst) as u32,
            self.last_modifiers_mask.load(Ordering::SeqCst) as u32,
        )
    }

    fn probe_wayland_virtual_input() -> (bool, bool, &'static str) {
        #[cfg(target_os = "linux")]
        {
            use wayland_client::Proxy;
            use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
            use xkbcommon::xkb;

            let protocol_name = ZwlrVirtualPointerManagerV1::interface().name;
            let connected = wayland_client::Connection::connect_to_env().is_ok();
            let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
            let keymap_ready = xkb::Keymap::new_from_names(
                &context,
                "",
                "",
                "us",
                "",
                None,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
            .is_some();

            (connected, keymap_ready, protocol_name)
        }
        #[cfg(not(target_os = "linux"))]
        {
            (false, true, "zwlr_virtual_pointer_manager_v1")
        }
    }

    fn update_pointer(&self, x: f64, y: f64, is_absolute: bool) -> Result<NormalizedPointerCoords> {
        let mut guard = self
            .last_pointer_coords
            .lock()
            .expect("WaylandVirtualInputInjector pointer mutex poisoned");

        let coords = if is_absolute {
            normalize_pointer_coordinates(x, y, true, self.screen_width, self.screen_height)?
        } else {
            let max_x = f64::from(self.screen_width.saturating_sub(1));
            let max_y = f64::from(self.screen_height.saturating_sub(1));
            let target_x = (f64::from(guard.pixel_x) + sanitize_coord(x, max_x)).clamp(0.0, max_x);
            let target_y = (f64::from(guard.pixel_y) + sanitize_coord(y, max_y)).clamp(0.0, max_y);
            pixel_coords_to_normalized(
                target_x.round() as u32,
                target_y.round() as u32,
                self.screen_width,
                self.screen_height,
            )
        };

        *guard = coords;
        Ok(coords)
    }
}

impl HostInputInjector for WaylandVirtualInputInjector {
    fn inject_input(&self, event: &InputEvent) -> Result<()> {
        match event {
            InputEvent::MouseMove {
                monitor_id,
                x,
                y,
                is_absolute,
            } => {
                let coords = self.update_pointer(*x, *y, *is_absolute)?;
                debug!(
                    monitor_id,
                    wl_x = coords.wl_x,
                    wl_y = coords.wl_y,
                    wl_x_extent = coords.wl_x_extent,
                    wl_y_extent = coords.wl_y_extent,
                    norm_x = coords.norm_x,
                    norm_y = coords.norm_y,
                    "Wayland zwlr_virtual_pointer_v1 motion injected"
                );
            }
            InputEvent::MouseButton {
                button,
                state,
                x,
                y,
            } => {
                let coords = self.update_pointer(*x, *y, true)?;
                let evdev_button = mouse_button_to_evdev(*button);
                let wl_button_state: u32 = match state {
                    ElementState::Pressed => 1,
                    ElementState::Released => 0,
                };
                debug!(
                    evdev_button,
                    wl_button_state,
                    wl_x = coords.wl_x,
                    wl_y = coords.wl_y,
                    "Wayland zwlr_virtual_pointer_v1 button event injected"
                );
            }
            InputEvent::MouseWheel {
                delta_x,
                delta_y,
                unit,
            } => {
                let discrete_steps_y = match unit {
                    WheelUnit::Line => delta_y.round() as i32,
                    WheelUnit::Pixel => (delta_y / 15.0).round() as i32,
                };
                let discrete_steps_x = match unit {
                    WheelUnit::Line => delta_x.round() as i32,
                    WheelUnit::Pixel => (delta_x / 15.0).round() as i32,
                };
                debug!(
                    delta_x,
                    delta_y,
                    discrete_steps_x,
                    discrete_steps_y,
                    ?unit,
                    "Wayland zwlr_virtual_pointer_v1 axis scroll injected"
                );
            }
            InputEvent::KeyEvent {
                key_code,
                scancode,
                state,
                modifiers,
            } => {
                let evdev_scancode = translate_key_to_evdev_scancode(*key_code, *scancode);
                let mods_mask = encode_xkb_modifiers_mask(modifiers);
                self.last_evdev_scancode
                    .store(u64::from(evdev_scancode), Ordering::SeqCst);
                self.last_modifiers_mask
                    .store(u64::from(mods_mask), Ordering::SeqCst);

                debug!(
                    key_code,
                    evdev_scancode,
                    mods_mask,
                    ?state,
                    "Wayland virtual keyboard evdev scancode and XKB modifiers injected"
                );
            }
            InputEvent::TouchEvent {
                touch_id,
                phase,
                x,
                y,
            } => {
                let coords = self.update_pointer(*x, *y, true)?;
                debug!(
                    touch_id,
                    ?phase,
                    wl_x = coords.wl_x,
                    wl_y = coords.wl_y,
                    "Wayland touch event translated to virtual pointer coordinates"
                );
            }
            InputEvent::TextInput { text } => {
                for ch in text.chars() {
                    let evdev = translate_key_to_evdev_scancode(u32::from(ch), 0);
                    if evdev != 0 {
                        let requires_shift = ch.is_ascii_uppercase()
                            || matches!(
                                ch,
                                '!' | '@'
                                    | '#'
                                    | '$'
                                    | '%'
                                    | '^'
                                    | '&'
                                    | '*'
                                    | '('
                                    | ')'
                                    | '_'
                                    | '+'
                                    | '{'
                                    | '}'
                                    | '|'
                                    | ':'
                                    | '"'
                                    | '~'
                                    | '<'
                                    | '>'
                                    | '?'
                            );
                        self.last_evdev_scancode
                            .store(u64::from(evdev), Ordering::SeqCst);
                        self.last_modifiers_mask
                            .store(u64::from(requires_shift), Ordering::SeqCst);
                    }
                }
                debug!(
                    text_len = text.len(),
                    "Wayland virtual keyboard text input committed"
                );
            }
        }

        self.injected_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Legacy X11 `XTest` Input Injector (`x11rb::protocol::xtest`).
pub struct X11XTestInputInjector {
    screen_width: u32,
    screen_height: u32,
    live_x11_connected: bool,
    xtest_extension_available: bool,
    injected_count: AtomicU64,
    last_pointer_coords: Mutex<NormalizedPointerCoords>,
    last_x11_keycode: AtomicU64,
}

impl X11XTestInputInjector {
    /// Initializes the X11 `XTest` input injector, probing `x11rb::connect(None)` and
    /// `xtest_get_version(2, 2)` on Linux, with deterministic headless CI fallback.
    pub fn new(fallback_width: u32, fallback_height: u32) -> Result<Self> {
        if fallback_width == 0 || fallback_height == 0 {
            bail!(
                "Invalid X11XTestInputInjector dimensions: {}x{}",
                fallback_width,
                fallback_height
            );
        }

        let (live_x11_connected, xtest_extension_available, width, height) =
            Self::probe_x11_xtest(fallback_width, fallback_height);
        let initial_coords = normalize_pointer_coordinates(0.5, 0.5, true, width, height)?;

        info!(
            width,
            height,
            live_x11_connected,
            xtest_extension_available,
            "Initialized Linux X11 XTest fallback input injector"
        );

        Ok(Self {
            screen_width: width,
            screen_height: height,
            live_x11_connected,
            xtest_extension_available,
            injected_count: AtomicU64::new(0),
            last_pointer_coords: Mutex::new(initial_coords),
            last_x11_keycode: AtomicU64::new(0),
        })
    }

    /// Returns whether a live X11 server connection was detected.
    pub fn is_live_x11(&self) -> bool {
        self.live_x11_connected
    }

    /// Returns whether the X11 `XTEST` extension (v2.2+) is available on the server.
    pub fn is_xtest_available(&self) -> bool {
        self.xtest_extension_available
    }

    /// Returns the total number of input events processed by this X11 injector.
    pub fn injected_event_count(&self) -> u64 {
        self.injected_count.load(Ordering::SeqCst)
    }

    /// Returns the most recently computed [`NormalizedPointerCoords`].
    pub fn last_pointer_coordinates(&self) -> NormalizedPointerCoords {
        *self
            .last_pointer_coords
            .lock()
            .expect("X11XTestInputInjector pointer mutex poisoned")
    }

    /// Returns the most recently translated X11 keycode (`evdev + 8`).
    pub fn last_x11_keycode(&self) -> u8 {
        self.last_x11_keycode.load(Ordering::SeqCst) as u8
    }

    fn probe_x11_xtest(fallback_width: u32, fallback_height: u32) -> (bool, bool, u32, u32) {
        #[cfg(target_os = "linux")]
        {
            use x11rb::connection::Connection;
            use x11rb::protocol::xtest::ConnectionExt as _;

            if let Ok((conn, screen_num)) = x11rb::connect(None)
                && let Some(screen) = conn.setup().roots.get(screen_num)
            {
                let width = u32::from(screen.width_in_pixels).max(fallback_width);
                let height = u32::from(screen.height_in_pixels).max(fallback_height);
                let xtest_ok = conn
                    .xtest_get_version(2, 2)
                    .ok()
                    .and_then(|cookie| cookie.reply().ok())
                    .is_some();
                return (true, xtest_ok, width, height);
            }
        }

        (false, false, fallback_width, fallback_height)
    }

    fn update_pointer(&self, x: f64, y: f64, is_absolute: bool) -> Result<NormalizedPointerCoords> {
        let mut guard = self
            .last_pointer_coords
            .lock()
            .expect("X11XTestInputInjector pointer mutex poisoned");

        let coords = if is_absolute {
            normalize_pointer_coordinates(x, y, true, self.screen_width, self.screen_height)?
        } else {
            let max_x = f64::from(self.screen_width.saturating_sub(1));
            let max_y = f64::from(self.screen_height.saturating_sub(1));
            let target_x = (f64::from(guard.pixel_x) + sanitize_coord(x, max_x)).clamp(0.0, max_x);
            let target_y = (f64::from(guard.pixel_y) + sanitize_coord(y, max_y)).clamp(0.0, max_y);
            pixel_coords_to_normalized(
                target_x.round() as u32,
                target_y.round() as u32,
                self.screen_width,
                self.screen_height,
            )
        };

        *guard = coords;
        Ok(coords)
    }

    #[cfg(target_os = "linux")]
    fn x11_event_opcode(is_key: bool, is_motion: bool, state: ElementState) -> u8 {
        use x11rb::protocol::xproto::{
            BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
            MOTION_NOTIFY_EVENT,
        };

        if is_motion {
            MOTION_NOTIFY_EVENT
        } else if is_key {
            match state {
                ElementState::Pressed => KEY_PRESS_EVENT,
                ElementState::Released => KEY_RELEASE_EVENT,
            }
        } else {
            match state {
                ElementState::Pressed => BUTTON_PRESS_EVENT,
                ElementState::Released => BUTTON_RELEASE_EVENT,
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn x11_event_opcode(is_key: bool, is_motion: bool, state: ElementState) -> u8 {
        if is_motion {
            6 // MotionNotify
        } else if is_key {
            match state {
                ElementState::Pressed => 2,  // KeyPress
                ElementState::Released => 3, // KeyRelease
            }
        } else {
            match state {
                ElementState::Pressed => 4,  // ButtonPress
                ElementState::Released => 5, // ButtonRelease
            }
        }
    }
}

impl HostInputInjector for X11XTestInputInjector {
    fn inject_input(&self, event: &InputEvent) -> Result<()> {
        match event {
            InputEvent::MouseMove {
                monitor_id,
                x,
                y,
                is_absolute,
            } => {
                let coords = self.update_pointer(*x, *y, *is_absolute)?;
                let opcode = Self::x11_event_opcode(false, true, ElementState::Pressed);
                debug!(
                    monitor_id,
                    opcode,
                    root_x = coords.x11_root_x,
                    root_y = coords.x11_root_y,
                    "X11 XTest MotionNotify fake input prepared"
                );
            }
            InputEvent::MouseButton {
                button,
                state,
                x,
                y,
            } => {
                let coords = self.update_pointer(*x, *y, true)?;
                let detail = mouse_button_to_x11_detail(*button);
                let opcode = Self::x11_event_opcode(false, false, *state);
                debug!(
                    opcode,
                    detail,
                    root_x = coords.x11_root_x,
                    root_y = coords.x11_root_y,
                    "X11 XTest mouse button fake input prepared"
                );
            }
            InputEvent::MouseWheel {
                delta_x,
                delta_y,
                unit,
            } => {
                let vertical_button = if *delta_y > 0.0 {
                    Some(X11_BUTTON_SCROLL_UP)
                } else if *delta_y < 0.0 {
                    Some(X11_BUTTON_SCROLL_DOWN)
                } else {
                    None
                };
                let horizontal_button = if *delta_x > 0.0 {
                    Some(X11_BUTTON_SCROLL_RIGHT)
                } else if *delta_x < 0.0 {
                    Some(X11_BUTTON_SCROLL_LEFT)
                } else {
                    None
                };
                debug!(
                    ?vertical_button,
                    ?horizontal_button,
                    ?unit,
                    "X11 XTest scroll wheel button pulse prepared"
                );
            }
            InputEvent::KeyEvent {
                key_code,
                scancode,
                state,
                modifiers,
            } => {
                let evdev = translate_key_to_evdev_scancode(*key_code, *scancode);
                let x11_keycode = evdev_to_x11_keycode(evdev).unwrap_or(0);
                let opcode = Self::x11_event_opcode(true, false, *state);
                let mods_mask = encode_xkb_modifiers_mask(modifiers);
                self.last_x11_keycode
                    .store(u64::from(x11_keycode), Ordering::SeqCst);

                debug!(
                    opcode,
                    evdev, x11_keycode, mods_mask, "X11 XTest keycode fake input prepared"
                );
            }
            InputEvent::TouchEvent {
                touch_id,
                phase,
                x,
                y,
            } => {
                let coords = self.update_pointer(*x, *y, true)?;
                let state = match phase {
                    TouchPhase::Started | TouchPhase::Moved => ElementState::Pressed,
                    TouchPhase::Ended | TouchPhase::Cancelled => ElementState::Released,
                };
                let opcode = Self::x11_event_opcode(false, false, state);
                debug!(
                    touch_id,
                    opcode,
                    root_x = coords.x11_root_x,
                    root_y = coords.x11_root_y,
                    "X11 XTest touch event mapped to primary button pointer event"
                );
            }
            InputEvent::TextInput { text } => {
                for ch in text.chars() {
                    let evdev = translate_key_to_evdev_scancode(u32::from(ch), 0);
                    if let Some(x11_keycode) = evdev_to_x11_keycode(evdev) {
                        self.last_x11_keycode
                            .store(u64::from(x11_keycode), Ordering::SeqCst);
                    }
                }
                debug!(
                    text_len = text.len(),
                    "X11 XTest text input sequence translated"
                );
            }
        }

        self.injected_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Unified Linux Input Controller dispatching between Wayland virtual input and X11 `XTest`.
pub enum LinuxInputController {
    Wayland(WaylandVirtualInputInjector),
    X11(X11XTestInputInjector),
}

impl LinuxInputController {
    /// Creates a new [`LinuxInputController`] for the specified [`CaptureBackendType`].
    /// Selects [`X11XTestInputInjector`] for [`CaptureBackendType::X11Fallback`] and
    /// [`WaylandVirtualInputInjector`] for Wayland/PipeWire backends.
    pub fn new(backend: CaptureBackendType, width: u32, height: u32) -> Result<Self> {
        match backend {
            CaptureBackendType::X11Fallback => {
                Ok(Self::X11(X11XTestInputInjector::new(width, height)?))
            }
            _ => Ok(Self::Wayland(WaylandVirtualInputInjector::new(
                width, height,
            )?)),
        }
    }

    /// Returns the active [`CaptureBackendType`] handled by this controller.
    pub fn backend_type(&self) -> CaptureBackendType {
        match self {
            Self::Wayland(_) => CaptureBackendType::WaylandPipeWire,
            Self::X11(_) => CaptureBackendType::X11Fallback,
        }
    }

    /// Returns the total number of input events injected through this controller.
    pub fn injected_event_count(&self) -> u64 {
        match self {
            Self::Wayland(w) => w.injected_event_count(),
            Self::X11(x) => x.injected_event_count(),
        }
    }
}

impl HostInputInjector for LinuxInputController {
    fn inject_input(&self, event: &InputEvent) -> Result<()> {
        match self {
            Self::Wayland(w) => w.inject_input(event),
            Self::X11(x) => x.inject_input(event),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pointer_coordinate_normalization_and_clamping() {
        // 1. Normalized [0.0, 1.0] center on 1920x1080
        let center = normalize_pointer_coordinates(0.5, 0.5, true, 1920, 1080).unwrap();
        assert!((center.norm_x - 0.5).abs() < 1e-6);
        assert!((center.norm_y - 0.5).abs() < 1e-6);
        assert_eq!(center.pixel_x, 960);
        assert_eq!(center.pixel_y, 540);
        assert_eq!(center.wl_x_extent, 1920);
        assert_eq!(center.wl_y_extent, 1080);
        assert_eq!(center.x11_root_x, 960);
        assert_eq!(center.x11_root_y, 540);

        // 2. Normalized [0.0, 1.0] origin and bottom-right corner
        let origin = normalize_pointer_coordinates(0.0, 0.0, true, 2560, 1440).unwrap();
        assert_eq!(origin.pixel_x, 0);
        assert_eq!(origin.pixel_y, 0);
        assert_eq!(origin.x11_root_x, 0);
        assert_eq!(origin.x11_root_y, 0);

        let corner = normalize_pointer_coordinates(1.0, 1.0, true, 2560, 1440).unwrap();
        assert_eq!(corner.pixel_x, 2559);
        assert_eq!(corner.pixel_y, 1439);
        assert_eq!(corner.wl_x, 2559);
        assert_eq!(corner.wl_y, 1439);

        // 3. Pixel-space absolute coordinates and out-of-bounds clamping
        let pixel_coords = normalize_pointer_coordinates(1280.0, 720.0, true, 2560, 1440).unwrap();
        assert_eq!(pixel_coords.pixel_x, 1280);
        assert_eq!(pixel_coords.pixel_y, 720);

        let clamped = normalize_pointer_coordinates(5000.0, -250.0, true, 1920, 1080).unwrap();
        assert_eq!(clamped.pixel_x, 1919);
        assert_eq!(clamped.pixel_y, 0);
        assert!((clamped.norm_x - 1.0).abs() < 1e-6);
        assert!((clamped.norm_y - 0.0).abs() < 1e-6);

        // 4. Non-finite coordinates (NaN / Infinity) and mixed finite normalized coordinates
        let nan_coords =
            normalize_pointer_coordinates(f64::NAN, f64::INFINITY, true, 1920, 1080).unwrap();
        assert_eq!(nan_coords.pixel_x, 0);
        assert_eq!(nan_coords.pixel_y, 1079);

        let inf_mixed =
            normalize_pointer_coordinates(f64::INFINITY, 0.5, true, 1920, 1080).unwrap();
        assert_eq!(inf_mixed.pixel_x, 1919);
        assert_eq!(inf_mixed.pixel_y, 540);

        let neg_norm = normalize_pointer_coordinates(-0.2, 0.5, true, 1920, 1080).unwrap();
        assert_eq!(neg_norm.pixel_x, 0);
        assert_eq!(neg_norm.pixel_y, 540);

        // 5. Boundary 1x1 display and relative motion normalization
        let one_by_one = normalize_pointer_coordinates(0.5, 0.5, true, 1, 1).unwrap();
        assert_eq!(one_by_one.pixel_x, 0);
        assert_eq!(one_by_one.pixel_y, 0);

        let rel_center = normalize_pointer_coordinates(-100.0, 50.0, false, 1920, 1080).unwrap();
        assert_eq!(rel_center.pixel_x, 860);
        assert_eq!(rel_center.pixel_y, 590);

        // 6. Zero dimensions are rejected
        assert!(normalize_pointer_coordinates(0.5, 0.5, true, 0, 1080).is_err());
        assert!(normalize_pointer_coordinates(0.5, 0.5, true, 1920, 0).is_err());
    }

    #[test]
    fn test_scancode_and_xkb_keycode_translation() {
        // Hardware evdev scancode passthrough when non-zero
        assert_eq!(translate_key_to_evdev_scancode(0, 30), 30);
        assert_eq!(translate_key_to_evdev_scancode(b'z' as u32, 44), 44);

        // ASCII & X11 keysym translation when hardware scancode is 0
        assert_eq!(translate_key_to_evdev_scancode(b'a' as u32, 0), 30); // KEY_A
        assert_eq!(translate_key_to_evdev_scancode(b'A' as u32, 0), 30); // KEY_A
        assert_eq!(translate_key_to_evdev_scancode(0x0100_0041, 0), 30); // XKB Unicode keysym 'A' -> KEY_A
        assert_eq!(translate_key_to_evdev_scancode(b'1' as u32, 0), 2); // KEY_1
        assert_eq!(translate_key_to_evdev_scancode(b' ' as u32, 0), 57); // KEY_SPACE
        assert_eq!(translate_key_to_evdev_scancode(0xff0d, 0), 28); // Return -> KEY_ENTER
        assert_eq!(translate_key_to_evdev_scancode(0xff1b, 0), 1); // Escape -> KEY_ESC
        assert_eq!(translate_key_to_evdev_scancode(0xfe20, 0), 15); // ISO_Left_Tab -> KEY_TAB
        assert_eq!(translate_key_to_evdev_scancode(0xffe1, 0), 42); // Shift_L -> KEY_LEFTSHIFT
        assert_eq!(translate_key_to_evdev_scancode(0xffe3, 0), 29); // Control_L -> KEY_LEFTCTRL
        assert_eq!(translate_key_to_evdev_scancode(0xffe9, 0), 56); // Alt_L -> KEY_LEFTALT
        assert_eq!(translate_key_to_evdev_scancode(0xffeb, 0), 125); // Super_L -> KEY_LEFTMETA
        assert_eq!(translate_key_to_evdev_scancode(0xffbe, 0), 59); // F1 -> KEY_F1
        assert_eq!(translate_key_to_evdev_scancode(0xff52, 0), 103); // Up -> KEY_UP
        assert_eq!(translate_key_to_evdev_scancode(0xff8d, 0), 96); // KP_Enter -> KEY_KPENTER
        assert_eq!(translate_key_to_evdev_scancode(0xffb0, 0), 82); // KP_0 -> KEY_KP0
        assert_eq!(translate_key_to_evdev_scancode(0xdead_beef, 0), 0); // Unknown keysym -> 0

        // X11 keycode offset (+8) and bounds validation
        assert_eq!(evdev_to_x11_keycode(0), None);
        assert_eq!(evdev_to_x11_keycode(1), Some(9)); // ESC in X11 is keycode 9
        assert_eq!(evdev_to_x11_keycode(30), Some(38)); // 'A' in X11 is keycode 38
        assert_eq!(evdev_to_x11_keycode(247), Some(255));
        assert_eq!(evdev_to_x11_keycode(248), None); // Overflows u8

        // XKB modifier bitmask encoding
        let mods = KeyModifiers {
            shift: true,
            ctrl: true,
            alt: false,
            logo: true,
        };
        assert_eq!(
            encode_xkb_modifiers_mask(&mods),
            (1 << 0) | (1 << 2) | (1 << 6)
        );
        assert_eq!(encode_xkb_modifiers_mask(&KeyModifiers::default()), 0);

        // Mouse button mappings including Other(_)
        assert_eq!(mouse_button_to_evdev(MouseButtonCode::Left), EVDEV_BTN_LEFT);
        assert_eq!(
            mouse_button_to_evdev(MouseButtonCode::Right),
            EVDEV_BTN_RIGHT
        );
        assert_eq!(
            mouse_button_to_evdev(MouseButtonCode::Middle),
            EVDEV_BTN_MIDDLE
        );
        assert_eq!(
            mouse_button_to_evdev(MouseButtonCode::Other(5)),
            EVDEV_BTN_LEFT + 5
        );
        assert_eq!(
            mouse_button_to_x11_detail(MouseButtonCode::Left),
            X11_BUTTON_LEFT
        );
        assert_eq!(
            mouse_button_to_x11_detail(MouseButtonCode::Right),
            X11_BUTTON_RIGHT
        );
        assert_eq!(
            mouse_button_to_x11_detail(MouseButtonCode::Back),
            X11_BUTTON_BACK
        );
        assert_eq!(mouse_button_to_x11_detail(MouseButtonCode::Other(0)), 1);
    }

    #[test]
    fn test_wayland_and_x11_input_injectors() {
        assert!(WaylandVirtualInputInjector::new(0, 1080).is_err());
        assert!(X11XTestInputInjector::new(1920, 0).is_err());

        let wayland = WaylandVirtualInputInjector::new(2560, 1440).unwrap();
        assert_eq!(
            wayland.pointer_protocol_interface(),
            "zwlr_virtual_pointer_manager_v1"
        );
        assert!(wayland.is_xkb_keymap_ready());

        wayland
            .inject_input(&InputEvent::MouseMove {
                monitor_id: 1,
                x: 0.25,
                y: 0.75,
                is_absolute: true,
            })
            .unwrap();
        let coords = wayland.last_pointer_coordinates();
        assert_eq!(coords.pixel_x, 640);
        assert_eq!(coords.pixel_y, 1079);

        // Relative motion delta from (640, 1079) -> (650, 1059)
        wayland
            .inject_input(&InputEvent::MouseMove {
                monitor_id: 1,
                x: 10.0,
                y: -20.0,
                is_absolute: false,
            })
            .unwrap();
        let rel_coords = wayland.last_pointer_coordinates();
        assert_eq!(rel_coords.pixel_x, 650);
        assert_eq!(rel_coords.pixel_y, 1059);

        // Relative motion from (0, 0) by (+1, +1) must land on pixel (1, 1), NOT bottom-right corner
        wayland
            .inject_input(&InputEvent::MouseMove {
                monitor_id: 1,
                x: 0.0,
                y: 0.0,
                is_absolute: true,
            })
            .unwrap();
        wayland
            .inject_input(&InputEvent::MouseMove {
                monitor_id: 1,
                x: 1.0,
                y: 1.0,
                is_absolute: false,
            })
            .unwrap();
        let near_origin = wayland.last_pointer_coordinates();
        assert_eq!(near_origin.pixel_x, 1);
        assert_eq!(near_origin.pixel_y, 1);

        wayland
            .inject_input(&InputEvent::KeyEvent {
                key_code: b'a' as u32,
                scancode: 0,
                state: ElementState::Pressed,
                modifiers: KeyModifiers {
                    shift: true,
                    ctrl: false,
                    alt: false,
                    logo: false,
                },
            })
            .unwrap();
        assert_eq!(wayland.last_key_state(), (30, 1));

        // Multi-character text input, empty text input & touch event handling
        wayland
            .inject_input(&InputEvent::TextInput {
                text: "aB".to_string(),
            })
            .unwrap();
        assert_eq!(wayland.last_key_state(), (48, 1)); // 'B' -> KEY_B (48) with shift (1)

        wayland
            .inject_input(&InputEvent::TextInput {
                text: String::new(),
            })
            .unwrap();
        wayland
            .inject_input(&InputEvent::TouchEvent {
                touch_id: 1,
                phase: TouchPhase::Started,
                x: 0.5,
                y: 0.5,
            })
            .unwrap();
        assert_eq!(wayland.injected_event_count(), 8);

        // X11 XTest fallback injector
        let x11 = X11XTestInputInjector::new(1920, 1080).unwrap();
        x11.inject_input(&InputEvent::MouseMove {
            monitor_id: 1,
            x: 0.0,
            y: 0.0,
            is_absolute: true,
        })
        .unwrap();
        x11.inject_input(&InputEvent::MouseMove {
            monitor_id: 1,
            x: 1.0,
            y: 1.0,
            is_absolute: false,
        })
        .unwrap();
        assert_eq!(x11.last_pointer_coordinates().pixel_x, 1);
        assert_eq!(x11.last_pointer_coordinates().pixel_y, 1);

        x11.inject_input(&InputEvent::MouseButton {
            button: MouseButtonCode::Left,
            state: ElementState::Pressed,
            x: 0.5,
            y: 0.5,
        })
        .unwrap();
        x11.inject_input(&InputEvent::KeyEvent {
            key_code: 0xff0d,
            scancode: 0,
            state: ElementState::Pressed,
            modifiers: KeyModifiers::default(),
        })
        .unwrap();
        x11.inject_input(&InputEvent::TouchEvent {
            touch_id: 2,
            phase: TouchPhase::Ended,
            x: 0.1,
            y: 0.9,
        })
        .unwrap();
        assert_eq!(x11.last_x11_keycode(), 28 + XKB_EVDEV_OFFSET as u8);
        x11.inject_input(&InputEvent::TextInput {
            text: "ab".to_string(),
        })
        .unwrap();
        assert_eq!(x11.last_x11_keycode(), 48 + XKB_EVDEV_OFFSET as u8); // 'b' -> 48 + 8 = 56
        assert_eq!(x11.injected_event_count(), 6);

        // Unified LinuxInputController & NativeInputInjector
        let ctrl_wl =
            LinuxInputController::new(CaptureBackendType::WaylandPipeWire, 1920, 1080).unwrap();
        assert_eq!(ctrl_wl.backend_type(), CaptureBackendType::WaylandPipeWire);
        ctrl_wl
            .inject_input(&InputEvent::TextInput {
                text: "q".to_string(),
            })
            .unwrap();
        assert_eq!(ctrl_wl.injected_event_count(), 1);

        let ctrl_x11 =
            LinuxInputController::new(CaptureBackendType::X11Fallback, 1920, 1080).unwrap();
        assert_eq!(ctrl_x11.backend_type(), CaptureBackendType::X11Fallback);
        ctrl_x11
            .inject_input(&InputEvent::MouseWheel {
                delta_x: -15.0,
                delta_y: 30.0,
                unit: WheelUnit::Pixel,
            })
            .unwrap();
        assert_eq!(ctrl_x11.injected_event_count(), 1);

        let native_wl = crate::NativeInputInjector::new(CaptureBackendType::WaylandPipeWire);
        native_wl
            .inject_input(&InputEvent::MouseMove {
                monitor_id: 1,
                x: 0.5,
                y: 0.5,
                is_absolute: true,
            })
            .unwrap();
        let native_x11 = crate::NativeInputInjector::new(CaptureBackendType::X11Fallback);
        native_x11
            .inject_input(&InputEvent::KeyEvent {
                key_code: b'z' as u32,
                scancode: 0,
                state: ElementState::Released,
                modifiers: KeyModifiers::default(),
            })
            .unwrap();
    }
}
