---
name: rferram-cosmic-iced-ui
description: >-
  Procedures and invariants for implementing and modifying rferram-ui and vendored
  libcosmic (Elm architecture, symbolic vector icons, ButtonStyle/TogglerStyle
  variants, two-tier header, portrait connection cards, popup context menus,
  inline Add Desktop modal, two-pane split drawers, responsive scaling, and
  frosted glass WGSL shaders).
---

# rFerram COSMIC & Iced UI Architecture Skill (`rferram-ui`)

Activate this skill when working on Phase 6 (`crates/rferram-ui`).

## 1. Local Reference Sources & Visual Wireframes

Before modifying UI components or layout structures, inspect:
- **Visual Layout Mockup**: `/home/theinfomoose/Documents/rFerram/Mockup/rFerramLayout-PNG.drawio.png`, `/home/theinfomoose/Documents/rFerram/Mockup/rFerramLayout.svg`, and `/home/theinfomoose/Documents/rFerram/DevDocumentation/UI_Fix.md`
- **Component Overview**: `/home/theinfomoose/Documents/rFerram/rFerram-UI-Components.md`
- **Vendored `libcosmic` Crate**: `/srv/devshare/rFerram/vendor/libcosmic`
- **Upstream Reference Archives**:
  - `/home/theinfomoose/Documents/Cosmic Elements/libcosmic-master.zip`
  - `/home/theinfomoose/Documents/Cosmic Elements/cosmic-settings-master.zip`
  - `/home/theinfomoose/Documents/Iced/iced-master.zip`

## 2. Elm Architecture Pattern (`cosmic::Application`)

- **Strict State Separation**:
  - `RFerramApp` holds application state (`core: Core`, `runtime_env: RuntimeEnvironment`, `active_nav: NavItem`, `connection_cards: Vec<ProfileUiState>`, `active_context_menu: Option<String>`, `quick_add_menu_open: bool`, `add_modal_state: AddDesktopModalState`, drawer flags).
  - `enum Message` defines all user interactions, modal field updates, drawer selections, popup menu actions, `WindowResized(u32, u32)`, and async session updates (`WireMessageReceived`, `SessionStatusChanged`).
  - `RFerramApp::update(&mut self, message: Message) -> Command<Message>` mutates state deterministically and returns non-blocking `Task`/`Command` handles.
  - `RFerramApp::on_resize(&mut self, width: u32, height: u32) -> Option<Message>` keeps `core.window_width` and `core.window_height` synchronized and scales the connection grid via `responsive_grid_columns()` (`1` col `<768px` or List mode, `2` cols `768..1199px`, `3` cols `1200..1799px`, `4` cols `>=1800px`).
  - `RFerramApp::view(&self) -> Element<'_, Message>` is a pure function of state composed from modular view helpers:
    - `view_header()`
    - `view_sidebar()`
    - `view_connection_grid()`
    - `view_card_context_menu()` & `view_quick_add_menu()`
    - `view_add_desktop_modal()`
    - `view_settings_drawer()`
    - `view_help_drawer()`

## 3. Symbolic Vector Icons, Button Styles & Container Hierarchy (Zero Placeholders)

- **No Generic Text Pills or Placeholder Boxes**:
  - Never use plain ASCII text buttons for icon actions (`"+"`, `"..."`, `"*"`, `"o"`, `"Grid"`, `"List"`, `"Settings"`, `"?"`). Always use semantic `libcosmic` builders (`icon_button`, `nav_button`, `segmented_button`, `menu_button`, `checkbox`, `icon::from_name`) with `ButtonStyle` (`Icon`, `Segmented`, `NavRow`, `CardTrigger`, `MenuItem`, `Primary`, `Standard`) and `TogglerStyle::Checkbox`.
  - Every symbolic icon in `cosmic::SYMBOLIC_ICON_NAMES` must render crisp `tiny-skia` vector paths in `vendor/libcosmic/src/raster.rs` (`paint_icon`):
    - Header/Toolbar: `com.theinfomoose.rFerram-symbolic`, `add-symbolic`, `view-grid-symbolic`, `view-list-symbolic`, `view-sort-descending-symbolic`, `system-search-symbolic`, `preferences-system-symbolic`, `help-browser-symbolic`.
    - Sidebar & Cards: `emblem-favorite-symbolic`, `folder-symbolic`, `folder-remote-symbolic`, `object-select-symbolic`, `view-more-horizontal-symbolic`.
    - Settings Drawer (`SettingsSection::icon_name`): `preferences-system-symbolic` (`General`), `input-keyboard-symbolic` (`Keyboard`), `video-display-symbolic` (`Resolution | Scaling`), `user-bookmarks-symbolic` (`Groups`), `avatar-default-symbolic` (`Profiles`), `network-vpn-symbolic` (`VPN`).
    - Help Drawer (`HelpSection::icon_name`): `document-open-symbolic` (`Documentation`), `tools-symbolic` (`Troubleshooting`), `dialog-warning-symbolic` (`Report Bug`), `lab-symbolic` (`Feature Request`).
- **Mandatory Container Classes (`vendor/libcosmic/src/layout.rs`)**:
  - Header: `CLASS_HEADER` wrapping `CLASS_TOP_TABBAR` and `CLASS_TOOLBAR` (with `CLASS_SEGMENTED_PILL` and `CLASS_SEARCH_PILL`).
  - Sidebar & Grid: `CLASS_SIDEBAR`, `CLASS_GROUP_SECTION`, `CLASS_GROUP_BADGE` (above a full-width `divider()`), and portrait `CLASS_CONNECTION_CARD` tiles (containing top-right `CardTrigger` buttons, `rferram-card-preview`, and bottom-left friendly name + `"Saved Profile"` subtext).
  - Floating Menus: `CLASS_ADD_MENU` (`Add`, `Import`, `Export`) and `CLASS_CONTEXT_MENU` (`Connect`, `Configure`, `Duplicate`, `Export`, `Delete`).
  - Modal & Drawers: `CLASS_ADD_DESKTOP_MODAL` (with `CLASS_FORM_ROW`, `CLASS_MODAL_TABS`, `CLASS_MODAL_TAB_FRAME`, `CLASS_MODAL_FOOTER`) and two-pane split drawers `CLASS_SETTINGS_DRAWER` / `CLASS_HELP_DRAWER` (with `CLASS_DRAWER_NAV` on the left and `CLASS_DRAWER_CONTENT` on the right).

## 4. Wireframe Parity Requirements (Mandatory Labels)

To maintain strict compatibility with UI design specifications and unit tests (`test_ui_modal_workflow_and_mockup_elements`), ensure the following exact labels and controls remain present in the view tree when the "Add Desktop" modal is open:
- Modal Title: `"Add Desktop"`
- Top-level Global Inputs (`CLASS_FORM_ROW`):
  - `"Connection:"` with placeholder `"Host name or IP address"`
  - `"Credentials:"` with dropdown option `"Ask when required"`
- Modal Tabs (`CLASS_MODAL_TABS`): `"General"`, `"Display"`, `"Devices | Audio"`, `"Folders"`
- General Tab Controls (`CLASS_MODAL_TAB_FRAME`):
  - `"Friendly Name:"` with placeholder `"Optional local only name"`
  - `"Group:"` dropdown (`"Group 1"`)
  - `"VPN:"` dropdown (`"None"`, `"WireGuard"`, `"Tailscale"`, `"UniFi VPN"`) mapping to `VpnAdapterBinding`
  - Square Checkboxes (`TogglerStyle::Checkbox`): `"Reconnect if the session is dropped"` and `"Connect to an admin session"`
- Action Buttons (`CLASS_MODAL_FOOTER`): `"Cancel"` and `"Add"`

## 5. Verification
Run both the `libcosmic` test suite and the `rferram-ui` quality gate:
```bash
cargo test --manifest-path /srv/devshare/rFerram/Cargo.toml -p libcosmic --all-targets --locked
/srv/devshare/rFerram/.agents/skills/rferram-quality-gate/scripts/verify-crate.sh rferram-ui
```
