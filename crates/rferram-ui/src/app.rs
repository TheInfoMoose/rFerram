//! Main `libcosmic` / `iced` Application Shell, Header, Sidebar & Connection Card Grid
//!
//! Implements `RFerramApp`, `NavItem`, `ProfileUiState`, `Message`, the `cosmic::Application`
//! trait implementation, and the primary shell views (`view_header`, `view_sidebar`, and
//! `view_connection_grid`).

use std::collections::HashMap;

use cosmic::app::{Command, Core, Task};
use cosmic::widget::{
    ButtonStyle, column, container, divider, grid, icon, icon_button, menu_button, nav_button, row,
    scrollable, text, text_input,
};
use cosmic::{Application, Element};

use rferram_common::{ConnectionProfile, RuntimeEnvironment};
use rferram_protocol::WireMessage;

use crate::{
    AddDesktopModalState, AddDesktopModalTab, HelpSection, SessionConnectOutcome, SettingsSection,
    UiSessionController, VpnAdapterBinding,
};

/// Symbolic theme-adaptive icon identifier used in the application header bar.
pub const APP_HEADER_ICON_NAME: &str = "com.theinfomoose.rFerram-symbolic";

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
    pub active_context_menu: Option<String>,
    pub quick_add_menu_open: bool,
    pub exported_profiles: Vec<ConnectionProfile>,
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
    ToggleContextMenu(String),
    CloseContextMenu,
    ToggleQuickAddMenu,
    CloseQuickAddMenu,
    ImportConnections,
    ExportAllConnections,
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
    WindowResized(u32, u32),
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

    fn on_resize(&mut self, width: u32, height: u32) -> Option<Self::Message> {
        RFerramApp::on_resize(self, width, height)
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let mut root_children = vec![
            self.view_header(),
            row(
                16,
                vec![self.view_sidebar(), scrollable(self.view_connection_grid())],
            ),
        ];

        if self.quick_add_menu_open {
            root_children.push(self.view_quick_add_menu());
        }
        if let Some(ref profile_id) = self.active_context_menu
            && !self.is_card_visible_in_grid(profile_id)
        {
            root_children.push(self.view_card_context_menu(profile_id));
        }
        if self.add_modal_open {
            root_children.push(self.view_add_desktop_modal());
        }
        if self.settings_drawer_open {
            root_children.push(self.view_settings_drawer());
        }
        if self.diagnostics_drawer_open {
            root_children.push(self.view_help_drawer());
        }

        container(cosmic::CLASS_ROOT_SHELL, column(12, root_children))
    }
}

impl RFerramApp {
    pub fn new(mut core: Core) -> (Self, Command<Message>) {
        if core.window_width == 0 {
            core.window_width = 1440;
        }
        if core.window_height == 0 {
            core.window_height = 900;
        }
        if core.scale_factor <= 0.0 {
            core.scale_factor = 1.0;
        }
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
            active_context_menu: None,
            quick_add_menu_open: false,
            exported_profiles: Vec::new(),
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
            Message::ToggleContextMenu(profile_id) => {
                self.quick_add_menu_open = false;
                if self.active_context_menu.as_deref() == Some(profile_id.as_str()) {
                    self.active_context_menu = None;
                } else {
                    self.active_context_menu = Some(profile_id);
                }
            }
            Message::CloseContextMenu => {
                self.active_context_menu = None;
            }
            Message::ToggleQuickAddMenu => {
                self.active_context_menu = None;
                self.quick_add_menu_open = !self.quick_add_menu_open;
            }
            Message::CloseQuickAddMenu => {
                self.quick_add_menu_open = false;
            }
            Message::ImportConnections => {
                self.quick_add_menu_open = false;
                self.active_context_menu = None;
                if !self.exported_profiles.is_empty() {
                    let staged = self.exported_profiles.clone();
                    let mut added_any = false;
                    for prof in &staged {
                        if !self
                            .connection_cards
                            .iter()
                            .any(|c| c.profile.id == prof.id)
                        {
                            self.connection_cards
                                .push(ProfileUiState::from_profile(prof.clone(), false));
                            added_any = true;
                        }
                    }
                    if !added_any {
                        for mut prof in staged {
                            prof.id = uuid::Uuid::new_v4().to_string();
                            prof.friendly_name = format!("{} (Imported)", prof.friendly_name);
                            self.connection_cards
                                .push(ProfileUiState::from_profile(prof, false));
                        }
                    }
                    self.auto_persist_profiles();
                } else if let Some(config) = self.persistence_config.clone() {
                    let _ = self.load_profiles_from_config(&config);
                }
            }
            Message::ExportAllConnections => {
                self.quick_add_menu_open = false;
                self.active_context_menu = None;
                self.exported_profiles = self
                    .connection_cards
                    .iter()
                    .map(|c| c.profile.clone())
                    .collect();
            }
            Message::OpenAddDesktopModal => {
                self.quick_add_menu_open = false;
                self.active_context_menu = None;
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
                self.active_context_menu = None;
                self.quick_add_menu_open = false;
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
            Message::ConfigureConnection(profile_id) => {
                self.active_context_menu = None;
                self.quick_add_menu_open = false;
                if let Some(card) = self
                    .connection_cards
                    .iter()
                    .find(|c| c.profile.id == profile_id)
                {
                    self.add_modal_state = AddDesktopModalState {
                        active_tab: AddDesktopModalTab::General,
                        name_input: card.profile.friendly_name.clone(),
                        host_input: card.profile.host_address.clone(),
                        group_input: card.profile.group_name.clone(),
                        auto_reconnect: card.profile.auto_reconnect,
                        admin_session_takeover: card.profile.admin_session_takeover,
                        username_input: String::new(),
                        password_input: String::new(),
                        ask_credentials_on_connect: card.profile.ask_credentials_on_connect,
                        selected_vpn: card.vpn_binding.clone(),
                        target_monitor_index: 0,
                        scale_factor_percent: 100,
                        folder_shares: card.profile.folder_shares.clone(),
                        enable_speaker_audio: card.profile.enable_speaker_audio,
                        enable_mic_passthrough: card.profile.enable_mic_passthrough,
                    };
                    self.add_modal_open = true;
                }
            }
            Message::DuplicateConnection(profile_id) => {
                self.active_context_menu = None;
                self.quick_add_menu_open = false;
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
            Message::ExportConnection(profile_id) => {
                self.active_context_menu = None;
                self.quick_add_menu_open = false;
                if let Some(card) = self
                    .connection_cards
                    .iter()
                    .find(|c| c.profile.id == profile_id)
                {
                    if let Some(slot) = self
                        .exported_profiles
                        .iter_mut()
                        .find(|p| p.id == card.profile.id)
                    {
                        *slot = card.profile.clone();
                    } else {
                        self.exported_profiles.push(card.profile.clone());
                    }
                }
            }
            Message::DeleteConnection(profile_id) => {
                if self.active_context_menu.as_deref() == Some(profile_id.as_str()) {
                    self.active_context_menu = None;
                }
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
            Message::WindowResized(width, height) => {
                self.core.window_width = width.max(1);
                self.core.window_height = height.max(1);
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
        }
        Command::none()
    }

    /// Handles window viewport resize events, updating `self.core.window_width` and
    /// `self.core.window_height` and returning [`Message::WindowResized`].
    pub fn on_resize(&mut self, width: u32, height: u32) -> Option<Message> {
        let safe_w = width.max(1);
        let safe_h = height.max(1);
        self.core.window_width = safe_w;
        self.core.window_height = safe_h;
        Some(Message::WindowResized(safe_w, safe_h))
    }

    /// Computes the responsive connection card grid column count for a given `viewport_width`
    /// (`1` column in List mode; in Grid mode scales across `1024x768`, `1280x720`, `1440x900`,
    /// and `1920x1080` viewports).
    pub const fn grid_columns_for_width(viewport_width: u32, is_grid_view: bool) -> usize {
        if !is_grid_view {
            return 1;
        }
        if viewport_width >= 1800 {
            4
        } else if viewport_width >= 1200 {
            3
        } else if viewport_width >= 768 {
            2
        } else {
            1
        }
    }

    /// Returns the active connection grid column count for the current window viewport and view mode.
    pub const fn responsive_grid_columns(&self) -> usize {
        let effective_width = if self.core.window_width == 0 {
            1440
        } else {
            self.core.window_width
        };
        Self::grid_columns_for_width(effective_width, self.is_grid_view)
    }

    fn card_matches_filter(&self, card: &ProfileUiState, query_lower: &str) -> bool {
        if self.active_nav == NavItem::Favorites && !card.is_favorite {
            return false;
        }
        if query_lower.is_empty() {
            return true;
        }
        card.profile
            .friendly_name
            .to_ascii_lowercase()
            .contains(query_lower)
            || card
                .profile
                .host_address
                .to_ascii_lowercase()
                .contains(query_lower)
            || card
                .profile
                .group_name
                .to_ascii_lowercase()
                .contains(query_lower)
    }

    fn is_card_visible_in_grid(&self, profile_id: &str) -> bool {
        let query = self.search_query.to_ascii_lowercase();
        self.connection_cards
            .iter()
            .any(|c| c.profile.id == profile_id && self.card_matches_filter(c, &query))
    }

    /// Top Header Bar (Two-tier header matching `rFerramLayout-PNG.drawio.png`: upper tab bar with
    /// theme-adaptive `"rF"` emblem, `Connections` tab, Quick Add `+` icon button, and right-aligned
    /// `preferences-system-symbolic` / `help-browser-symbolic` icon buttons; secondary toolbar with
    /// circular `add-symbolic` button, segmented `view-grid-symbolic` / `view-list-symbolic` pill
    /// toggle, `view-sort-descending-symbolic` button, and `rferram-search-pill` containing
    /// `Search connections...` input + `system-search-symbolic` icon).
    pub fn view_header(&self) -> Element<'_, Message> {
        let top_tabbar = container(
            cosmic::CLASS_TOP_TABBAR,
            row(
                8,
                vec![
                    row(
                        6,
                        vec![
                            icon(APP_HEADER_ICON_NAME, 24),
                            text("Connections").size(16).into(),
                            icon_button("add-symbolic")
                                .active(self.quick_add_menu_open)
                                .on_press(Message::ToggleQuickAddMenu)
                                .into(),
                        ],
                    ),
                    row(
                        6,
                        vec![
                            icon_button("preferences-system-symbolic")
                                .label("Settings")
                                .on_press(Message::OpenSettingsDrawer)
                                .into(),
                            icon_button("help-browser-symbolic")
                                .label("?")
                                .on_press(Message::OpenDiagnosticsDrawer)
                                .into(),
                        ],
                    ),
                ],
            ),
        );

        let view_toggle_pill = container(
            cosmic::CLASS_SEGMENTED_PILL,
            row(
                4,
                vec![
                    icon_button("view-grid-symbolic")
                        .label(if self.is_grid_view { "Grid" } else { "" })
                        .style(ButtonStyle::Segmented)
                        .active(self.is_grid_view)
                        .on_press(Message::ToggleViewMode)
                        .into(),
                    icon_button("view-list-symbolic")
                        .label(if self.is_grid_view { "" } else { "List" })
                        .style(ButtonStyle::Segmented)
                        .active(!self.is_grid_view)
                        .on_press(Message::ToggleViewMode)
                        .into(),
                ],
            ),
        );

        let search_pill = container(
            cosmic::CLASS_SEARCH_PILL,
            row(
                6,
                vec![
                    text_input("Search connections...", &self.search_query)
                        .on_input(Message::SearchChanged)
                        .into(),
                    icon::from_name("system-search-symbolic").size(16).into(),
                ],
            ),
        );

        let toolbar = container(
            cosmic::CLASS_TOOLBAR,
            row(
                10,
                vec![
                    icon_button("add-symbolic")
                        .label("+")
                        .on_press(Message::OpenAddDesktopModal)
                        .into(),
                    view_toggle_pill,
                    icon_button("view-sort-descending-symbolic").into(),
                    search_pill,
                ],
            ),
        );

        container(cosmic::CLASS_HEADER, column(6, vec![top_tabbar, toolbar]))
    }

    /// Left Sidebar Navigation (`Favorites` with `emblem-favorite-symbolic`, `Groups` with `folder-symbolic`, and horizontal dividers)
    pub fn view_sidebar(&self) -> Element<'_, Message> {
        container(
            cosmic::CLASS_SIDEBAR,
            column(
                8,
                vec![
                    nav_button("Favorites")
                        .icon("emblem-favorite-symbolic")
                        .active(self.active_nav == NavItem::Favorites)
                        .on_press(Message::NavSelected(NavItem::Favorites))
                        .into(),
                    divider(),
                    nav_button("Groups")
                        .icon("folder-symbolic")
                        .active(self.active_nav == NavItem::AllDesktops)
                        .on_press(Message::NavSelected(NavItem::AllDesktops))
                        .into(),
                    divider(),
                ],
            ),
        )
    }

    /// Main Connection Card Grid grouped by raised `Group 1` tab badge above a full-width divider,
    /// styling each connection profile as a portrait dark-glass tile (`rferram-connection-card`) with
    /// top-right `view-more-horizontal-symbolic` and `emblem-favorite-symbolic` icon triggers, upper
    /// preview surface, and bottom-left friendly name + `Saved Profile` subtext.
    pub fn view_connection_grid(&self) -> Element<'_, Message> {
        let query = self.search_query.to_ascii_lowercase();
        let cards: Vec<Element<'_, Message>> = self
            .connection_cards
            .iter()
            .filter(|c| self.card_matches_filter(c, &query))
            .map(|c| {
                let pid = c.profile.id.clone();
                let is_menu_open = self.active_context_menu.as_deref() == Some(pid.as_str());
                let mut card_children: Vec<Element<'_, Message>> = vec![
                    row(
                        4,
                        vec![
                            icon_button("view-more-horizontal-symbolic")
                                .label("...")
                                .style(ButtonStyle::CardTrigger)
                                .active(is_menu_open)
                                .on_press(Message::ToggleContextMenu(pid.clone()))
                                .into(),
                            icon_button("emblem-favorite-symbolic")
                                .label(if c.is_favorite { "*" } else { "o" })
                                .style(ButtonStyle::CardTrigger)
                                .active(c.is_favorite)
                                .on_press(Message::ToggleFavorite(pid.clone()))
                                .into(),
                        ],
                    ),
                    container("rferram-card-preview", row(0, vec![])),
                    column(
                        4,
                        vec![
                            text(&c.profile.friendly_name).size(14).into(),
                            text("Saved Profile").size(11).into(),
                        ],
                    ),
                ];
                if is_menu_open {
                    card_children.push(self.view_card_context_menu(&pid));
                }
                container(cosmic::CLASS_CONNECTION_CARD, column(8, card_children))
            })
            .collect();

        container(
            cosmic::CLASS_GROUP_SECTION,
            column(
                10,
                vec![
                    container(cosmic::CLASS_GROUP_BADGE, text("Group 1").size(14)),
                    divider(),
                    grid(self.responsive_grid_columns(), 16, cards),
                ],
            ),
        )
    }

    /// Floating Card Context Menu (`Connect`, `Configure`, `Duplicate`, `Export`, `Delete`)
    /// matching Phase 3 of `UI_Fix.md` and `rFerramLayout-PNG.drawio.png`.
    pub fn view_card_context_menu(&self, profile_id: &str) -> Element<'_, Message> {
        let pid = profile_id.to_string();
        container(
            cosmic::CLASS_CONTEXT_MENU,
            column(
                4,
                vec![
                    menu_button("Connect")
                        .on_press(Message::ConnectSession(pid.clone()))
                        .into(),
                    divider(),
                    menu_button("Configure")
                        .on_press(Message::ConfigureConnection(pid.clone()))
                        .into(),
                    menu_button("Duplicate")
                        .on_press(Message::DuplicateConnection(pid.clone()))
                        .into(),
                    menu_button("Export")
                        .on_press(Message::ExportConnection(pid.clone()))
                        .into(),
                    menu_button("Delete")
                        .on_press(Message::DeleteConnection(pid))
                        .into(),
                ],
            ),
        )
    }

    /// Floating Quick Add Menu (`Add`, `Import`, `Export`) matching `rFerramLayout-PNG.drawio.png`.
    pub fn view_quick_add_menu(&self) -> Element<'_, Message> {
        container(
            cosmic::CLASS_ADD_MENU,
            column(
                4,
                vec![
                    menu_button("Add")
                        .on_press(Message::OpenAddDesktopModal)
                        .into(),
                    menu_button("Import")
                        .on_press(Message::ImportConnections)
                        .into(),
                    menu_button("Export")
                        .on_press(Message::ExportAllConnections)
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

    #[test]
    fn test_themed_header_and_window_icon_variants() {
        let (app, _) = RFerramApp::new(Core::new());
        let header_scene = app.view_header().layout_scene(1440.0, 900.0);
        let has_symbolic_logo = header_scene.boxes.iter().any(|b| {
            matches!(
                &b.node,
                cosmic::WidgetNode::Icon { name, size }
                    if name == APP_HEADER_ICON_NAME && *size == 24
            )
        });
        assert!(
            has_symbolic_logo,
            "expected symbolic theme-matched rFerram logo in view_header"
        );

        // Verify dark and light themes produce distinct theme-matched icons using the theme palette
        let dark_icon = cosmic::rasterize_themed_app_icon(64, &cosmic::Theme::dark());
        let light_icon = cosmic::rasterize_themed_app_icon(64, &cosmic::Theme::light());
        assert_eq!((dark_icon.width(), dark_icon.height()), (64, 64));
        assert_eq!((light_icon.width(), light_icon.height()), (64, 64));
        assert_ne!(
            dark_icon.data(),
            light_icon.data(),
            "embedded app logo must adapt to Dark vs Light theme rather than using a static full-color bitmap"
        );

        let dark_palette = cosmic::ThemePalette::from_theme(&cosmic::Theme::dark());
        let has_dark_accent = dark_icon.pixels().iter().any(|px| {
            px.red() == dark_palette.accent[0]
                && px.green() == dark_palette.accent[1]
                && px.blue() == dark_palette.accent[2]
        });
        assert!(
            has_dark_accent,
            "expected dark theme accent color in themed app icon"
        );

        let default_settings = cosmic::app::Settings::default();
        let (rgba, w, h) = default_settings
            .window_icon
            .expect("Settings::default must provide a themed window_icon");
        assert_eq!((w, h), (64, 64));
        assert_eq!(rgba.len(), 64 * 64 * 4);
    }

    #[test]
    fn test_two_tier_header_and_sidebar_mockup_parity() {
        let (mut app, _) = RFerramApp::new(Core::new());

        // 1. Verify two-tier header structure (CLASS_TOP_TABBAR stacked above CLASS_TOOLBAR)
        let header_scene = app.view_header().layout_scene(1440.0, 900.0);
        let top_tabbar = header_scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_TOP_TABBAR))
            .expect("missing rferram-top-tabbar in view_header");
        let toolbar = header_scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_TOOLBAR))
            .expect("missing rferram-toolbar in view_header");
        assert!(
            toolbar.y >= top_tabbar.y + top_tabbar.height,
            "secondary toolbar must sit below top tabbar in two-tier header"
        );

        let segmented_pill = header_scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_SEGMENTED_PILL))
            .expect("missing rferram-segmented-pill in view_header");
        let search_pill = header_scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_SEARCH_PILL))
            .expect("missing rferram-search-pill in view_header");
        assert!(
            search_pill.x > segmented_pill.x + segmented_pill.width,
            "rferram-search-pill must be right-aligned to the right of segmented view pill"
        );

        let header_texts = app.view_header().collect_text();
        for expected in [
            "Connections",
            "add-symbolic",
            "preferences-system-symbolic",
            "help-browser-symbolic",
            "view-grid-symbolic",
            "view-list-symbolic",
            "view-sort-descending-symbolic",
            "Search connections...",
        ] {
            assert!(
                header_texts.iter().any(|t| t == expected),
                "expected {expected:?} in view_header().collect_text()"
            );
        }

        let has_search_icon = header_scene.boxes.iter().any(|b| {
            matches!(
                &b.node,
                cosmic::WidgetNode::Icon { name, .. } if name == "system-search-symbolic"
            )
        });
        assert!(
            has_search_icon,
            "expected system-search-symbolic icon inside rferram-search-pill"
        );

        // 2. Verify sidebar full-width navigation rows with symbolic icons and horizontal dividers
        let sidebar_scene = app.view_sidebar().layout_scene(1440.0, 900.0);
        let fav_btn = sidebar_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Button {
                        label,
                        icon_name: Some(icon),
                        style: ButtonStyle::NavRow,
                        is_active,
                        ..
                    } if label == "Favorites" && icon == "emblem-favorite-symbolic" && !*is_active
                )
            })
            .expect("missing Favorites NavRow button with emblem-favorite-symbolic");
        let groups_btn = sidebar_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Button {
                        label,
                        icon_name: Some(icon),
                        style: ButtonStyle::NavRow,
                        is_active,
                        ..
                    } if label == "Groups" && icon == "folder-symbolic" && *is_active
                )
            })
            .expect("missing active Groups NavRow button with folder-symbolic");
        assert!(
            (fav_btn.width - groups_btn.width).abs() < 1.0 && fav_btn.width >= 150.0,
            "sidebar navigation rows must stretch to full sidebar width (fav={}, groups={})",
            fav_btn.width,
            groups_btn.width
        );

        let divider_count = sidebar_scene
            .boxes
            .iter()
            .filter(|b| matches!(b.node, cosmic::WidgetNode::Divider))
            .count();
        assert!(
            divider_count >= 2,
            "expected horizontal dividers separating sidebar rows (found {divider_count})"
        );

        // Switching to Favorites updates the active NavRow state
        let _ = app.update(Message::NavSelected(NavItem::Favorites));
        let fav_sidebar_scene = app.view_sidebar().layout_scene(1440.0, 900.0);
        let fav_active = fav_sidebar_scene.boxes.iter().any(|b| {
            matches!(
                &b.node,
                cosmic::WidgetNode::Button {
                    label,
                    style: ButtonStyle::NavRow,
                    is_active: true,
                    ..
                } if label == "Favorites"
            )
        });
        assert!(fav_active, "Favorites NavRow must be active when selected");
    }

    #[test]
    fn test_connection_grid_group_badge_and_portrait_card_anatomy() {
        let (app, _) = RFerramApp::new(Core::new());
        let grid_view = app.view_connection_grid();
        let grid_texts = grid_view.collect_text();

        for expected in [
            "Group 1",
            "macOS Workstation M3",
            "Wayland Arch Build Box",
            "Saved Profile",
            "view-more-horizontal-symbolic",
            "emblem-favorite-symbolic",
        ] {
            assert!(
                grid_texts.iter().any(|t| t == expected),
                "expected {expected:?} in view_connection_grid().collect_text()"
            );
        }

        let scene = grid_view.layout_scene(1440.0, 900.0);

        // 1. Raised Group 1 tab badge above a full-width divider
        let group_badge = scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_GROUP_BADGE))
            .expect("missing rferram-group-badge in view_connection_grid");
        let group_divider = scene
            .boxes
            .iter()
            .find(|b| matches!(b.node, cosmic::WidgetNode::Divider))
            .expect("missing group divider below Group 1 badge");
        assert!(
            group_divider.y >= group_badge.y + group_badge.height,
            "full-width divider must sit below raised Group 1 tab badge"
        );
        assert!(
            group_divider.width > group_badge.width * 2.0,
            "divider (width={}) must span wider than the compact Group 1 tab badge (width={})",
            group_divider.width,
            group_badge.width
        );

        // 2. Portrait dark-glass connection cards with top-right triggers, upper preview surface, and bottom-left labels
        let cards: Vec<_> = scene
            .boxes
            .iter()
            .filter(|b| b.class.as_deref() == Some(cosmic::CLASS_CONNECTION_CARD))
            .collect();
        assert_eq!(cards.len(), 2);
        for card in &cards {
            assert!(
                card.height > card.width,
                "connection card must have portrait aspect ratio (height={} > width={})",
                card.height,
                card.width
            );
        }

        let first_card = cards[0];
        let more_trigger = scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Button {
                        label,
                        icon_name: Some(icon),
                        style: ButtonStyle::CardTrigger,
                        ..
                    } if label == "..." && icon == "view-more-horizontal-symbolic"
                )
            })
            .expect("missing view-more-horizontal-symbolic CardTrigger button");
        let fav_trigger = scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Button {
                        label,
                        icon_name: Some(icon),
                        style: ButtonStyle::CardTrigger,
                        is_active: true,
                        ..
                    } if label == "*" && icon == "emblem-favorite-symbolic"
                )
            })
            .expect("missing active emblem-favorite-symbolic CardTrigger button on prof-1");

        assert!(
            more_trigger.x > first_card.x + first_card.width * 0.45
                && fav_trigger.x > more_trigger.x,
            "top-right card triggers must be right-aligned in the upper-right of the portrait card"
        );

        let has_preview_surface = scene
            .boxes
            .iter()
            .any(|b| b.class.as_deref() == Some("rferram-card-preview"));
        assert!(
            has_preview_surface,
            "expected rferram-card-preview surface inside connection card"
        );

        let friendly_label = scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Text { content, .. }
                        if content == "macOS Workstation M3"
                )
            })
            .expect("missing friendly name text box on first card");
        let subtext_label = scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Text { content, .. }
                        if content == "Saved Profile"
                )
            })
            .expect("missing Saved Profile subtext box on first card");
        assert!(
            friendly_label.y > first_card.y + first_card.height * 0.5
                && subtext_label.y > friendly_label.y,
            "friendly name and Saved Profile subtext must be positioned in the bottom-left of the portrait card"
        );
    }

    #[test]
    fn test_floating_card_context_menu_and_quick_add_menu_actions() {
        let (mut app, _) = RFerramApp::new(Core::new());
        assert_eq!(app.active_context_menu, None);
        assert!(!app.quick_add_menu_open);

        // 1. Open floating card context menu for prof-1 ("macOS Workstation M3")
        let _ = app.update(Message::ToggleContextMenu("prof-1".to_string()));
        assert_eq!(app.active_context_menu.as_deref(), Some("prof-1"));

        let root_view = Application::view(&app);
        let root_texts = root_view.collect_text();
        for item in ["Connect", "Configure", "Duplicate", "Export", "Delete"] {
            assert!(
                root_texts.iter().any(|t| t == item),
                "expected context menu item {item:?} when card context menu is open"
            );
        }

        let scene = root_view.layout_scene(1440.0, 900.0);
        let ctx_menu = scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_CONTEXT_MENU))
            .expect("missing rferram-context-menu overlay in Application::view");
        assert!(ctx_menu.z_index > 0);

        // Toggling the same card's context menu again closes it
        let _ = app.update(Message::ToggleContextMenu("prof-1".to_string()));
        assert_eq!(app.active_context_menu, None);

        // 2. Configure action: populates AddDesktopModalState with prof-1 and opens modal
        let _ = app.update(Message::ToggleContextMenu("prof-1".to_string()));
        let _ = app.update(Message::ConfigureConnection("prof-1".to_string()));
        assert_eq!(app.active_context_menu, None);
        assert!(app.add_modal_open);
        assert_eq!(app.add_modal_state.name_input, "macOS Workstation M3");
        assert_eq!(app.add_modal_state.host_input, "10.0.10.15:4433");
        assert_eq!(
            app.add_modal_state.selected_vpn,
            VpnAdapterBinding::WireGuard("utun3".to_string())
        );
        let _ = app.update(Message::CloseAddDesktopModal);

        // 3. Duplicate action: clones prof-1 with "(Copy)" suffix and closes context menu
        let _ = app.update(Message::ToggleContextMenu("prof-1".to_string()));
        let _ = app.update(Message::DuplicateConnection("prof-1".to_string()));
        assert_eq!(app.active_context_menu, None);
        assert_eq!(app.connection_cards.len(), 3);
        assert_eq!(
            app.connection_cards[2].profile.friendly_name,
            "macOS Workstation M3 (Copy)"
        );

        // 4. Export action: exports prof-2 into exported_profiles and closes context menu
        let _ = app.update(Message::ToggleContextMenu("prof-2".to_string()));
        let _ = app.update(Message::ExportConnection("prof-2".to_string()));
        assert_eq!(app.active_context_menu, None);
        assert_eq!(app.exported_profiles.len(), 1);
        assert_eq!(app.exported_profiles[0].id, "prof-2");
        assert_eq!(
            app.exported_profiles[0].friendly_name,
            "Wayland Arch Build Box"
        );

        // 5. Delete action: removes prof-2 and closes context menu
        let _ = app.update(Message::ToggleContextMenu("prof-2".to_string()));
        let _ = app.update(Message::DeleteConnection("prof-2".to_string()));
        assert_eq!(app.active_context_menu, None);
        assert_eq!(app.connection_cards.len(), 2);
        assert!(
            !app.connection_cards
                .iter()
                .any(|c| c.profile.id == "prof-2")
        );

        // 6. Quick Add menu (Add, Import, Export): open menu, verify overlay, and import exported prof-2 back
        let _ = app.update(Message::ToggleQuickAddMenu);
        assert!(app.quick_add_menu_open);
        let add_menu_view = Application::view(&app);
        let add_menu_texts = add_menu_view.collect_text();
        for item in ["Add", "Import", "Export"] {
            assert!(
                add_menu_texts.iter().any(|t| t == item),
                "expected Quick Add menu item {item:?}"
            );
        }
        let add_menu_scene = add_menu_view.layout_scene(1440.0, 900.0);
        assert!(
            add_menu_scene
                .boxes
                .iter()
                .any(|b| b.class.as_deref() == Some(cosmic::CLASS_ADD_MENU) && b.z_index > 0)
        );

        let _ = app.update(Message::ImportConnections);
        assert!(!app.quick_add_menu_open);
        assert_eq!(app.connection_cards.len(), 3);
        assert!(
            app.connection_cards
                .iter()
                .any(|c| c.profile.id == "prof-2")
        );

        // Export all connections via Quick Add menu
        let _ = app.update(Message::ToggleQuickAddMenu);
        let _ = app.update(Message::ExportAllConnections);
        assert!(!app.quick_add_menu_open);
        assert_eq!(app.exported_profiles.len(), 3);

        // 7. Connect action from context menu: closes menu and transitions status to Connecting
        let _ = app.update(Message::ToggleContextMenu("prof-1".to_string()));
        let _ = app.update(Message::ConnectSession("prof-1".to_string()));
        assert_eq!(app.active_context_menu, None);
        assert_eq!(app.session_status("prof-1"), Some("Connecting"));

        // 8. Interactive CosmicAppRunner pointer click workflow for card context menu & Quick Add menu
        let mut runner =
            cosmic::CosmicAppRunner::<RFerramApp>::headless(cosmic::app::Settings::default(), ());
        let initial_scene = runner.layout_scene();
        let card_more_btn = initial_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Button {
                        label,
                        icon_name: Some(icon),
                        style: ButtonStyle::CardTrigger,
                        ..
                    } if label == "..." && icon == "view-more-horizontal-symbolic"
                )
            })
            .expect("missing card '...' trigger in runner scene");
        assert!(runner.handle_pointer_click(
            card_more_btn.x + card_more_btn.width * 0.5,
            card_more_btn.y + card_more_btn.height * 0.5,
        ));
        assert_eq!(runner.app.active_context_menu.as_deref(), Some("prof-1"));

        let menu_scene = runner.layout_scene();
        let dup_btn = menu_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Button {
                        label,
                        style: ButtonStyle::MenuItem,
                        ..
                    } if label == "Duplicate"
                )
            })
            .expect("missing Duplicate MenuItem in context menu");
        assert!(runner.handle_pointer_click(
            dup_btn.x + dup_btn.width * 0.5,
            dup_btn.y + dup_btn.height * 0.5,
        ));
        assert_eq!(runner.app.active_context_menu, None);
        assert_eq!(runner.app.connection_cards.len(), 3);

        // Click top-tabbar Quick Add button to open Quick Add menu, then click "Add"
        let header_scene = runner.layout_scene();
        let quick_add_btn = header_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Button {
                        label,
                        icon_name: Some(icon),
                        style: ButtonStyle::Icon,
                        ..
                    } if label == "add-symbolic" && icon == "add-symbolic"
                )
            })
            .expect("missing top-tabbar Quick Add icon button");
        assert!(runner.handle_pointer_click(
            quick_add_btn.x + quick_add_btn.width * 0.5,
            quick_add_btn.y + quick_add_btn.height * 0.5,
        ));
        assert!(runner.app.quick_add_menu_open);

        let quick_menu_scene = runner.layout_scene();
        let quick_add_item = quick_menu_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Button {
                        label,
                        style: ButtonStyle::MenuItem,
                        ..
                    } if label == "Add"
                )
            })
            .expect("missing Add MenuItem in Quick Add menu");
        assert!(runner.handle_pointer_click(
            quick_add_item.x + quick_add_item.width * 0.5,
            quick_add_item.y + quick_add_item.height * 0.5,
        ));
        assert!(!runner.app.quick_add_menu_open);
        assert!(runner.app.add_modal_open);
    }

    #[test]
    fn test_ui_mockup_parity_icons_menus_and_responsive_scaling() {
        let mut runner =
            cosmic::CosmicAppRunner::<RFerramApp>::headless(cosmic::app::Settings::default(), ());

        // 1. Open all overlays (Quick Add menu, Card Context menu, Add Desktop modal, Settings drawer, Help drawer)
        // and verify all symbolic icons from UI_Fix.md + rFerramLayout-PNG.drawio.png are bound in the scene tree.
        let _ = runner.app.update(Message::OpenAddDesktopModal);
        let _ = runner
            .app
            .update(Message::SelectAddModalTab(AddDesktopModalTab::Folders));
        let _ = runner.app.update(Message::OpenSettingsDrawer);
        let _ = runner.app.update(Message::OpenDiagnosticsDrawer);
        let _ = runner.app.update(Message::ToggleQuickAddMenu);
        runner.app.active_context_menu = Some(String::from("prof-1"));

        let folders_scene = runner.layout_scene();
        assert!(
            folders_scene.boxes.iter().any(|b| matches!(
                &b.node,
                cosmic::WidgetNode::Icon { name, .. } if name == "folder-remote-symbolic"
            )),
            "expected folder-remote-symbolic icon in Add Desktop Folders tab"
        );

        // Switch back to General tab so all General modal form rows and checkboxes are active
        let _ = runner
            .app
            .update(Message::SelectAddModalTab(AddDesktopModalTab::General));
        let full_view = Application::view(&runner.app);
        let full_scene = full_view.layout_scene(1440.0, 900.0);

        for expected_icon in [
            APP_HEADER_ICON_NAME,
            "add-symbolic",
            "preferences-system-symbolic",
            "help-browser-symbolic",
            "view-grid-symbolic",
            "view-list-symbolic",
            "view-sort-descending-symbolic",
            "system-search-symbolic",
            "emblem-favorite-symbolic",
            "folder-symbolic",
            "view-more-horizontal-symbolic",
            "input-keyboard-symbolic",
            "video-display-symbolic",
            "user-bookmarks-symbolic",
            "avatar-default-symbolic",
            "network-vpn-symbolic",
            "document-open-symbolic",
            "tools-symbolic",
            "dialog-warning-symbolic",
            "lab-symbolic",
        ] {
            let found = full_scene.boxes.iter().any(|b| match &b.node {
                cosmic::WidgetNode::Icon { name, .. } => name == expected_icon,
                cosmic::WidgetNode::Button {
                    label, icon_name, ..
                } => label == expected_icon || icon_name.as_deref() == Some(expected_icon),
                _ => false,
            });
            assert!(
                found,
                "expected symbolic icon {expected_icon:?} in full UI scene"
            );
        }

        // 2. Verify popup menus (Quick Add menu & Card Context menu) with MenuItem buttons and positive z-index
        let add_menu = full_scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_ADD_MENU))
            .expect("missing rferram-add-menu overlay");
        let ctx_menu = full_scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_CONTEXT_MENU))
            .expect("missing rferram-context-menu overlay");
        assert!(add_menu.z_index > 0);
        assert!(ctx_menu.z_index > add_menu.z_index);

        for menu_label in [
            "Add",
            "Import",
            "Export",
            "Connect",
            "Configure",
            "Duplicate",
            "Delete",
        ] {
            assert!(
                full_scene.boxes.iter().any(|b| matches!(
                    &b.node,
                    cosmic::WidgetNode::Button {
                        label,
                        style: ButtonStyle::MenuItem,
                        ..
                    } if label == menu_label
                )),
                "expected MenuItem button {menu_label:?} in popup menus"
            );
        }

        // 3. Verify inline modal form rows (label on left, input/dropdown on right), equal-width segmented tabs,
        // square checkboxes, and right-aligned Cancel/Add footer
        let modal_box = full_scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_ADD_DESKTOP_MODAL))
            .expect("missing rferram-add-desktop-modal overlay");
        assert!(modal_box.z_index > ctx_menu.z_index);

        let form_rows: Vec<_> = full_scene
            .boxes
            .iter()
            .filter(|b| b.class.as_deref() == Some(cosmic::CLASS_FORM_ROW))
            .collect();
        assert_eq!(
            form_rows.len(),
            5,
            "expected 5 inline rferram-form-row containers in Add Desktop General tab"
        );

        let conn_label = full_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Text { content, .. } if content == "Connection:"
                )
            })
            .expect("missing Connection: label");
        let host_input = full_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::TextInput { placeholder, .. }
                        if placeholder == "Host name or IP address"
                )
            })
            .expect("missing Host input");
        assert!(
            host_input.x > conn_label.x + conn_label.width,
            "inline modal row must position Host input to the right of Connection: label"
        );

        let checkbox_count = full_scene
            .boxes
            .iter()
            .filter(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Toggler {
                        style: cosmic::TogglerStyle::Checkbox,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(checkbox_count, 2, "expected 2 square Checkbox controls");

        // 4. Verify split-pane drawers (rferram-drawer-nav on the left, rferram-drawer-content on the right)
        for drawer_class in [cosmic::CLASS_SETTINGS_DRAWER, cosmic::CLASS_HELP_DRAWER] {
            let drawer_box = full_scene
                .boxes
                .iter()
                .find(|b| b.class.as_deref() == Some(drawer_class))
                .unwrap_or_else(|| panic!("missing {drawer_class} overlay"));
            assert!((drawer_box.x + drawer_box.width - 1440.0).abs() < 1.0);
        }
        let nav_panes: Vec<_> = full_scene
            .boxes
            .iter()
            .filter(|b| b.class.as_deref() == Some(cosmic::CLASS_DRAWER_NAV))
            .collect();
        let content_panes: Vec<_> = full_scene
            .boxes
            .iter()
            .filter(|b| b.class.as_deref() == Some(cosmic::CLASS_DRAWER_CONTENT))
            .collect();
        assert_eq!(nav_panes.len(), 2);
        assert_eq!(content_panes.len(), 2);
        for (nav, content) in nav_panes.iter().zip(content_panes.iter()) {
            assert!(content.x >= nav.x + nav.width);
            assert!(content.width > nav.width);
        }

        // 5. Close overlays, populate 4 connection cards, and verify Application::on_resize and responsive grid
        // column scaling across 1024x768, 1280x720, 1440x900, and 1920x1080 viewports
        let _ = runner.app.update(Message::CloseAddDesktopModal);
        let _ = runner.app.update(Message::CloseSettingsDrawer);
        let _ = runner.app.update(Message::CloseDiagnosticsDrawer);
        let _ = runner.app.update(Message::CloseQuickAddMenu);
        let _ = runner.app.update(Message::CloseContextMenu);

        let _ = runner
            .app
            .update(Message::DuplicateConnection("prof-1".to_string()));
        let _ = runner
            .app
            .update(Message::DuplicateConnection("prof-2".to_string()));
        assert_eq!(runner.app.connection_cards.len(), 4);

        // Direct Application::on_resize hook verification
        let resize_msg = Application::on_resize(&mut runner.app, 1024, 768);
        assert!(matches!(
            resize_msg,
            Some(Message::WindowResized(1024, 768))
        ));
        assert_eq!(runner.app.core().window_width, 1024);
        assert_eq!(runner.app.core().window_height, 768);

        let viewports = [
            (1024u32, 768u32, 2usize),
            (1280u32, 720u32, 3usize),
            (1440u32, 900u32, 3usize),
            (1920u32, 1080u32, 4usize),
        ];

        for (width, height, expected_cols) in viewports {
            assert!(runner.handle_resize(width, height));
            assert_eq!(runner.viewport_size(), (width, height));
            assert_eq!(runner.app.core().window_width, width);
            assert_eq!(runner.app.core().window_height, height);
            assert_eq!(
                runner.app.responsive_grid_columns(),
                expected_cols,
                "expected {expected_cols} grid columns at {width}x{height}"
            );

            let scene = runner.layout_scene();
            assert_eq!((scene.width(), scene.height()), (width, height));

            let grid_box = scene
                .boxes
                .iter()
                .find(|b| matches!(b.node, cosmic::WidgetNode::Grid { .. }))
                .expect("missing WidgetNode::Grid in connection grid scene");
            if let cosmic::WidgetNode::Grid { columns, .. } = grid_box.node {
                assert_eq!(
                    columns, expected_cols,
                    "WidgetNode::Grid columns mismatch at {width}x{height}"
                );
            }

            let cards: Vec<_> = scene
                .boxes
                .iter()
                .filter(|b| b.class.as_deref() == Some(cosmic::CLASS_CONNECTION_CARD))
                .collect();
            assert_eq!(cards.len(), 4);

            for card in &cards {
                assert!(
                    card.height > card.width,
                    "card must remain portrait at {width}x{height} (w={}, h={})",
                    card.width,
                    card.height
                );
                assert!(
                    card.x + card.width <= width as f32 + 0.5,
                    "card right edge ({}) must fit inside viewport width {width}",
                    card.x + card.width
                );
            }

            // First `expected_cols` cards sit side-by-side on the first row
            for col_idx in 1..expected_cols {
                assert!(
                    (cards[col_idx].y - cards[0].y).abs() < 1.0,
                    "card {col_idx} must share row 0 with card 0 at {width}x{height}"
                );
                assert!(
                    cards[col_idx].x > cards[col_idx - 1].x + cards[col_idx - 1].width,
                    "card {col_idx} must sit to the right of card {} at {width}x{height}",
                    col_idx - 1
                );
            }
            // If 4 cards exceed `expected_cols`, card[expected_cols] wraps onto row 1
            if expected_cols < cards.len() {
                assert!(
                    cards[expected_cols].y > cards[0].y + cards[0].height * 0.5,
                    "card {expected_cols} must wrap to second row at {width}x{height}"
                );
            }

            let (aligned_buf, packed_w, packed_h, stride) = runner.pack_aligned_frame();
            assert_eq!((packed_w, packed_h), (width, height));
            assert_eq!(stride % cosmic::WGPU_ROW_ALIGNMENT, 0);
            assert_eq!(aligned_buf.len(), (stride * height) as usize);
        }

        // Toggling to List mode forces 1 column regardless of viewport width
        let _ = runner.app.update(Message::ToggleViewMode);
        assert!(!runner.app.is_grid_view);
        assert_eq!(runner.app.responsive_grid_columns(), 1);
        let _ = runner.app.update(Message::ToggleViewMode);
        assert!(runner.app.is_grid_view);

        // 6. Verify card context menu anchors to the specific active card (prof-2 and card 2)
        // and reflows with its parent card across 1024x768 (row 1) and 1280x720 (row 0)
        let third_card_id = runner.app.connection_cards[2].profile.id.clone();
        let _ = runner
            .app
            .update(Message::ToggleContextMenu(third_card_id.clone()));
        assert!(runner.handle_resize(1024, 768));
        let scene_1024 = runner.layout_scene();
        let cards_1024: Vec<_> = scene_1024
            .boxes
            .iter()
            .filter(|b| b.class.as_deref() == Some(cosmic::CLASS_CONNECTION_CARD))
            .collect();
        let menu_1024 = scene_1024
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_CONTEXT_MENU))
            .expect("missing context menu at 1024x768");
        assert!(
            menu_1024.y >= cards_1024[2].y && menu_1024.y < cards_1024[2].y + cards_1024[2].height,
            "context menu on card 2 must anchor to row 1 at 1024x768 (menu_y={}, card2_y={})",
            menu_1024.y,
            cards_1024[2].y
        );

        assert!(runner.handle_resize(1280, 720));
        let scene_1280 = runner.layout_scene();
        let cards_1280: Vec<_> = scene_1280
            .boxes
            .iter()
            .filter(|b| b.class.as_deref() == Some(cosmic::CLASS_CONNECTION_CARD))
            .collect();
        let menu_1280 = scene_1280
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_CONTEXT_MENU))
            .expect("missing context menu at 1280x720");
        assert!(
            menu_1280.x >= cards_1280[2].x
                && (menu_1280.y - (cards_1280[2].y + 42.0)).abs() < 2.0
                && menu_1280.y < menu_1024.y,
            "context menu on card 2 must reflow with card 2 to row 0 column 2 at 1280x720 (menu_x={}, card2_x={}, menu_y={})",
            menu_1280.x,
            cards_1280[2].x,
            menu_1280.y
        );

        // 7. Verify fractional HiDPI scaling (1.5x and 2.0x) combined with rapid resize sequences
        // (1024x768 -> 1280x720 -> 1440x900 -> 1920x1080 -> 3840x2160) while popup menus and drawers are open
        let _ = runner.app.update(Message::OpenSettingsDrawer);
        let _ = runner.app.update(Message::OpenDiagnosticsDrawer);
        let _ = runner.app.update(Message::ToggleQuickAddMenu);
        runner.app.active_context_menu = Some(third_card_id);

        for scale in [1.5_f32, 2.0_f32] {
            assert!(runner.handle_scale_factor_changed(scale));
            assert!((runner.app.core().scale_factor - scale).abs() < f32::EPSILON);
            for (w, h, cols) in [
                (1024u32, 768u32, 2usize),
                (1280u32, 720u32, 3usize),
                (1440u32, 900u32, 3usize),
                (1920u32, 1080u32, 4usize),
                (3840u32, 2160u32, 4usize),
            ] {
                assert!(runner.handle_resize(w, h));
                assert_eq!(runner.app.responsive_grid_columns(), cols);
                let overlay_scene = runner.layout_scene();
                assert_eq!((overlay_scene.width(), overlay_scene.height()), (w, h));
                let settings_drawer = overlay_scene
                    .boxes
                    .iter()
                    .find(|b| b.class.as_deref() == Some(cosmic::CLASS_SETTINGS_DRAWER))
                    .expect("missing settings drawer during HiDPI resize");
                assert!((settings_drawer.x + settings_drawer.width - w as f32).abs() < 1.0);
            }
        }
        let _ = runner.app.update(Message::CloseSettingsDrawer);
        let _ = runner.app.update(Message::CloseDiagnosticsDrawer);
        let _ = runner.app.update(Message::CloseQuickAddMenu);
        let _ = runner.app.update(Message::CloseContextMenu);

        // 8. Verify Core::default() normalization and scroll_offset_y clamping across resize
        let (default_core_app, _) = RFerramApp::new(Core::default());
        assert_eq!(default_core_app.core().window_width, 1440);
        assert_eq!(default_core_app.core().window_height, 900);
        assert_eq!(default_core_app.responsive_grid_columns(), 3);

        for idx in 0..8 {
            let _ = runner
                .app
                .update(Message::DuplicateConnection("prof-1".to_string()));
            let _ = idx;
        }
        assert!(runner.handle_resize(1024, 768));
        let scrolled = runner.set_scroll_offset(10_000.0);
        assert!(scrolled > 0.0);
        assert!(runner.handle_resize(1920, 1080));
        let max_at_1080 = runner.layout_scene().max_scroll_offset();
        assert!(
            runner.scroll_offset_y <= max_at_1080 + 0.5,
            "handle_resize must clamp scroll_offset_y ({}) to new max_scroll_offset ({})",
            runner.scroll_offset_y,
            max_at_1080
        );

        // Boundary thresholds and degenerate (0, 0) resize clamping
        assert_eq!(RFerramApp::grid_columns_for_width(0, true), 1);
        assert_eq!(RFerramApp::grid_columns_for_width(767, true), 1);
        assert_eq!(RFerramApp::grid_columns_for_width(768, true), 2);
        assert_eq!(RFerramApp::grid_columns_for_width(1199, true), 2);
        assert_eq!(RFerramApp::grid_columns_for_width(1200, true), 3);
        assert_eq!(RFerramApp::grid_columns_for_width(1799, true), 3);
        assert_eq!(RFerramApp::grid_columns_for_width(1800, true), 4);

        let zero_msg = Application::on_resize(&mut runner.app, 0, 0);
        assert!(matches!(zero_msg, Some(Message::WindowResized(1, 1))));
        assert_eq!(runner.app.core().window_width, 1);
        assert_eq!(runner.app.core().window_height, 1);
        assert_eq!(runner.app.responsive_grid_columns(), 1);
    }
}
