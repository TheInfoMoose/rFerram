//! Operating system display capture and input injection backends.

pub mod linux_capture;
pub mod linux_input;
pub mod macos;
pub mod windows;

pub use linux_capture::{
    DRM_FORMAT_ABGR8888, DRM_FORMAT_ARGB8888, DRM_FORMAT_NV12, DRM_FORMAT_XBGR8888,
    DRM_FORMAT_XRGB8888, DmaBufPlaneDescriptor, FrameCapturer, GbmDmabufSurface,
    LinuxCaptureController, WaylandDmabufCapturer, X11ShmCapturer, map_drm_fourcc_to_pixel_format,
};
pub use linux_input::{
    EVDEV_BTN_EXTRA, EVDEV_BTN_LEFT, EVDEV_BTN_MIDDLE, EVDEV_BTN_RIGHT, EVDEV_BTN_SIDE,
    LinuxInputController, NormalizedPointerCoords, WaylandVirtualInputInjector, X11_BUTTON_BACK,
    X11_BUTTON_FORWARD, X11_BUTTON_LEFT, X11_BUTTON_MIDDLE, X11_BUTTON_RIGHT,
    X11_BUTTON_SCROLL_DOWN, X11_BUTTON_SCROLL_LEFT, X11_BUTTON_SCROLL_RIGHT, X11_BUTTON_SCROLL_UP,
    X11XTestInputInjector, XKB_EVDEV_OFFSET, encode_xkb_modifiers_mask, evdev_to_x11_keycode,
    mouse_button_to_evdev, mouse_button_to_x11_detail, normalize_pointer_coordinates,
    translate_key_to_evdev_scancode,
};
pub use macos::{
    CG_EVENT_FLAG_MASK_ALTERNATE, CG_EVENT_FLAG_MASK_COMMAND, CG_EVENT_FLAG_MASK_CONTROL,
    CG_EVENT_FLAG_MASK_SHIFT, CV_PIXEL_FORMAT_32BGRA, CV_PIXEL_FORMAT_32RGBA, CV_PIXEL_FORMAT_420F,
    CV_PIXEL_FORMAT_420V, CV_PIXEL_FORMAT_Y420, CV_PIXEL_FORMAT_Y444, CgMouseEventKind,
    IoSurfacePlaneDescriptor, MacOsCgEventInjector, MacOsIoSurfaceDescriptor,
    MacOsScreenCaptureKitCapturer, TranslatedCgEvent, map_cv_pixel_format_to_wire,
    map_scancode_to_macos_keycode, translate_input_event_to_cg, translate_modifiers_to_cg_flags,
};
pub use windows::{
    DXGI_FORMAT_B8G8R8A8_UNORM_RAW, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB_RAW, DXGI_FORMAT_NV12_RAW,
    DXGI_FORMAT_R8G8B8A8_UNORM_RAW, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB_RAW, DxgiSubresourceLayout,
    TranslatedWin32Input, VK_CONTROL_CODE, VK_LWIN_CODE, VK_MENU_CODE, VK_SHIFT_CODE,
    WIN32_KEYEVENTF_EXTENDEDKEY, WIN32_KEYEVENTF_KEYUP, WIN32_KEYEVENTF_SCANCODE,
    WIN32_KEYEVENTF_UNICODE, WIN32_MOUSEEVENTF_ABSOLUTE, WIN32_MOUSEEVENTF_HWHEEL,
    WIN32_MOUSEEVENTF_LEFTDOWN, WIN32_MOUSEEVENTF_LEFTUP, WIN32_MOUSEEVENTF_MIDDLEDOWN,
    WIN32_MOUSEEVENTF_MIDDLEUP, WIN32_MOUSEEVENTF_MOVE, WIN32_MOUSEEVENTF_RIGHTDOWN,
    WIN32_MOUSEEVENTF_RIGHTUP, WIN32_MOUSEEVENTF_VIRTUALDESK, WIN32_MOUSEEVENTF_WHEEL,
    WIN32_MOUSEEVENTF_XDOWN, WIN32_MOUSEEVENTF_XUP, WIN32_WHEEL_DELTA, WIN32_XBUTTON1,
    WIN32_XBUTTON2, WindowsDxgiCapturer, WindowsDxgiSurfaceDescriptor, WindowsSendInputInjector,
    encode_win32_modifier_transitions, map_dxgi_format_to_wire, map_scancode_to_win32_vk,
    normalize_win32_absolute_coords, normalize_win32_virtual_desktop_coords,
    translate_input_event_to_win32,
};
