---
name: rferram-cosmic-iced-ui
description: >-
  Procedures and invariants for implementing and modifying rferram-ui
  (libcosmic and iced Elm architecture, cosmic-config persistence, Add Desktop
  modal wireframe parity, VPN adapter binding selector, context drawers, and
  frosted glass WGSL shaders).
---

# rFerram COSMIC & Iced UI Architecture Skill (`rferram-ui`)

Activate this skill when working on Phase 6 (`crates/rferram-ui`).

## 1. Local Reference Sources & Visual Wireframes

Before modifying UI components or layout structures, inspect:
- **Visual Layout Mockup**: `/home/theinfomoose/Documents/rFerram/Mockup/rFerramLayout-PNG.drawio.png` and `/home/theinfomoose/Documents/rFerram/rFerram-UI-Components.md`
- **Vendored `libcosmic` Crate**: `/srv/devshare/rFerram/vendor/libcosmic`
- **Upstream Reference Archives**:
  - `/home/theinfomoose/Documents/Cosmic Elements/libcosmic-master.zip`
  - `/home/theinfomoose/Documents/Cosmic Elements/cosmic-settings-master.zip`
  - `/home/theinfomoose/Documents/Iced/iced-master.zip`

## 2. Elm Architecture Pattern (`cosmic::Application`)

- **Strict State Separation**:
  - `RFerramApp` holds application state (`core: Core`, `runtime_env: RuntimeEnvironment`, `active_nav: NavItem`, `connection_cards: Vec<ProfileUiState>`, `add_modal_state: AddDesktopModalState`, drawer flags).
  - `enum Message` defines all user interactions, modal field updates, drawer selections, and async session updates (`WireMessageReceived`, `SessionStatusChanged`).
  - `RFerramApp::update(&mut self, message: Message) -> Command<Message>` mutates state deterministically and returns non-blocking `Task`/`Command` handles.
  - `RFerramApp::view(&self) -> Element<'_, Message>` is a pure function of state composed from modular view helpers:
    - `view_header()`
    - `view_sidebar()`
    - `view_connection_grid()`
    - `view_add_desktop_modal()`
    - `view_settings_drawer()`
    - `view_help_drawer()`

## 3. Wireframe Parity Requirements (Mandatory Labels)

To maintain strict compatibility with UI design specifications and unit tests (`test_ui_modal_workflow_and_mockup_elements`), ensure the following exact labels and controls remain present in the view tree when the "Add Desktop" modal is open:
- Modal Title: `"Add Desktop"`
- Top-level Global Inputs:
  - `"Connection:"` with placeholder `"Host name or IP address"`
  - `"Credentials:"` with dropdown option `"Ask when required"`
- Modal Tabs: `"General"`, `"Display"`, `"Devices | Audio"`, `"Folders"`
- General Tab Controls:
  - `"Friendly Name:"` with placeholder `"Optional local only name"`
  - `"Group:"` dropdown (`"Group 1"`)
  - `"VPN:"` dropdown (`"None"`, `"WireGuard"`, `"Tailscale"`, `"UniFi VPN"`) mapping to `VpnAdapterBinding`
  - Togglers: `"Reconnect if the session is dropped"` and `"Connect to an admin session"`
- Action Buttons: `"Cancel"` and `"Add"`

## 4. Verification
Run the quality gate for `rferram-ui`:
```bash
/srv/devshare/rFerram/.agents/skills/rferram-quality-gate/scripts/verify-crate.sh rferram-ui
```
