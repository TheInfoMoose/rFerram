//! Main `libcosmic` / `iced` Application Shell, Header, Sidebar & Connection Card Grid
//!
//! Implements `RFerramApp`, `NavItem`, `ProfileUiState`, `Message`, the `cosmic::Application`
//! trait implementation, and the primary shell views (`view_header`, `view_sidebar`, and
//! `view_connection_grid`).

use std::collections::HashMap;

use cosmic::app::{Command, Core, Task};
use cosmic::widget::{button, column, container, divider, grid, row, scrollable, text, text_input};
use cosmic::{Application, Element};

use rferram_common::{ConnectionProfile, RuntimeEnvironment};
use rferram_protocol::WireMessage;

use crate::{
    AddDesktopModalState, AddDesktopModalTab, HelpSection, SessionConnectOutcome, SettingsSection,
    UiSessionController, VpnAdapterBinding,
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
    pub persistence_config: Option<cosmic_config::Config>,
    pub session_controller: UiSessionController,
    pub session_statuses: HashMap<String, String>,
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
            persistence_config: None,
            session_controller: UiSessionController::new(),
            session_statuses: HashMap::new(),
        };

        (app, Command::none())
    }

    /// Returns the most recent session status string recorded for `profile_id`.
    pub fn session_status(&self, profile_id: &str) -> Option<&str> {
        self.session_statuses.get(profile_id).map(String::as_str)
    }

    /// Attaches a `cosmic-config` store for automatic profile persistence and loads any
    /// previously saved profiles from that store if present.
    pub fn with_persistence_config(mut self, config: cosmic_config::Config) -> Self {
        let _ = self.load_profiles_from_config(&config);
        self.persistence_config = Some(config);
        self
    }

    /// Sets or clears the optional `cosmic-config` store used to auto-persist profile mutations.
    pub fn set_persistence_config(&mut self, config: Option<cosmic_config::Config>) {
        self.persistence_config = config;
    }

    fn auto_persist_profiles(&self) {
        if let Some(ref config) = self.persistence_config
            && let Err(err) = self.save_profiles_to_config(config)
        {
            tracing::warn!(
                error = %err,
                "Failed to auto-persist connection profiles to cosmic-config"
            );
        }
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
                self.auto_persist_profiles();
            }
            Message::ConnectSession(profile_id) => {
                tracing::info!(
                    profile_id = %profile_id,
                    "Connecting to target host over QUIC transport"
                );
                self.session_statuses
                    .insert(profile_id.clone(), String::from("Connecting"));
                if let Some(card) = self
                    .connection_cards
                    .iter()
                    .find(|c| c.profile.id == profile_id)
                {
                    let profile = card.profile.clone();
                    let vpn_binding = card.vpn_binding.clone();
                    return self
                        .session_controller
                        .spawn_connect_task(profile, vpn_binding);
                }
                let controller = self.session_controller.clone();
                return Task::perform(
                    async move {
                        let status = String::from("Failed: Profile not found");
                        controller.record_failed_status(&profile_id, &status);
                        SessionConnectOutcome::failed(profile_id, status)
                    },
                    SessionConnectOutcome::into_message,
                );
            }
            Message::DuplicateConnection(profile_id) => {
                if let Some(existing) = self
                    .connection_cards
                    .iter()
                    .find(|c| c.profile.id == profile_id)
                    .cloned()
                {
                    let mut duplicated = existing;
                    duplicated.profile.id = uuid::Uuid::new_v4().to_string();
                    duplicated.profile.friendly_name =
                        format!("{} (Copy)", duplicated.profile.friendly_name);
                    self.connection_cards.push(duplicated);
                    self.auto_persist_profiles();
                }
            }
            Message::DeleteConnection(profile_id) => {
                self.session_controller.disconnect_profile(&profile_id);
                self.session_statuses.remove(&profile_id);
                self.connection_cards.retain(|c| c.profile.id != profile_id);
                self.auto_persist_profiles();
            }
            Message::ToggleFavorite(profile_id) => {
                if let Some(card) = self
                    .connection_cards
                    .iter_mut()
                    .find(|c| c.profile.id == profile_id)
                {
                    card.is_favorite = !card.is_favorite;
                    self.auto_persist_profiles();
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
            Message::WireMessageReceived { profile_id, msg } => {
                let snapshot = self
                    .session_controller
                    .handle_wire_message(&profile_id, msg);
                if let Some(ref snap) = snapshot {
                    self.session_statuses
                        .insert(profile_id.clone(), snap.status.clone());
                }
                if let Some(card) = self
                    .connection_cards
                    .iter_mut()
                    .find(|c| c.profile.id == profile_id)
                    && let Some(snap) = snapshot
                {
                    if snap.active_fps.is_some() {
                        card.active_fps = snap.active_fps;
                    }
                    if snap.active_rtt_ms.is_some() {
                        card.active_rtt_ms = snap.active_rtt_ms;
                    }
                }
            }
            Message::SessionStatusChanged { profile_id, status } => {
                self.session_statuses
                    .insert(profile_id.clone(), status.clone());
                let snapshot = self.session_controller.refresh_telemetry(&profile_id);
                if let Some(card) = self
                    .connection_cards
                    .iter_mut()
                    .find(|c| c.profile.id == profile_id)
                {
                    if status.starts_with("Failed")
                        || status.starts_with("Disconnected")
                        || status.starts_with("Terminated")
                    {
                        card.active_fps = None;
                        card.active_rtt_ms = None;
                    } else if let Some(snap) = snapshot {
                        card.active_fps = snap.active_fps;
                        card.active_rtt_ms = snap.active_rtt_ms;
                    } else if status.starts_with("Active") || status.starts_with("Connected") {
                        if card.active_fps.is_none() {
                            card.active_fps = Some(crate::DEFAULT_TARGET_FPS);
                        }
                        if card.active_rtt_ms.is_none() {
                            card.active_rtt_ms = Some(1.0);
                        }
                    }
                }
            }
            Message::ConfigureConnection(_) | Message::ExportConnection(_) => {}
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
                    text_input("Search connections...", &self.search_query)
                        .on_input(Message::SearchChanged)
                        .into(),
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

    #[test]
    fn test_rferram_app_1440x900_rasterization_and_overlays() {
        let (mut app, _) = RFerramApp::new(Core::new());

        // 1. Render default RFerramApp shell at 1440x900 in Dark Theme
        let base_scene = Application::view(&app).layout_scene(1440.0, 900.0);
        assert_eq!(base_scene.width(), 1440);
        assert_eq!(base_scene.height(), 900);
        let dark_pixmap = base_scene.rasterize(&cosmic::Theme::dark());
        assert_eq!(dark_pixmap.width(), 1440);
        assert_eq!(dark_pixmap.height(), 900);
        assert_eq!(dark_pixmap.data().len(), 1440 * 900 * 4);

        // Verify cyan accent pixels (from cosmic::Theme::dark().accent_rgb) and text glyphs are painted
        let dark_palette = cosmic::ThemePalette::from_theme(&cosmic::Theme::dark());
        let has_accent_pixel = dark_pixmap.pixels().iter().any(|px| {
            px.red() == dark_palette.accent[0]
                && px.green() == dark_palette.accent[1]
                && px.blue() == dark_palette.accent[2]
        });
        assert!(
            has_accent_pixel,
            "expected cyan accent pixels in 1440x900 dark render"
        );

        let has_text_pixel = dark_pixmap.pixels().iter().any(|px| {
            px.red() == dark_palette.text_primary[0]
                && px.green() == dark_palette.text_primary[1]
                && px.blue() == dark_palette.text_primary[2]
        });
        assert!(
            has_text_pixel,
            "expected rendered ASCII text glyph pixels in 1440x900 render"
        );

        // 2. Render in Light Theme and verify distinct surface output
        let light_pixmap = base_scene.rasterize(&cosmic::Theme::light());
        assert_eq!(light_pixmap.width(), 1440);
        assert_eq!(light_pixmap.height(), 900);
        assert_ne!(dark_pixmap.data(), light_pixmap.data());

        // 3. Open Add Desktop modal, Settings drawer, and Help drawer and rasterize at 1440x900
        let _ = app.update(Message::OpenAddDesktopModal);
        let _ = app.update(Message::UpdateModalHost("10.42.0.10:4433".to_string()));
        let _ = app.update(Message::OpenSettingsDrawer);
        let _ = app.update(Message::OpenDiagnosticsDrawer);

        let overlay_view = Application::view(&app);
        let overlay_scene = overlay_view
            .layout_scene(1440.0, 900.0)
            .with_caret_position(Some(8));
        let overlay_pixmap = cosmic::rasterize_scene(&overlay_scene, &app.core.theme);
        assert_eq!(overlay_pixmap.width(), 1440);
        assert_eq!(overlay_pixmap.height(), 900);
        assert_ne!(
            overlay_pixmap.data(),
            dark_pixmap.data(),
            "frosted modal and drawer overlays must alter the 1440x900 framebuffer"
        );

        // Verify caret position on the modal's populated host input ("10.42.0.10:4433") alters rendered pixels
        let overlay_caret_0 = overlay_view
            .layout_scene(1440.0, 900.0)
            .with_caret_position(Some(0))
            .rasterize(&app.core.theme);
        assert_ne!(
            overlay_pixmap.data(),
            overlay_caret_0.data(),
            "moving caret position on focused modal host input must shift rendered caret bar"
        );

        // 4. Populate 24 connection profiles so the scrollable connection grid overflows 900px height
        let _ = app.update(Message::CloseAddDesktopModal);
        let _ = app.update(Message::CloseSettingsDrawer);
        let _ = app.update(Message::CloseDiagnosticsDrawer);
        for idx in 0..24 {
            app.connection_cards.push(ProfileUiState {
                profile: ConnectionProfile {
                    id: format!("prof-overflow-{idx}"),
                    friendly_name: format!("Overflow Node {idx}"),
                    host_address: format!("10.99.0.{idx}:4433"),
                    group_name: String::from("Group 1"),
                    vpn_binding: Some(String::from("wg0")),
                    ..Default::default()
                },
                is_favorite: idx % 2 == 0,
                vpn_binding: VpnAdapterBinding::WireGuard(String::from("wg0")),
                active_fps: Some(60.0),
                active_rtt_ms: Some(4.2),
            });
        }

        // Verify rasterize_application helper renders the overflowing scrollable grid cleanly at 1440x900
        let app_pixmap = cosmic::rasterize_application(&app, 1440, 900);
        assert_eq!(app_pixmap.width(), 1440);
        assert_eq!(app_pixmap.height(), 900);
        assert_ne!(app_pixmap.data(), dark_pixmap.data());
    }

    #[test]
    fn test_rferram_app_runner_interactive_workflow() {
        let mut runner =
            cosmic::CosmicAppRunner::<RFerramApp>::headless(cosmic::app::Settings::default(), ());
        assert_eq!(runner.viewport_size(), (1440, 900));
        assert_eq!(runner.app.connection_cards.len(), 2);

        // 1. Focus the background header search bar, then click "+" button to open the "Add Desktop" modal
        let scene = runner.layout_scene();
        let search_input = scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::TextInput { placeholder, .. }
                        if placeholder == "Search connections..."
                )
            })
            .expect("missing header search input");
        assert!(runner.handle_pointer_click(
            search_input.x + 16.0,
            search_input.y + search_input.height * 0.5,
        ));
        assert_eq!(runner.focused_widget_id, Some(search_input.id));

        let plus_btn = scene
            .boxes
            .iter()
            .find(|b| matches!(&b.node, cosmic::WidgetNode::Button { label, .. } if label == "+"))
            .expect("missing '+' button");
        assert!(runner.handle_pointer_click(
            plus_btn.x + plus_btn.width * 0.5,
            plus_btn.y + plus_btn.height * 0.5,
        ));
        assert!(runner.app.add_modal_open);
        assert_eq!(runner.focused_widget_id, None);

        // 2. Typing while modal is open (even before clicking a modal input) targets the modal's top-z host input, never the occluded background search bar
        assert!(runner.handle_text_input("10.77.0.9:4433"));
        assert_eq!(runner.app.search_query, "");
        assert_eq!(runner.app.add_modal_state.host_input, "10.77.0.9:4433");

        let modal_scene = runner.layout_scene();
        let name_input = modal_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::TextInput { placeholder, .. }
                        if placeholder == "Optional local only name"
                )
            })
            .expect("missing modal friendly name input");
        assert!(
            runner
                .handle_pointer_click(name_input.x + 16.0, name_input.y + name_input.height * 0.5,)
        );
        assert!(runner.handle_text_input("COSMIC Dev Host"));
        assert_eq!(runner.app.add_modal_state.name_input, "COSMIC Dev Host");

        // 3. Click the VPN dropdown in the modal to select WireGuard (index 1)
        let modal_scene = runner.layout_scene();
        let vpn_dropdown = modal_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Dropdown { options, .. }
                        if options.iter().any(|o| o == "WireGuard")
                )
            })
            .expect("missing VPN dropdown");
        assert!(runner.handle_pointer_click(
            vpn_dropdown.x + vpn_dropdown.width * 0.5,
            vpn_dropdown.y + vpn_dropdown.height * 0.5,
        ));
        assert_eq!(
            runner.app.add_modal_state.selected_vpn,
            VpnAdapterBinding::WireGuard("wg0".to_string())
        );

        // 4. Click "Add" button inside the modal to save the profile
        let modal_scene = runner.layout_scene();
        let add_btn = modal_scene
            .boxes
            .iter()
            .find(|b| matches!(&b.node, cosmic::WidgetNode::Button { label, .. } if label == "Add"))
            .expect("missing modal Add button");
        assert!(runner.handle_pointer_click(
            add_btn.x + add_btn.width * 0.5,
            add_btn.y + add_btn.height * 0.5,
        ));
        assert!(!runner.app.add_modal_open);
        assert_eq!(runner.app.connection_cards.len(), 3);
        assert_eq!(
            runner.app.connection_cards[2].profile.friendly_name,
            "COSMIC Dev Host"
        );

        // 5. Resize and verify 256-byte row-aligned RGBA8 frame packing
        assert!(runner.handle_resize(1280, 720));
        let (aligned_buf, w, h, stride) = runner.pack_aligned_frame();
        assert_eq!((w, h), (1280, 720));
        assert_eq!(stride % cosmic::WGPU_ROW_ALIGNMENT, 0);
        assert_eq!(aligned_buf.len(), (stride * 720) as usize);
    }
}
