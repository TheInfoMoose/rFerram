---
name: rferram-st-channels
description: >-
  Procedures and invariants for implementing and modifying rferram-channels
  (arboard cross-platform clipboard with loopback echo suppression, ST_Rust_Age
  snapshot-backed file transfers with SHA-256 verification, embedded russh P2P
  OOB Ed25519 key ingestion, device passthrough, and session elevation).
---

# rFerram Out-of-Band Virtual Channels Skill (`rferram-channels`)

Activate this skill when working on Phase 3 (`crates/rferram-channels`).

## 1. Clipboard Synchronization & Echo Suppression (`ClipboardChannelHandler`)

- **Headless / Display Resilience**:
  - `arboard::Clipboard::new()` can fail in headless CI environments or Wayland sessions without an active display server. Store `Option<Clipboard>` inside `Arc<Mutex<Option<Clipboard>>>` and log a `debug!` message on initialization failure so unit tests never panic in headless runners.
- **Two-Layer Loopback Suppression**:
  1. `is_suppressed: Arc<AtomicBool>`: Set to `true` (`Ordering::SeqCst`) before writing remote clipboard contents to the local OS pasteboard, and reset after a short settling window (`100ms`). The polling loop must skip iterations while `is_suppressed` is `true`.
  2. `last_sent_content: Arc<Mutex<ClipboardContent>>`: Track the last text string, `ImageHash(u64)`, or file list written or sent so identical consecutive payloads are never re-broadcast across QUIC.
- **Image Dimensions**:
  - Preserve actual image `width` and `height` in `ClipboardPayload::Image { width, height, format, data }` when reading/writing `arboard::ImageData`.

## 2. `ST_Rust_Age` Snapshot File Transfers (`SimplyTransferChannelHandler`)

- **Native Snapshot Drivers**:
  - Use `simply_transfer_snapshots::{get_native_driver, list_local_volumes, resolve_owning_volume}` from the vendored `ST_Rust_Age` workspace (`vendor/ST_Rust_Age`).
  - Resolve the owning volume for `source_path`, attempt `driver.create_snapshot(&owning_vol)`, and map the source path via `driver.resolve_snapshot_path_for_volume(...)`. If snapshot creation fails (e.g., unprivileged user on Linux/Windows without LVM/VSS privileges), fall back gracefully to the live path with `is_snapshot_locked = false`.
- **Streaming SHA-256 Validation**:
  - Validate every incoming transfer header and chunk using `simply_transfer_crypto::hash::compute_sha256_bytes(data)` and record the hex digest on `SnapshotTransferSession`.

## 3. Embedded `russh` P2P OOB Key Ingestion

- `Cargo.toml` provides `russh = { version = "0.64.1", default-features = false, features = ["flate2", "aws-lc-rs"] }`.
- When implementing or extending the out-of-band (OOB) P2P SSH key ingestion handler:
  - Never shell out to OS `sshd` or modify `~/.ssh/authorized_keys`.
  - Ingest peer `ed25519_dalek::VerifyingKey` public keys directly into `NistVault` memory/keystore for subsequent QUIC session authentication.

## 4. Verification
Run the quality gate for `rferram-channels`:
```bash
/srv/devshare/rFerram/.agents/skills/rferram-quality-gate/scripts/verify-crate.sh rferram-channels
```
