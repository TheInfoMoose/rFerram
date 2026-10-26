# Solo Developer Branching & Development Guide

This guide outlines the branching strategy, commit conventions, local quality gates, and release workflows for **rFerram**. Even when operating as a solo developer, adhering to strict branch hygiene and automated CI checks maintains main-branch stability and ensures zero-regression development.

---

## 1. Branching Strategy

The repository follows a modified **GitHub Flow** tailored for modular Rust workspace development:

```
 main (Protected / Stable / Locked Releases)
  ├── feature/quic-vpn-binding
  ├── feature/screencapturekit-mac
  └── fix/clipboard-loopback-echo
```

### Branch Naming Conventions
* **`main`**: The canonical production branch. Must remain green, passing all CI checks at all times.
* **`feature/<short-description>`**: New crate features or architectural additions (e.g., `feature/quic-transport`, `feature/screencapturekit`).
* **`fix/<issue-description>`**: Targeted bug fixes or regression patches (e.g., `fix/clipboard-loopback-echo`).
* **`refactor/<component-name>`**: Internal code cleanup, trait restructures, or optimization work.
* **`docs/<scope>`**: Documentation updates, README refinements, or architecture specs.
* **`release/vX.Y.Z`**: Staging branch for version bumping, tag generation, and final release validation.

---

## 2. Local Pre-Merge Quality Checklist

Before merging any topic branch into `main`, run the following verification suite locally to match CI requirements:

```bash
# 1. Check code formatting
cargo fmt --check

# 2. Run Clippy across all workspace crates and targets (strict mode)
cargo clippy --workspace --all-targets -- -D warnings

# 3. Run full unit & integration test suite against locked dependencies
cargo test --locked

# 4. Security vulnerability & license check
cargo audit
cargo deny check
```

---

## 3. Commit Message Standards

Adhere to **Conventional Commits** to keep git history clean, readable, and easy to audit:

* `feat(net)`: add socket2 interface binding for WireGuard and Tailscale adapters
* `fix(channels)`: suppress loopback echo on native clipboard sync
* `refactor(server)`: extract macOS ScreenCaptureKit logic into dedicated host encoder module
* `test(protocol)`: add roundtrip serialization tests for VideoFrameMessage
* `ci`: update GitHub Actions cross-platform matrix to Rust 1.80+

---

## 4. PR & Self-Review Protocol

Even when self-merging pull requests:

1. **Open a PR**: Push feature branches and open a PR against `main` to trigger the automated GitHub Actions matrix (`ubuntu-latest`, `windows-latest`, `macos-latest`).
2. **Verify CI Status**: Ensure security audits (`cargo-audit`, `cargo-deny`), linters (`clippy`), and cross-platform tests pass.
3. **Squash and Merge**: Use **Squash and Merge** for feature branches to keep `main` history clean and linear.

---

## 5. Dependency Lock Hygiene

To prevent upstream incremental releases from breaking builds:

* Always commit `Cargo.lock`.
* Run builds with `--locked` in CI.
* Review automated Dependabot PRs in isolated feature branches rather than floating main dependencies.
