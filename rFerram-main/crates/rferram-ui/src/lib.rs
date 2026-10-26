//! `rferram-ui`: Graphical Client View & Message Loop (v2 - Native COSMIC Desktop & Cross-Platform)
//!
//! Implements the primary `libcosmic` / `iced` Application state engine (`cosmic::Application`),
//! desktop connection card grids, "Add Desktop" setup modal (matching the visual layout mockup with
//! global Connection/Credentials inputs and General, Display, Devices | Audio, and Folders tabs),
//! settings/help context drawers, frosted glass WGSL shader, and async transport message loops.

use cosmic::app::{Command, Core, Task};
use cosmic::widget::{
    button, column, container, divider, dropdown, grid, icon, row, scrollable, text, text_input,
    toggler,
};
use cosmic::{Application, Element};

use rferram_common::{ConnectionProfile, RuntimeEnvironment};
use rferram_protocol::WireMessage;

/// Native COSMIC Desktop Frosted Glass WGSL Fragment Shader
pub const FROSTED_GLASS_SHADER_WGSL: &str = r#"
struct FrostedGlassUniforms {
    tint_color: vec4<f32>,
    blur_radius: f32,
    opacity: f32,
    corner_radius: f32,
    _pad: f32,
};

@group(0) @binding(0)
var<uniform> glass: FrostedGlassUniforms;

@fragment
fn fs_frosted_glass(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    let alpha = clamp(glass.opacity, 0.0, 1.0);
    return vec4<f32>(glass.tint_color.rgb, alpha);
}
"#;

/// Main Navigation Sidebar Item
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavItem {
    AllDesktops,
    Favorites,
    Group(u32),
    GlobalSettings,
    Diagnostics,
}

/// Settings Drawer Section (matching Mockup top-right Settings menu)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsSection {
    #[default]
    General,
    Keyboard,
    ResolutionScaling,
    Groups,
    Profiles,
    Vpn,
}

/// Help / Diagnostics Drawer Section (matching Mockup bottom-right Help menu)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HelpSection {
    #[default]
    Documentation,
    Troubleshooting,
    ReportBug,
    FeatureRequest,
}

/// Modal Tab Options inside the "Add Desktop" Modal Dialog
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AddDesktopModalTab {
    #[default]
    General,
    Display,
    DevicesAudio,
    Folders,
    Credentials,
    VpnSelect,
}

/// VPN Adapter Binding Option (Crucial for "Bring Your Own Network" Model)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum VpnAdapterBinding {
    #[default]
    Direct,
    WireGuard(String), // e.g., wg0 on Linux or utun3 on macOS
    Tailscale,
    UniFiVpn,
    CustomAdapter(String),
}

impl VpnAdapterBinding {
    pub fn to_interface_option(&self) -> Option<String> {
        match self {
            Self::Direct => None,
            Self::WireGuard(if_name) => Some(if_name.clone()),
            Self::Tailscale => Some(String::from("tailscale0")),
            Self::UniFiVpn => Some(String::from("unifi0")),
            Self::CustomAdapter(if_name) => Some(if_name.clone()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Direct => "None",
            Self::WireGuard(_) => "WireGuard",
            Self::Tailscale => "Tailscale",
            Self::UniFiVpn => "UniFi VPN",
            Self::CustomAdapter(name) => name.as_str(),
        }
    }
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
#[derive(Debug, Clone)]
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
    pub target_monitor_index: usize,
    pub scale_factor_percent: u32,
    pub folder_shares: Vec<String>,
    pub enable_speaker_audio: bool,
    pub enable_mic_passthrough: bool,
}

impl Default for AddDesktopModalState {
    fn default() -> Self {
        Self {
            active_tab: AddDesktopModalTab::General,
            name_input: String::new(),
            host_input: String::new(),
            group_input: String::from("Group 1"),
            auto_reconnect: true,
            admin_session_takeover: false,
            username_input: String::new(),
            password_input: String::new(),
            ask_credentials_on_connect: true,
            selected_vpn: VpnAdapterBinding::Direct,
            target_monitor_index: 0,
            scale_factor_percent: 100,
            folder_shares: Vec::new(),
            enable_speaker_audio: true,
            enable_mic_passthrough: false,
        }
    }
}

/// Central State Engine for the rFerram Desktop Application
pub struct RFerramApp {
    pub core: Core,
    pub runtime_env: RuntimeEnvironment,
    pub active_nav: NavItem,
    pub search_query: String,
    pub is_grid_view: bool,
    pub connection_cards: Vec<ProfileUiState>,
    pub add_modal_open: bool,
    pub add_modal_state: AddDesktopModalState,
    pub settings_drawer_open: bool,
    pub active_settings_section: SettingsSection,
    pub diagnostics_drawer_open: bool,
    pub active_help_section: HelpSection,
}

/// Messages Emitted across the Event Bus
#[derive(Debug, Clone)]
pub enum Message {
    NavSelected(NavItem),
    SearchChanged(String),
    ToggleViewMode,
    OpenAddDesktopModal,
    CloseAddDesktopModal,
    SelectAddModalTab(AddDesktopModalTab),
    UpdateModalName(String),
    UpdateModalHost(String),
    UpdateModalGroup(String),
    UpdateModalUser(String),
    UpdateModalPassword(String),
    SelectModalVpn(VpnAdapterBinding),
    ToggleAutoReconnect(bool),
    ToggleAdminSession(bool),
    SaveConnectionProfile,
    ConnectSession(String),
    ConfigureConnection(String),
    DuplicateConnection(String),
    ExportConnection(String),
    DeleteConnection(String),
    ToggleFavorite(String),
    OpenSettingsDrawer,
    CloseSettingsDrawer,
    SelectSettingsSection(SettingsSection),
    OpenDiagnosticsDrawer,
    CloseDiagnosticsDrawer,
    SelectHelpSection(HelpSection),
    WireMessageReceived {
        profile_id: String,
        msg: WireMessage,
    },
    SessionStatusChanged {
        profile_id: String,
        status: String,
    },
}

impl Application for RFerramApp {
    const APP_ID: &'static str = "com.theinfomoose.rFerram";
    type Executor = ();
    type Flags = ();
    type Message = Message;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        Self::new(core)
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        RFerramApp::update(self, message)
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let mut root_children = vec![
            self.view_header(),
            row(
                16,
                vec![self.view_sidebar(), scrollable(self.view_connection_grid())],
            ),
        ];

        if self.add_modal_open {
            root_children.push(self.view_add_desktop_modal());
        }
        if self.settings_drawer_open {
            root_children.push(self.view_settings_drawer());
        }
        if self.diagnostics_drawer_open {
            root_children.push(self.view_help_drawer());
        }

        container("rferram-root-shell", column(12, root_children))
    }
}

impl RFerramApp {
    pub fn new(core: Core) -> (Self, Command<Message>) {
        let runtime_env = RuntimeEnvironment::detect();
        let app = Self {
            core,
            runtime_env,
            active_nav: NavItem::AllDesktops,
            search_query: String::new(),
            is_grid_view: true,
            connection_cards: vec![
                ProfileUiState {
                    profile: ConnectionProfile {
                        id: String::from("prof-1"),
                        friendly_name: String::from("macOS Workstation M3"),
                        host_address: String::from("10.0.10.15:4433"),
                        group_name: String::from("Group 1"),
                        vpn_binding: Some(String::from("utun3")),
                        ..Default::default()
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
                        group_name: String::from("Group 1"),
                        vpn_binding: Some(String::from("tailscale0")),
                        ..Default::default()
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
            active_settings_section: SettingsSection::General,
            diagnostics_drawer_open: false,
            active_help_section: HelpSection::Documentation,
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
            Message::UpdateModalGroup(val) => {
                self.add_modal_state.group_input = val;
            }
            Message::UpdateModalUser(val) => {
                self.add_modal_state.username_input = val;
            }
            Message::UpdateModalPassword(val) => {
                self.add_modal_state.password_input = val;
            }
            Message::SelectModalVpn(vpn) => {
                self.add_modal_state.selected_vpn = vpn;
            }
            Message::ToggleAutoReconnect(val) => {
                self.add_modal_state.auto_reconnect = val;
            }
            Message::ToggleAdminSession(val) => {
                self.add_modal_state.admin_session_takeover = val;
            }
            Message::SaveConnectionProfile => {
                let friendly = if self.add_modal_state.name_input.trim().is_empty() {
                    self.add_modal_state.host_input.clone()
                } else {
                    self.add_modal_state.name_input.clone()
                };
                let group = if self.add_modal_state.group_input.trim().is_empty() {
                    String::from("Group 1")
                } else {
                    self.add_modal_state.group_input.clone()
                };
                let vpn_binding = self.add_modal_state.selected_vpn.to_interface_option();

                let new_profile = ProfileUiState {
                    profile: ConnectionProfile {
                        id: uuid::Uuid::new_v4().to_string(),
                        friendly_name: friendly,
                        host_address: self.add_modal_state.host_input.clone(),
                        group_name: group,
                        vpn_binding,
                        auto_reconnect: self.add_modal_state.auto_reconnect,
                        admin_session_takeover: self.add_modal_state.admin_session_takeover,
                        ask_credentials_on_connect: self.add_modal_state.ask_credentials_on_connect,
                        folder_shares: self.add_modal_state.folder_shares.clone(),
                        enable_speaker_audio: self.add_modal_state.enable_speaker_audio,
                        enable_mic_passthrough: self.add_modal_state.enable_mic_passthrough,
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
                tracing::info!(
                    profile_id = %profile_id,
                    "Connecting to target host over QUIC transport"
                );
            }
            Message::DeleteConnection(profile_id) => {
                self.connection_cards.retain(|c| c.profile.id != profile_id);
            }
            Message::ToggleFavorite(profile_id) => {
                if let Some(card) = self
                    .connection_cards
                    .iter_mut()
                    .find(|c| c.profile.id == profile_id)
                {
                    card.is_favorite = !card.is_favorite;
                }
            }
            Message::OpenSettingsDrawer => {
                self.settings_drawer_open = true;
            }
            Message::CloseSettingsDrawer => {
                self.settings_drawer_open = false;
            }
            Message::SelectSettingsSection(sec) => {
                self.active_settings_section = sec;
            }
            Message::OpenDiagnosticsDrawer => {
                self.diagnostics_drawer_open = true;
            }
            Message::CloseDiagnosticsDrawer => {
                self.diagnostics_drawer_open = false;
            }
            Message::SelectHelpSection(sec) => {
                self.active_help_section = sec;
            }
            _ => {}
        }
        Command::none()
    }

    /// Top Header Bar (`Connections`, `+` Add/Import/Export, Grid/List toggle, Search, Settings gear, Help `?`)
    pub fn view_header(&self) -> Element<'_, Message> {
        container(
            "rferram-header",
            row(
                12,
                vec![
                    text("Connections").size(18).into(),
                    button("+").on_press(Message::OpenAddDesktopModal).into(),
                    button(if self.is_grid_view { "Grid" } else { "List" })
                        .on_press(Message::ToggleViewMode)
                        .into(),
                    text_input("Search connections...", &self.search_query).into(),
                    button("Settings")
                        .on_press(Message::OpenSettingsDrawer)
                        .into(),
                    button("?").on_press(Message::OpenDiagnosticsDrawer).into(),
                ],
            ),
        )
    }

    /// Left Sidebar Navigation (`Favorites`, `Groups`)
    pub fn view_sidebar(&self) -> Element<'_, Message> {
        container(
            "rferram-sidebar",
            column(
                8,
                vec![
                    button("Favorites")
                        .on_press(Message::NavSelected(NavItem::Favorites))
                        .into(),
                    divider(),
                    button("Groups")
                        .on_press(Message::NavSelected(NavItem::AllDesktops))
                        .into(),
                ],
            ),
        )
    }

    /// Main Connection Card Grid grouped by Group Name
    pub fn view_connection_grid(&self) -> Element<'_, Message> {
        let query = self.search_query.to_ascii_lowercase();
        let cards: Vec<Element<'_, Message>> = self
            .connection_cards
            .iter()
            .filter(|c| {
                if self.active_nav == NavItem::Favorites && !c.is_favorite {
                    return false;
                }
                if query.is_empty() {
                    return true;
                }
                c.profile
                    .friendly_name
                    .to_ascii_lowercase()
                    .contains(&query)
                    || c.profile.host_address.to_ascii_lowercase().contains(&query)
                    || c.profile.group_name.to_ascii_lowercase().contains(&query)
            })
            .map(|c| {
                let pid = c.profile.id.clone();
                container(
                    "rferram-connection-card",
                    column(
                        6,
                        vec![
                            row(
                                4,
                                vec![
                                    button("...")
                                        .on_press(Message::ConfigureConnection(pid.clone()))
                                        .into(),
                                    button(if c.is_favorite { "*" } else { "o" })
                                        .on_press(Message::ToggleFavorite(pid.clone()))
                                        .into(),
                                ],
                            ),
                            text(&c.profile.friendly_name).size(15).into(),
                            text(&c.profile.host_address).size(12).into(),
                            text(format!("VPN: {}", c.vpn_binding.label()))
                                .size(11)
                                .into(),
                            button("Connect")
                                .on_press(Message::ConnectSession(pid))
                                .into(),
                        ],
                    ),
                )
            })
            .collect();

        container(
            "rferram-group-section",
            column(
                10,
                vec![
                    text("Group 1").size(16).into(),
                    divider(),
                    grid(if self.is_grid_view { 3 } else { 1 }, 16, cards),
                ],
            ),
        )
    }

    /// "Add Desktop" Modal Dialog matching the visual wireframe specification
    pub fn view_add_desktop_modal(&self) -> Element<'_, Message> {
        let tab_bar_row = row(
            8,
            vec![
                button("General")
                    .on_press(Message::SelectAddModalTab(AddDesktopModalTab::General))
                    .into(),
                button("Display")
                    .on_press(Message::SelectAddModalTab(AddDesktopModalTab::Display))
                    .into(),
                button("Devices | Audio")
                    .on_press(Message::SelectAddModalTab(AddDesktopModalTab::DevicesAudio))
                    .into(),
                button("Folders")
                    .on_press(Message::SelectAddModalTab(AddDesktopModalTab::Folders))
                    .into(),
            ],
        );

        let tab_body = match self.add_modal_state.active_tab {
            AddDesktopModalTab::General
            | AddDesktopModalTab::Credentials
            | AddDesktopModalTab::VpnSelect => column(
                8,
                vec![
                    text("Friendly Name:").into(),
                    text_input("Optional local only name", &self.add_modal_state.name_input).into(),
                    text("Group:").into(),
                    dropdown(vec![String::from("Group 1")], Some(0)),
                    divider(),
                    text("VPN:").into(),
                    dropdown(
                        vec![
                            String::from("None"),
                            String::from("WireGuard"),
                            String::from("Tailscale"),
                            String::from("UniFi VPN"),
                        ],
                        Some(0),
                    ),
                    divider(),
                    toggler(
                        "Reconnect if the session is dropped",
                        self.add_modal_state.auto_reconnect,
                        Message::ToggleAutoReconnect(!self.add_modal_state.auto_reconnect),
                    ),
                    toggler(
                        "Connect to an admin session",
                        self.add_modal_state.admin_session_takeover,
                        Message::ToggleAdminSession(!self.add_modal_state.admin_session_takeover),
                    ),
                ],
            ),
            AddDesktopModalTab::Display => column(
                8,
                vec![
                    text("Resolution | Scaling").into(),
                    dropdown(
                        vec![
                            String::from("Native (100%)"),
                            String::from("HiDPI (150%)"),
                            String::from("UltraHD (200%)"),
                        ],
                        Some(0),
                    ),
                ],
            ),
            AddDesktopModalTab::DevicesAudio => column(
                8,
                vec![
                    toggler(
                        "Stream Host Speaker Audio (PipeWire / WASAPI / CoreAudio)",
                        self.add_modal_state.enable_speaker_audio,
                        Message::SelectAddModalTab(AddDesktopModalTab::DevicesAudio),
                    ),
                    toggler(
                        "Enable Microphone Passthrough",
                        self.add_modal_state.enable_mic_passthrough,
                        Message::SelectAddModalTab(AddDesktopModalTab::DevicesAudio),
                    ),
                ],
            ),
            AddDesktopModalTab::Folders => column(
                8,
                vec![
                    text("Shared Folders (ST_Rust_Age Snapshot Engine)").into(),
                    icon("folder-remote-symbolic", 24),
                ],
            ),
        };

        container(
            "rferram-add-desktop-modal",
            column(
                10,
                vec![
                    text("Add Desktop").size(18).into(),
                    text("Connection:").into(),
                    text_input("Host name or IP address", &self.add_modal_state.host_input).into(),
                    text("Credentials:").into(),
                    dropdown(vec![String::from("Ask when required")], Some(0)),
                    tab_bar_row,
                    tab_body,
                    row(
                        8,
                        vec![
                            button("Cancel")
                                .on_press(Message::CloseAddDesktopModal)
                                .into(),
                            button("Add")
                                .on_press(Message::SaveConnectionProfile)
                                .into(),
                        ],
                    ),
                ],
            ),
        )
    }

    /// Settings Context Drawer (`General`, `Keyboard`, `Resolution | Scaling`, `Groups`, `Profiles`, `VPN`)
    pub fn view_settings_drawer(&self) -> Element<'_, Message> {
        container(
            "rferram-settings-drawer",
            column(
                8,
                vec![
                    button("General")
                        .on_press(Message::SelectSettingsSection(SettingsSection::General))
                        .into(),
                    button("Keyboard")
                        .on_press(Message::SelectSettingsSection(SettingsSection::Keyboard))
                        .into(),
                    button("Resolution | Scaling")
                        .on_press(Message::SelectSettingsSection(
                            SettingsSection::ResolutionScaling,
                        ))
                        .into(),
                    button("Groups")
                        .on_press(Message::SelectSettingsSection(SettingsSection::Groups))
                        .into(),
                    button("Profiles")
                        .on_press(Message::SelectSettingsSection(SettingsSection::Profiles))
                        .into(),
                    button("VPN")
                        .on_press(Message::SelectSettingsSection(SettingsSection::Vpn))
                        .into(),
                    button("Close")
                        .on_press(Message::CloseSettingsDrawer)
                        .into(),
                ],
            ),
        )
    }

    /// Help / Diagnostics Drawer (`Documentation`, `Troubleshooting`, `Report Bug`, `Feature Request`)
    pub fn view_help_drawer(&self) -> Element<'_, Message> {
        container(
            "rferram-help-drawer",
            column(
                8,
                vec![
                    button("Documentation")
                        .on_press(Message::SelectHelpSection(HelpSection::Documentation))
                        .into(),
                    button("Troubleshooting")
                        .on_press(Message::SelectHelpSection(HelpSection::Troubleshooting))
                        .into(),
                    button("Report Bug")
                        .on_press(Message::SelectHelpSection(HelpSection::ReportBug))
                        .into(),
                    button("Feature Request")
                        .on_press(Message::SelectHelpSection(HelpSection::FeatureRequest))
                        .into(),
                    button("Close")
                        .on_press(Message::CloseDiagnosticsDrawer)
                        .into(),
                ],
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_modal_workflow_and_mockup_elements() {
        let (mut app, _) = RFerramApp::new(Core::new());
        assert_eq!(app.connection_cards.len(), 2);

        // Open Add Desktop modal
        let _ = app.update(Message::OpenAddDesktopModal);
        assert!(app.add_modal_open);

        // Verify mockup labels are rendered in the view tree
        let view_labels = Application::view(&app).collect_text();
        assert!(view_labels.iter().any(|t| t == "Add Desktop"));
        assert!(view_labels.iter().any(|t| t == "Host name or IP address"));
        assert!(view_labels.iter().any(|t| t == "Ask when required"));
        assert!(
            view_labels
                .iter()
                .any(|t| t == "Reconnect if the session is dropped")
        );
        assert!(
            view_labels
                .iter()
                .any(|t| t == "Connect to an admin session")
        );

        // Populate modal fields and save new connection profile
        let _ = app.update(Message::UpdateModalName("SUSE Enterprise Host".to_string()));
        let _ = app.update(Message::UpdateModalHost("10.20.30.40:4433".to_string()));
        let _ = app.update(Message::SelectModalVpn(VpnAdapterBinding::WireGuard(
            "wg0".to_string(),
        )));
        let _ = app.update(Message::SaveConnectionProfile);

        assert!(!app.add_modal_open);
        assert_eq!(app.connection_cards.len(), 3);
        assert_eq!(
            app.connection_cards[2].profile.friendly_name,
            "SUSE Enterprise Host"
        );
        assert_eq!(
            app.connection_cards[2].profile.vpn_binding.as_deref(),
            Some("wg0")
        );
    }
}
