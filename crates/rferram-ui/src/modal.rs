//! Tabbed "Add Desktop" Configuration Modal & VPN Adapter Binding Selector
//!
//! Implements `AddDesktopModalTab`, `VpnAdapterBinding`, `AddDesktopModalState`,
//! and `RFerramApp::view_add_desktop_modal` matching the visual layout mockup.

use cosmic::Element;
use cosmic::widget::{
    button, column, container, divider, dropdown, icon, row, text, text_input, toggler,
};

use crate::{Message, RFerramApp};

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

    pub fn from_interface_option(interface: Option<&str>) -> Self {
        match interface.map(str::trim) {
            None | Some("") => Self::Direct,
            Some("tailscale0") => Self::Tailscale,
            Some("unifi0") => Self::UniFiVpn,
            Some(if_name)
                if if_name.starts_with("wg")
                    || if_name.starts_with("utun")
                    || if_name.starts_with("wintun") =>
            {
                Self::WireGuard(if_name.to_string())
            }
            Some(if_name) => Self::CustomAdapter(if_name.to_string()),
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

impl RFerramApp {
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

        let selected_vpn_idx = match &self.add_modal_state.selected_vpn {
            VpnAdapterBinding::Direct => 0,
            VpnAdapterBinding::WireGuard(_) => 1,
            VpnAdapterBinding::Tailscale => 2,
            VpnAdapterBinding::UniFiVpn => 3,
            VpnAdapterBinding::CustomAdapter(_) => 0,
        };

        let tab_body = match self.add_modal_state.active_tab {
            AddDesktopModalTab::General
            | AddDesktopModalTab::Credentials
            | AddDesktopModalTab::VpnSelect => column(
                8,
                vec![
                    text("Friendly Name:").into(),
                    text_input("Optional local only name", &self.add_modal_state.name_input)
                        .on_input(Message::UpdateModalName)
                        .into(),
                    text("Group:").into(),
                    dropdown(vec![String::from("Group 1")], Some(0))
                        .on_select(|_| Message::UpdateModalGroup(String::from("Group 1"))),
                    divider(),
                    text("VPN:").into(),
                    dropdown(
                        vec![
                            String::from("None"),
                            String::from("WireGuard"),
                            String::from("Tailscale"),
                            String::from("UniFi VPN"),
                        ],
                        Some(selected_vpn_idx),
                    )
                    .on_select(|idx| {
                        let vpn = match idx {
                            1 => VpnAdapterBinding::WireGuard(String::from("wg0")),
                            2 => VpnAdapterBinding::Tailscale,
                            3 => VpnAdapterBinding::UniFiVpn,
                            _ => VpnAdapterBinding::Direct,
                        };
                        Message::SelectModalVpn(vpn)
                    }),
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
                    text_input("Host name or IP address", &self.add_modal_state.host_input)
                        .on_input(Message::UpdateModalHost)
                        .into(),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::Application;
    use cosmic::app::Core;

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

    #[test]
    fn test_modal_tabs_and_vpn_adapter_binding_mappings() {
        assert_eq!(VpnAdapterBinding::Direct.to_interface_option(), None);
        assert_eq!(VpnAdapterBinding::Direct.label(), "None");
        assert_eq!(
            VpnAdapterBinding::WireGuard("utun3".to_string()).to_interface_option(),
            Some("utun3".to_string())
        );
        assert_eq!(
            VpnAdapterBinding::WireGuard("utun3".to_string()).label(),
            "WireGuard"
        );
        assert_eq!(
            VpnAdapterBinding::Tailscale.to_interface_option(),
            Some("tailscale0".to_string())
        );
        assert_eq!(VpnAdapterBinding::Tailscale.label(), "Tailscale");
        assert_eq!(
            VpnAdapterBinding::UniFiVpn.to_interface_option(),
            Some("unifi0".to_string())
        );
        assert_eq!(VpnAdapterBinding::UniFiVpn.label(), "UniFi VPN");
        assert_eq!(
            VpnAdapterBinding::CustomAdapter("tun9".to_string()).to_interface_option(),
            Some("tun9".to_string())
        );
        assert_eq!(
            VpnAdapterBinding::CustomAdapter("tun9".to_string()).label(),
            "tun9"
        );

        assert_eq!(
            VpnAdapterBinding::from_interface_option(None),
            VpnAdapterBinding::Direct
        );
        assert_eq!(
            VpnAdapterBinding::from_interface_option(Some("")),
            VpnAdapterBinding::Direct
        );
        assert_eq!(
            VpnAdapterBinding::from_interface_option(Some("   ")),
            VpnAdapterBinding::Direct
        );
        assert_eq!(
            VpnAdapterBinding::from_interface_option(Some("wg0")),
            VpnAdapterBinding::WireGuard("wg0".to_string())
        );
        assert_eq!(
            VpnAdapterBinding::from_interface_option(Some("utun3")),
            VpnAdapterBinding::WireGuard("utun3".to_string())
        );
        assert_eq!(
            VpnAdapterBinding::from_interface_option(Some("wintun0")),
            VpnAdapterBinding::WireGuard("wintun0".to_string())
        );
        assert_eq!(
            VpnAdapterBinding::from_interface_option(Some("tailscale0")),
            VpnAdapterBinding::Tailscale
        );
        assert_eq!(
            VpnAdapterBinding::from_interface_option(Some("unifi0")),
            VpnAdapterBinding::UniFiVpn
        );
        assert_eq!(
            VpnAdapterBinding::from_interface_option(Some("tun9")),
            VpnAdapterBinding::CustomAdapter("tun9".to_string())
        );

        let (mut app, _) = RFerramApp::new(Core::new());
        let _ = app.update(Message::OpenAddDesktopModal);

        // Switch through tabs and verify tab-specific content renders
        let _ = app.update(Message::SelectAddModalTab(AddDesktopModalTab::Display));
        let display_labels = app.view_add_desktop_modal().collect_text();
        assert!(display_labels.iter().any(|t| t == "Resolution | Scaling"));

        let _ = app.update(Message::SelectAddModalTab(AddDesktopModalTab::DevicesAudio));
        let audio_labels = app.view_add_desktop_modal().collect_text();
        assert!(
            audio_labels
                .iter()
                .any(|t| t == "Enable Microphone Passthrough")
        );

        let _ = app.update(Message::SelectAddModalTab(AddDesktopModalTab::Folders));
        let folder_labels = app.view_add_desktop_modal().collect_text();
        assert!(
            folder_labels
                .iter()
                .any(|t| t == "Shared Folders (ST_Rust_Age Snapshot Engine)")
        );
    }

    #[test]
    fn test_header_and_modal_widget_callbacks() {
        let (mut app, _) = RFerramApp::new(Core::new());

        // 1. Verify SearchChanged callback in view_header
        let header_scene = app.view_header().layout_scene(1440.0, 900.0);
        let search_box = header_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::TextInput { placeholder, .. }
                        if placeholder == "Search connections..."
                )
            })
            .expect("missing search input node");
        let search_msg = search_box
            .node
            .emit_input("arch")
            .expect("SearchChanged callback missing");
        let _ = app.update(search_msg);
        assert_eq!(app.search_query, "arch");

        // 2. Verify UpdateModalHost, UpdateModalName, and SelectModalVpn callbacks in view_add_desktop_modal
        let _ = app.update(Message::OpenAddDesktopModal);
        let modal_scene = app.view_add_desktop_modal().layout_scene(1440.0, 900.0);

        let host_box = modal_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::TextInput { placeholder, .. }
                        if placeholder == "Host name or IP address"
                )
            })
            .expect("missing modal host input node");
        let host_msg = host_box
            .node
            .emit_input("10.55.0.12:4433")
            .expect("UpdateModalHost callback missing");
        let _ = app.update(host_msg);
        assert_eq!(app.add_modal_state.host_input, "10.55.0.12:4433");

        let name_box = modal_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::TextInput { placeholder, .. }
                        if placeholder == "Optional local only name"
                )
            })
            .expect("missing modal friendly name input node");
        let name_msg = name_box
            .node
            .emit_input("RHEL Workstation")
            .expect("UpdateModalName callback missing");
        let _ = app.update(name_msg);
        assert_eq!(app.add_modal_state.name_input, "RHEL Workstation");

        let vpn_box = modal_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Dropdown { options, .. }
                        if options.iter().any(|o| o == "Tailscale")
                )
            })
            .expect("missing modal VPN dropdown node");
        let vpn_msg = vpn_box
            .node
            .emit_select(2)
            .expect("SelectModalVpn callback missing");
        let _ = app.update(vpn_msg);
        assert_eq!(
            app.add_modal_state.selected_vpn,
            VpnAdapterBinding::Tailscale
        );
    }
}
