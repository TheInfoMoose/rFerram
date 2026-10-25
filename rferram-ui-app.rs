//! `rferram-ui`: Graphical Client View & Message Loop (v2 - Cross-Platform including macOS)
//!
//! Implements the primary `libcosmic` / `iced` Application state engine,
//! desktop connection card grids, "Add Desktop" setup modal (VPN dropdown, ST v2 folder shares,
//! PipeWire/CoreAudio audio loopbacks), settings/diagnostics drawers, and async transport message loops.

use std::collections::HashMap;
use std::sync::Arc;

use cosmic::app::{Command, Core};
use cosmic::widget::{button, container, grid, icon, row, text, text_input, column};
use cosmic::{Element, Theme};

use rferram_common::{ConnectionProfile, HostCredentials};
use rferram_protocol::{VideoCodec, WireMessage};

/// Main Navigation Sidebar Item
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavItem {
    AllDesktops,
    Favorites,
    Group(u32),
    GlobalSettings,
    Diagnostics,
}

/// Modal Tab Options inside the "Add Desktop" Modal Dialog
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddDesktopModalTab {
    General,
    Credentials,
    VpnSelect,
    Folders,
    DevicesAudio,
}

/// VPN Adapter Binding Option (Crucial for "Bring Your Own Network" Model)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VpnAdapterBinding {
    Direct,
    WireGuard(String), // e.g., wg0 on Linux or utun3 on macOS
    Tailscale,
    UniFiVpn,
    CustomAdapter(String),
}

/// Active Connection Profile State in UI
#[derive(Debug, Clone)]
pub struct ProfileUiState {
    pub profile: ConnectionProfile,
    pub is_favorite: bool,
    pub vpn_binding: VpnAdapterBinding,
    pub active_fps: Option<f32>,
    pub active_rtt_ms: Option<f32>,
}

/// State for the "Add Desktop" Configuration Modal
#[derive(Debug, Default, Clone)]
pub struct AddDesktopModalState {
    pub active_tab: AddDesktopModalTab,
    pub name_input: String,
    pub host_input: String,
    pub group_input: String,
    pub auto_reconnect: bool,
    pub admin_session_takeover: bool,
    pub username_input: String,
    pub password_input: String,
    pub ask_credentials_on_connect: bool,
    pub selected_vpn: VpnAdapterBinding,
    pub folder_shares: Vec<String>,
    pub enable_speaker_audio: bool,
    pub enable_mic_passthrough: bool,
}

impl Default for AddDesktopModalTab {
    fn default() -> Self {
        Self::General
    }
}

impl Default for VpnAdapterBinding {
    fn default() -> Self {
        Self::Direct
    }
}

/// Central State Engine for the rFerram Desktop Application
pub struct RFerramApp {
    core: Core,
    active_nav: NavItem,
    search_query: String,
    is_grid_view: bool,
    connection_cards: Vec<ProfileUiState>,
    add_modal_open: bool,
    add_modal_state: AddDesktopModalState,
    settings_drawer_open: bool,
    diagnostics_drawer_open: bool,
}

/// Messages Emitted across the Event Bus
#[derive(Debug, Clone)]
pub enum Message {
    NavSelected(NavItem),
    SearchChanged(String),
    ToggleViewMode, // Toggle Grid vs List
    OpenAddDesktopModal,
    CloseAddDesktopModal,
    SelectAddModalTab(AddDesktopModalTab),
    UpdateModalName(String),
    UpdateModalHost(String),
    UpdateModalUser(String),
    UpdateModalPassword(String),
    SelectModalVpn(VpnAdapterBinding),
    ToggleAutoReconnect(bool),
    ToggleAdminSession(bool),
    SaveConnectionProfile,
    ConnectSession(String),
    ConfigureConnection(String),
    ToggleFavorite(String),
    OpenSettingsDrawer,
    CloseSettingsDrawer,
    OpenDiagnosticsDrawer,
    CloseDiagnosticsDrawer,
    WireMessageReceived { profile_id: String, msg: WireMessage },
    SessionStatusChanged { profile_id: String, status: String },
}

impl RFerramApp {
    pub fn new(core: Core) -> (Self, Command<Message>) {
        let app = Self {
            core,
            active_nav: NavItem::AllDesktops,
            search_query: String::new(),
            is_grid_view: true,
            connection_cards: vec![
                ProfileUiState {
                    profile: ConnectionProfile {
                        id: String::from("prof-1"),
                        friendly_name: String::from("macOS Workstation M3"),
                        host_address: String::from("10.0.10.15:4433"),
                        group_name: String::from("Production"),
                    },
                    is_favorite: true,
                    vpn_binding: VpnAdapterBinding::WireGuard(String::from("utun3")),
                    active_fps: Some(60.0),
                    active_rtt_ms: Some(12.4),
                },
                ProfileUiState {
                    profile: ConnectionProfile {
                        id: String::from("prof-2"),
                        friendly_name: String::from("Wayland Arch Build Box"),
                        host_address: String::from("192.168.1.100:4433"),
                        group_name: String::from("Development"),
                    },
                    is_favorite: false,
                    vpn_binding: VpnAdapterBinding::Tailscale,
                    active_fps: None,
                    active_rtt_ms: None,
                },
            ],
            add_modal_open: false,
            add_modal_state: AddDesktopModalState::default(),
            settings_drawer_open: false,
            diagnostics_drawer_open: false,
        };

        (app, Command::none())
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::NavSelected(item) => {
                self.active_nav = item;
            }
            Message::SearchChanged(q) => {
                self.search_query = q;
            }
            Message::ToggleViewMode => {
                self.is_grid_view = !self.is_grid_view;
            }
            Message::OpenAddDesktopModal => {
                self.add_modal_open = true;
                self.add_modal_state = AddDesktopModalState::default();
            }
            Message::CloseAddDesktopModal => {
                self.add_modal_open = false;
            }
            Message::SelectAddModalTab(tab) => {
                self.add_modal_state.active_tab = tab;
            }
            Message::UpdateModalName(val) => {
                self.add_modal_state.name_input = val;
            }
            Message::UpdateModalHost(val) => {
                self.add_modal_state.host_input = val;
            }
            Message::SelectModalVpn(vpn) => {
                self.add_modal_state.selected_vpn = vpn;
            }
            Message::SaveConnectionProfile => {
                let new_profile = ProfileUiState {
                    profile: ConnectionProfile {
                        id: uuid::Uuid::new_v4().to_string(),
                        friendly_name: self.add_modal_state.name_input.clone(),
                        host_address: self.add_modal_state.host_input.clone(),
                        group_name: self.add_modal_state.group_input.clone(),
                    },
                    is_favorite: false,
                    vpn_binding: self.add_modal_state.selected_vpn.clone(),
                    active_fps: None,
                    active_rtt_ms: None,
                };
                self.connection_cards.push(new_profile);
                self.add_modal_open = false;
            }
            Message::ConnectSession(profile_id) => {
                tracing::info!(profile_id = %profile_id, "Connecting to target host over QUIC transport");
            }
            Message::OpenSettingsDrawer => {
                self.settings_drawer_open = true;
            }
            Message::CloseSettingsDrawer => {
                self.settings_drawer_open = false;
            }
            _ => {}
        }
        Command::none()
    }
}
