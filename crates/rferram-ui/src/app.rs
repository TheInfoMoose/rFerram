//! Main `libcosmic` / `iced` Application Shell, Header, Sidebar & Connection Card Grid
//!
//! Implements `RFerramApp`, `NavItem`, `ProfileUiState`, `Message`, the `cosmic::Application`
//! trait implementation, and the primary shell views (`view_header`, `view_sidebar`, and
//! `view_connection_grid`).

use cosmic::app::{Command, Core, Task};
use cosmic::widget::{button, column, container, divider, grid, row, scrollable, text, text_input};
use cosmic::{Application, Element};

use rferram_common::{ConnectionProfile, RuntimeEnvironment};
use rferram_protocol::WireMessage;

use crate::{
    AddDesktopModalState, AddDesktopModalTab, HelpSection, SettingsSection, VpnAdapterBinding,
};

/// Main Navigation Sidebar Item
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavItem {
    AllDesktops,
    Favorites,
    Group(u32),
    GlobalSettings,
    Diagnostics,
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

impl ProfileUiState {
    /// Constructs a `ProfileUiState` from a persisted `ConnectionProfile`.
    pub fn from_profile(profile: ConnectionProfile, is_favorite: bool) -> Self {
        let vpn_binding = VpnAdapterBinding::from_interface_option(profile.vpn_binding.as_deref());
        Self {
            profile,
            is_favorite,
            vpn_binding,
            active_fps: None,
            active_rtt_ms: None,
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

    /// Saves all active `ConnectionProfile` entries and favorite profile IDs in `connection_cards` to `cosmic-config`.
    pub fn save_profiles_to_config(
        &self,
        config: &cosmic_config::Config,
    ) -> Result<(), cosmic_config::ConfigError> {
        let profiles: Vec<ConnectionProfile> = self
            .connection_cards
            .iter()
            .map(|c| c.profile.clone())
            .collect();
        let favorite_ids: Vec<String> = self
            .connection_cards
            .iter()
            .filter(|c| c.is_favorite)
            .map(|c| c.profile.id.clone())
            .collect();
        crate::save_connection_profiles(config, &profiles)?;
        config.set(crate::FAVORITE_PROFILES_CONFIG_KEY, &favorite_ids)
    }

    /// Loads `ConnectionProfile` entries (and favorite profile IDs when present) from `cosmic-config` and updates `connection_cards`.
    pub fn load_profiles_from_config(
        &mut self,
        config: &cosmic_config::Config,
    ) -> Result<Vec<ConnectionProfile>, cosmic_config::ConfigError> {
        let profiles = crate::load_connection_profiles(config)?;
        let favorite_ids: Vec<String> = config
            .get(crate::FAVORITE_PROFILES_CONFIG_KEY)
            .unwrap_or_else(|_| {
                self.connection_cards
                    .iter()
                    .filter(|c| c.is_favorite)
                    .map(|c| c.profile.id.clone())
                    .collect()
            });
        self.connection_cards = profiles
            .iter()
            .cloned()
            .map(|p| {
                let is_fav = favorite_ids.iter().any(|id| id == &p.id);
                ProfileUiState::from_profile(p, is_fav)
            })
            .collect();
        Ok(profiles)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_filtering() {
        let (mut app, _) = RFerramApp::new(Core::new());

        // Initially both profiles are visible
        let initial_labels = app.view_connection_grid().collect_text();
        assert!(initial_labels.iter().any(|t| t == "macOS Workstation M3"));
        assert!(initial_labels.iter().any(|t| t == "Wayland Arch Build Box"));

        // Filter by friendly name (case-insensitive)
        let _ = app.update(Message::SearchChanged("macos".to_string()));
        let filtered_by_name = app.view_connection_grid().collect_text();
        assert!(filtered_by_name.iter().any(|t| t == "macOS Workstation M3"));
        assert!(
            !filtered_by_name
                .iter()
                .any(|t| t == "Wayland Arch Build Box")
        );

        // Filter by host address
        let _ = app.update(Message::SearchChanged("192.168.1.100".to_string()));
        let filtered_by_host = app.view_connection_grid().collect_text();
        assert!(!filtered_by_host.iter().any(|t| t == "macOS Workstation M3"));
        assert!(
            filtered_by_host
                .iter()
                .any(|t| t == "Wayland Arch Build Box")
        );

        // Filter by group name
        let _ = app.update(Message::SearchChanged("group 1".to_string()));
        let filtered_by_group = app.view_connection_grid().collect_text();
        assert!(
            filtered_by_group
                .iter()
                .any(|t| t == "macOS Workstation M3")
        );
        assert!(
            filtered_by_group
                .iter()
                .any(|t| t == "Wayland Arch Build Box")
        );

        // Filter with non-matching query
        let _ = app.update(Message::SearchChanged("nonexistent-host".to_string()));
        let empty_filter = app.view_connection_grid().collect_text();
        assert!(!empty_filter.iter().any(|t| t == "macOS Workstation M3"));
        assert!(!empty_filter.iter().any(|t| t == "Wayland Arch Build Box"));
    }

    #[test]
    fn test_favorites_view_filtering() {
        let (mut app, _) = RFerramApp::new(Core::new());
        assert_eq!(app.active_nav, NavItem::AllDesktops);

        // Switch to Favorites view: only prof-1 ("macOS Workstation M3") is favorite by default
        let _ = app.update(Message::NavSelected(NavItem::Favorites));
        assert_eq!(app.active_nav, NavItem::Favorites);
        let fav_labels = app.view_connection_grid().collect_text();
        assert!(fav_labels.iter().any(|t| t == "macOS Workstation M3"));
        assert!(!fav_labels.iter().any(|t| t == "Wayland Arch Build Box"));

        // Mark prof-2 as favorite and verify both appear in Favorites view
        let _ = app.update(Message::ToggleFavorite("prof-2".to_string()));
        let updated_fav_labels = app.view_connection_grid().collect_text();
        assert!(
            updated_fav_labels
                .iter()
                .any(|t| t == "macOS Workstation M3")
        );
        assert!(
            updated_fav_labels
                .iter()
                .any(|t| t == "Wayland Arch Build Box")
        );

        // Unfavorite prof-1 and verify only prof-2 remains in Favorites view
        let _ = app.update(Message::ToggleFavorite("prof-1".to_string()));
        let single_fav_labels = app.view_connection_grid().collect_text();
        assert!(
            !single_fav_labels
                .iter()
                .any(|t| t == "macOS Workstation M3")
        );
        assert!(
            single_fav_labels
                .iter()
                .any(|t| t == "Wayland Arch Build Box")
        );

        // Switch back to AllDesktops: both profiles are visible again
        let _ = app.update(Message::NavSelected(NavItem::AllDesktops));
        let all_labels = app.view_connection_grid().collect_text();
        assert!(all_labels.iter().any(|t| t == "macOS Workstation M3"));
        assert!(all_labels.iter().any(|t| t == "Wayland Arch Build Box"));
    }

    #[test]
    fn test_grid_list_mode_toggling() {
        let (mut app, _) = RFerramApp::new(Core::new());
        assert!(app.is_grid_view);

        let header_grid = app.view_header().collect_text();
        assert!(header_grid.iter().any(|t| t == "Grid"));
        assert!(!header_grid.iter().any(|t| t == "List"));

        // Toggle to List mode
        let _ = app.update(Message::ToggleViewMode);
        assert!(!app.is_grid_view);

        let header_list = app.view_header().collect_text();
        assert!(header_list.iter().any(|t| t == "List"));
        assert!(!header_list.iter().any(|t| t == "Grid"));

        // Toggle back to Grid mode
        let _ = app.update(Message::ToggleViewMode);
        assert!(app.is_grid_view);

        let header_grid_restored = app.view_header().collect_text();
        assert!(header_grid_restored.iter().any(|t| t == "Grid"));
    }
}
