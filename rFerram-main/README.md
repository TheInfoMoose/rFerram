# rFerram

> *Just like the iron framework that supports large stained-glass windows in stone architecture that is its namesake, Ferram supports a solid window comprised of numerous disparate pieces.*

**rFerram** is a hyper-optimized, zero-copy, cross-platform remote display application built from the ground up in pure Rust. It is designed to replace legacy, TCP-bound remote desktop tools (such as VNC, Remmina, and traditional RDP) by eliminating head-of-line blocking, high CPU overhead, and proprietary cloud dependencies.

Built natively for **COSMIC Desktop Environment** and modern desktop platforms using **`libcosmic`** and **`wgpu`**, rFerram combines UDP-backed **QUIC transport** (`quinn` + `rustls`), zero-copy hardware video pipelines (AV1, H.265, H.264), native VPN adapter binding (`socket2` + `if-addrs`), and snapshot-backed file synchronization from **`ST_Rust_Age`**.

---

## Key Features

* **Zero-Copy Hardware Video Pipeline**: Direct GPU frame buffer capture to hardware encoders (Apple Silicon / VideoToolbox on macOS, DXGI on Windows, DMABUF / Smithay / PipeWire on Linux) streaming at 60+ FPS.
* **"Bring Your Own Network" (BYON) Transport**: Operates as a direct-connect peer-to-peer utility over UDP-backed **QUIC** (`quinn` + `rustls`), bypassing cloud relays and explicitly binding to target VPN adapters (WireGuard `wg0`, Tailscale `tailscale0`, macOS `utun3`, UniFi) via `socket2`.
* **Native COSMIC & Cross-Platform UI**: Features a sleek, responsive client interface built with `libcosmic` and `cosmic-config`, leveraging `wgpu` (Vulkan, Metal, DirectX 12) for hardware-accelerated rendering and frosted glass aesthetics.
* **Low-Latency Input & Datagrams**: Dispatches mouse coordinates, touch gestures, and keyboard scancodes over un-ordered QUIC datagrams, ensuring input flow remains unaffected by lost video packets.
* **Duplex Audio Synchronization**: Captures host speaker audio (PipeWire, WASAPI, CoreAudio) and compresses it into 20ms 48kHz Opus frame chunks alongside bi-directional microphone passthrough.
* **Native Bi-Directional Clipboard Sync**: Uses `arboard` for instant cross-platform clipboard synchronization (text, RGBA/PNG images, file lists) with automated loopback echo suppression.
* **Snapshot-Backed File Synchronization**: Integrates directly with `ST_Rust_Age` (`simply-transfer-core`, `simply-transfer-crypto`, and `simply-transfer-snapshots`) and embedded `russh` P2P channels to safely transfer open or locked files via native OS filesystem snapshots (APFS, VSS, LVM/ZFS/btrfs) with streaming SHA-256 verification.

---

## Workspace Crate Architecture

rFerram is structured as a 7-crate Cargo workspace ensuring strict separation of concerns, memory safety, and high modularity:

| Crate | Primary Responsibility | Key Dependencies |
| :--- | :--- | :--- |
| **`rferram-ui`** | COSMIC desktop viewer graphical application & layout | `libcosmic`, `cosmic-config`, `wgpu` |
| **`rferram-client`** | Remote viewer session engine & GPU frame presentation | `wgpu`, `tokio`, `rferram-net` |
| **`rferram-server`** | Elevated host daemon & multi-backend frame capturer | `wayland-client`, `windows`, `objc2-metal`, `wgpu` |
| **`rferram-net`** | QUIC/UDP transport layer & VPN adapter binding | `quinn`, `rustls`, `socket2`, `if-addrs`, `rcgen` |
| **`rferram-channels`** | Virtual channels (Clipboard & `ST_Rust_Age` snapshots) | `arboard`, `simply-transfer-*`, `russh`, `sha2` |
| **`rferram-protocol`** | Zero-copy binary wire envelope & message structs | `serde`, `postcard` |
| **`rferram-common`** | NIST credential vault, session models, & distro detection | `keyring`, `ed25519-dalek`, `zeroize`, `simply-transfer-crypto` |

---

## Platform Support & Backends

| Subsystem | Linux (Debian, RHEL, SUSE, Arch / COSMIC) | Windows | macOS |
| :--- | :--- | :--- | :--- |
| **Host Capture** | Wayland DMABUF / Smithay / PipeWire / X11 SHM | DXGI Desktop Duplication | ScreenCaptureKit (`IOSurface`) |
| **Hardware Encoding** | VAAPI / NVENC / AV1 | NVENC / AMF / QuickSync | VideoToolbox (Apple Silicon / Intel) |
| **Graphics API** | Vulkan / OpenGL (`wgpu`) | Direct3D 12 (`wgpu`) | Metal (`wgpu`) |
| **Input Injection** | Wayland Virtual Pointer & Keyboard / XTest | Win32 `SendInput` API | `CGEvent` / Accessibility APIs |
| **System Audio** | PipeWire / ALSA Loopback | WASAPI Loopback | CoreAudio Loopback |
| **Filesystem Snapshots** | LVM / ZFS / btrfs (`simply-transfer-snapshots`) | Volume Shadow Copy (VSS) | APFS Snapshots |

---

## Building & Installation

### Prerequisites

Ensure you have the Rust toolchain installed (MSRV: `1.93`, `edition = "2024"`).

#### Platform Dependencies:
* **Debian / Ubuntu / Pop!_OS**:
  ```bash
  sudo apt-get install pkg-config libwayland-dev libdrm-dev libgbm-dev libxkbcommon-dev libdbus-1-dev
  ```
* **RHEL / Fedora / Rocky / AlmaLinux**:
  ```bash
  sudo dnf install pkgconf-pkg-config wayland-devel libdrm-devel mesa-libgbm-devel libxkbcommon-devel dbus-devel
  ```
* **openSUSE / SLES**:
  ```bash
  sudo zypper install pkg-config wayland-devel libdrm-devel libgbm-devel libxkbcommon-devel dbus-1-devel
  ```
* **Arch Linux / CachyOS**:
  ```bash
  sudo pacman -S --needed pkgconf wayland libdrm mesa libxkbcommon dbus
  ```
* **macOS**: Xcode Command Line Tools (`Metal.framework`, `CoreGraphics.framework`, `ScreenCaptureKit.framework`)
* **Windows**: Windows 10/11 SDK (DXGI & D3D11 headers)

### Compilation

Build the workspace release binaries against the locked dependency graph:

```bash
cargo build --workspace --release --locked
```

The compiled binaries will be located in `target/release/`:
* `rferram-ui`: The main COSMIC / cross-platform viewer application.
* `rferram-server`: The elevated host daemon service.

---

## Testing & CI/CD Pipeline

rFerram enforces strict code quality, security auditing, and cross-platform verification:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

---

## License

This project is licensed under the **GPL-3.0-only License**.
