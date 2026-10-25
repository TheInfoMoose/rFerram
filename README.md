# rFerram

> *Just like the iron framework that supports large stained-glass windows in stone architecture that is its namesake, Ferram supports a solid window comprised of numerous disparate pieces.*

**rFerram** is a hyper-optimized, zero-copy, cross-platform remote display application built from the ground up in pure Rust. It is designed to replace legacy, TCP-bound remote desktop tools (such as VNC, Remmina, and traditional RDP) by eliminating head-of-line blocking, high CPU overhead, and proprietary cloud dependencies.

Built natively for modern desktop environments using **** and ****, rFerram combines UDP-backed **QUIC transport**, zero-copy hardware video pipelines (AV1, H.265, H.264), native VPN adapter binding, and snapshot-backed file synchronization from ****.

---

## Key Features

* **Zero-Copy Hardware Video Pipeline**: Direct GPU frame buffer capture to hardware encoders (Apple Silicon / VideoToolbox on macOS, DXGI on Windows, DMABUF / Smithay / PipeWire on Linux) streaming at 60+ FPS.
* **"Bring Your Own Network" (BYON) Transport**: Operates as a direct-connect peer-to-peer utility over UDP-backed **QUIC** ( + ), bypassing cloud relays and explicitly binding to target VPN adapters (WireGuard, Tailscale, UniFi) via .
* **Modern COSMIC & Cross-Platform UI**: Features a sleek, responsive client interface built with  and , leveraging  (Metal, Vulkan, DX12) for smooth rendering and frosted glass aesthetics.
* **Low-Latency Input & Datagrams**: Dispatches mouse coordinates, touch gestures, and keyboard scancodes over un-ordered QUIC datagrams, ensuring input flow remains unaffected by lost video packets.
* **Duplex Audio Synchronization**: Captures host speaker audio (PipeWire, WASAPI, CoreAudio) and compresses it into 20ms Opus frame chunks alongside bi-directional microphone passthrough.
* **Native Bi-Directional Clipboard Sync**: Uses  for instant cross-platform clipboard synchronization (text, PNG images, file lists) with automated loopback echo suppression.
* **Snapshot-Backed File Synchronization**: Integrates directly with  ( and ) to safely transfer open or locked files via native OS filesystem snapshots (APFS, VSS, LVM/ZFS).

---

## Workspace Crate Architecture

rFerram is structured as a multi-crate Cargo workspace ensuring strict separation of concerns, memory safety, and high modularity:

| Crate | Primary Responsibility | Key Dependencies |
| :--- | :--- | :--- |
| **** | COSMIC desktop viewer graphical application & layout | , ,  |
| **** | Remote viewer session engine & GPU frame presentation | , ,  |
| **** | Elevated host daemon & multi-backend frame capturer | , , ,  |
| **** | QUIC/UDP transport layer & VPN adapter binding | , ,  |
| ****| Virtual channels (Clipboard &  snapshots) | ,  |
| ****| Zero-copy binary wire envelope & message structs | ,  |
| ****  | NIST credential vault, session models, & diagnostics | ,  |

---

## Platform Support & Backends

| Subsystem | Linux (Wayland / X11) | Windows | macOS |
| :--- | :--- | :--- | :--- |
| **Host Capture** | Smithay / PipeWire DMABUF | DXGI Desktop Duplication | ScreenCaptureKit |
| **Hardware Encoding** | VAAPI / NVENC | NVENC / AMF / QuickSync | VideoToolbox (Apple Silicon / Intel) |
| **Graphics API** | Vulkan / OpenGL | Direct3D 12 | Metal |
| **Input Injection** | Smithay Virtual Pointer /  |  API |  / Accessibility APIs |
| **System Audio** | PipeWire Loopback | WASAPI Loopback | CoreAudio Loopback |
| **Filesystem Snapshots**| LVM / ZFS / btrfs | Volume Shadow Copy (VSS) | APFS Snapshots |

---

## Building & Installation

### Prerequisites

Ensure you have the Rust toolchain installed (MSRV: ).

#### Platform Dependencies:
* **Linux**: , , , 
* **macOS**: Xcode Command Line Tools (, , )
* **Windows**: Windows 10/11 SDK (DXGI headers)

### Compilation

Clone the repository and build the workspace release binaries:



The compiled binaries will be located in target release directory:
* : The main viewer application.
* : The elevated host daemon service.

---

## Testing & CI/CD Pipeline

rFerram enforces strict code quality, security auditing, and cross-platform verification.

### Running Unit Tests

Run the workspace test suite locally:



### CI Pipeline Standards

The repository includes automated GitHub Actions workflows () performing:
* **Security Audits**: Automated CVE checks using  and license compliance via .
* **Linting**: Strict  and .
* **Cross-Platform Matrix**: Parallel test execution across , , and .

---

## Security & Privacy

rFerram strictly adheres to NIST credential management standards:
* **Pure Rust Crypto**: All transport encryption is powered by audited  (TLS 1.3), eliminating C-library vulnerabilities.
* **Native Vault Integration**: Host keys and authentication tokens are secured via OS credential vaults ().
* **Direct Connections**: No proprietary relay servers ever intercept or touch your display streams.

---

## License

This project is licensed under the **GPL-3.0 License**.
