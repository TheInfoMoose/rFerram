---
name: rferram-quality-gate
description: >-
  Enforces the mandatory rFerram formatting, clippy linting, unit/integration test,
  and daemon smoke-test quality gate. Activate this skill at the conclusion of every
  implementation task or whenever verifying workspace build health.
---

# rFerram Quality Gate & Verification Skill

Every atomic implementation task in the **rFerram** workspace must pass this verification sequence before completion.

## 1. Automated Verification Helper

Run the bundled verification script from `/srv/devshare/rFerram`:

```bash
/srv/devshare/rFerram/.agents/skills/rferram-quality-gate/scripts/verify-crate.sh <crate-name>
```

* Omit `<crate-name>` (or pass `--all`) to run tests across the entire 7-crate workspace.
* Example for `rferram-net`:
  ```bash
  /srv/devshare/rFerram/.agents/skills/rferram-quality-gate/scripts/verify-crate.sh rferram-net
  ```

## 2. Manual Step-by-Step Verification Sequence

If running individual steps manually inside `/srv/devshare/rFerram`:

1. **Format Code (`rustfmt` Edition 2024)**:
   ```bash
   cargo fmt --all
   cargo fmt --all -- --check
   ```
2. **Workspace Linter Gate (Zero Warnings Allowed)**:
   ```bash
   cargo clippy --workspace --all-targets --locked -- -D warnings
   ```
3. **Targeted Crate Unit & Integration Tests**:
   ```bash
   cargo test -p <target-crate> --all-targets --locked
   ```
4. **Full Workspace Regression Check**:
   ```bash
   cargo test --workspace --all-targets --locked
   ```
5. **Host Daemon Smoke Test (When modifying `rferram-server` or `rferram-net`)**:
   ```bash
   RFERRAM_SMOKE_TEST=1 RFERRAM_BIND_ADDR=127.0.0.1:0 cargo run -p rferram-server --locked
   ```

## 3. Troubleshooting Common Gate Failures

- **Clippy Let-Chains / Edition 2024 Idioms**: Rust 1.93 (`edition = "2024"`) enforces modern collapsible `if let` chains and `is_multiple_of()`. Follow clippy's exact suggestion rather than adding `#[allow(...)]`.
- **Re-Export Regressions**: If extracting a module (e.g. `src/socket.rs`) breaks downstream crates during `cargo clippy --workspace`, ensure `pub mod socket; pub use socket::*;` is present in `src/lib.rs`.
- **Unannotated `unsafe`**: Ensure every `unsafe` block has a preceding `// SAFETY:` comment documenting why the FFI invariants hold.
