//! Settings and Help/Diagnostics Context Drawers
//!
//! Implements `SettingsSection`, `HelpSection`, `RFerramApp::view_settings_drawer`,
//! and `RFerramApp::view_help_drawer` matching the top-right Settings menu and
//! bottom-right Help menu from the visual layout mockup.

use cosmic::Element;
use cosmic::widget::{button, column, container, divider, nav_button, row, text};

use crate::{Message, RFerramApp};

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

impl SettingsSection {
    /// Human-readable label matching the Settings drawer button text.
    pub fn label(&self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Keyboard => "Keyboard",
            Self::ResolutionScaling => "Resolution | Scaling",
            Self::Groups => "Groups",
            Self::Profiles => "Profiles",
            Self::Vpn => "VPN",
        }
    }

    /// Freedesktop / COSMIC symbolic icon name matching `UI_Fix.md` and `rFerramLayout-PNG.drawio.png`.
    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::General => "preferences-system-symbolic",
            Self::Keyboard => "input-keyboard-symbolic",
            Self::ResolutionScaling => "video-display-symbolic",
            Self::Groups => "user-bookmarks-symbolic",
            Self::Profiles => "avatar-default-symbolic",
            Self::Vpn => "network-vpn-symbolic",
        }
    }
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

impl HelpSection {
    /// Human-readable label matching the Help drawer button text.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Documentation => "Documentation",
            Self::Troubleshooting => "Troubleshooting",
            Self::ReportBug => "Report Bug",
            Self::FeatureRequest => "Feature Request",
        }
    }

    /// Freedesktop / COSMIC symbolic icon name matching `UI_Fix.md` and `rFerramLayout-PNG.drawio.png`.
    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Documentation => "document-open-symbolic",
            Self::Troubleshooting => "tools-symbolic",
            Self::ReportBug => "dialog-warning-symbolic",
            Self::FeatureRequest => "lab-symbolic",
        }
    }
}

impl RFerramApp {
    /// Settings Context Drawer (`General`, `Keyboard`, `Resolution | Scaling`, `Groups`, `Profiles`, `VPN`)
    /// rendered as a two-pane split overlay (`rferram-drawer-nav` on the left with symbolic icons and
    /// `rferram-drawer-content` on the right).
    pub fn view_settings_drawer(&self) -> Element<'_, Message> {
        let nav_items = [
            SettingsSection::General,
            SettingsSection::Keyboard,
            SettingsSection::ResolutionScaling,
            SettingsSection::Groups,
            SettingsSection::Profiles,
            SettingsSection::Vpn,
        ]
        .into_iter()
        .map(|sec| {
            nav_button(sec.label())
                .icon(sec.icon_name())
                .active(self.active_settings_section == sec)
                .on_press(Message::SelectSettingsSection(sec))
                .into()
        })
        .collect();

        let nav_pane = container(cosmic::CLASS_DRAWER_NAV, column(6, nav_items));

        let detail_pane = container(
            cosmic::CLASS_DRAWER_CONTENT,
            column(
                10,
                vec![
                    text(format!(
                        "Settings: {}",
                        self.active_settings_section.label()
                    ))
                    .size(16)
                    .into(),
                    divider(),
                    button("Close")
                        .on_press(Message::CloseSettingsDrawer)
                        .into(),
                ],
            ),
        );

        container(
            cosmic::CLASS_SETTINGS_DRAWER,
            row(12, vec![nav_pane, detail_pane]),
        )
    }

    /// Help / Diagnostics Drawer (`Documentation`, `Troubleshooting`, `Report Bug`, `Feature Request`)
    /// rendered as a two-pane split overlay (`rferram-drawer-nav` on the left with symbolic icons and
    /// `rferram-drawer-content` on the right).
    pub fn view_help_drawer(&self) -> Element<'_, Message> {
        let nav_items = [
            HelpSection::Documentation,
            HelpSection::Troubleshooting,
            HelpSection::ReportBug,
            HelpSection::FeatureRequest,
        ]
        .into_iter()
        .map(|sec| {
            nav_button(sec.label())
                .icon(sec.icon_name())
                .active(self.active_help_section == sec)
                .on_press(Message::SelectHelpSection(sec))
                .into()
        })
        .collect();

        let nav_pane = container(cosmic::CLASS_DRAWER_NAV, column(6, nav_items));

        let detail_pane = container(
            cosmic::CLASS_DRAWER_CONTENT,
            column(
                10,
                vec![
                    text(format!("Help: {}", self.active_help_section.label()))
                        .size(16)
                        .into(),
                    divider(),
                    button("Close")
                        .on_press(Message::CloseDiagnosticsDrawer)
                        .into(),
                ],
            ),
        );

        container(
            cosmic::CLASS_HELP_DRAWER,
            row(12, vec![nav_pane, detail_pane]),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::Application;
    use cosmic::app::Core;

    #[test]
    fn test_settings_drawer_navigation_and_visibility() {
        let (mut app, _) = RFerramApp::new(Core::new());
        assert!(!app.settings_drawer_open);
        assert_eq!(app.active_settings_section, SettingsSection::General);

        // Root view should not render the settings drawer when closed
        let closed_labels = Application::view(&app).collect_text();
        assert!(!closed_labels.iter().any(|t| t.starts_with("Settings: ")));

        // Open settings drawer
        let _ = app.update(Message::OpenSettingsDrawer);
        assert!(app.settings_drawer_open);

        let open_labels = Application::view(&app).collect_text();
        assert!(open_labels.iter().any(|t| t == "Settings: General"));
        assert!(open_labels.iter().any(|t| t == "General"));
        assert!(open_labels.iter().any(|t| t == "Keyboard"));
        assert!(open_labels.iter().any(|t| t == "Resolution | Scaling"));
        assert!(open_labels.iter().any(|t| t == "Groups"));
        assert!(open_labels.iter().any(|t| t == "Profiles"));
        assert!(open_labels.iter().any(|t| t == "VPN"));
        assert!(open_labels.iter().any(|t| t == "Close"));

        // Navigate across all SettingsSection variants
        let sections = [
            (SettingsSection::Keyboard, "Settings: Keyboard"),
            (
                SettingsSection::ResolutionScaling,
                "Settings: Resolution | Scaling",
            ),
            (SettingsSection::Groups, "Settings: Groups"),
            (SettingsSection::Profiles, "Settings: Profiles"),
            (SettingsSection::Vpn, "Settings: VPN"),
            (SettingsSection::General, "Settings: General"),
        ];
        for (section, expected_header) in sections {
            let _ = app.update(Message::SelectSettingsSection(section));
            assert_eq!(app.active_settings_section, section);
            let drawer_labels = app.view_settings_drawer().collect_text();
            assert!(drawer_labels.iter().any(|t| t == expected_header));
        }

        // Close settings drawer
        let _ = app.update(Message::CloseSettingsDrawer);
        assert!(!app.settings_drawer_open);
        let after_close_labels = Application::view(&app).collect_text();
        assert!(
            !after_close_labels
                .iter()
                .any(|t| t.starts_with("Settings: "))
        );
    }

    #[test]
    fn test_help_drawer_navigation_and_visibility() {
        let (mut app, _) = RFerramApp::new(Core::new());
        assert!(!app.diagnostics_drawer_open);
        assert_eq!(app.active_help_section, HelpSection::Documentation);

        // Root view should not render the help drawer when closed
        let closed_labels = Application::view(&app).collect_text();
        assert!(!closed_labels.iter().any(|t| t.starts_with("Help: ")));

        // Open help/diagnostics drawer
        let _ = app.update(Message::OpenDiagnosticsDrawer);
        assert!(app.diagnostics_drawer_open);

        let open_labels = Application::view(&app).collect_text();
        assert!(open_labels.iter().any(|t| t == "Help: Documentation"));
        assert!(open_labels.iter().any(|t| t == "Documentation"));
        assert!(open_labels.iter().any(|t| t == "Troubleshooting"));
        assert!(open_labels.iter().any(|t| t == "Report Bug"));
        assert!(open_labels.iter().any(|t| t == "Feature Request"));
        assert!(open_labels.iter().any(|t| t == "Close"));

        // Navigate across all HelpSection variants
        let sections = [
            (HelpSection::Troubleshooting, "Help: Troubleshooting"),
            (HelpSection::ReportBug, "Help: Report Bug"),
            (HelpSection::FeatureRequest, "Help: Feature Request"),
            (HelpSection::Documentation, "Help: Documentation"),
        ];
        for (section, expected_header) in sections {
            let _ = app.update(Message::SelectHelpSection(section));
            assert_eq!(app.active_help_section, section);
            let drawer_labels = app.view_help_drawer().collect_text();
            assert!(drawer_labels.iter().any(|t| t == expected_header));
        }

        // Close help/diagnostics drawer
        let _ = app.update(Message::CloseDiagnosticsDrawer);
        assert!(!app.diagnostics_drawer_open);
        let after_close_labels = Application::view(&app).collect_text();
        assert!(!after_close_labels.iter().any(|t| t.starts_with("Help: ")));
    }

    #[test]
    fn test_settings_and_help_drawers_two_pane_split_and_symbolic_icons() {
        let (mut app, _) = RFerramApp::new(Core::new());

        // 1. Verify Settings Drawer two-pane split layout and symbolic icon mapping
        let _ = app.update(Message::OpenSettingsDrawer);
        let settings_view = app.view_settings_drawer();
        let settings_texts = settings_view.collect_text();

        let expected_settings_icons = [
            (
                SettingsSection::General,
                "General",
                "preferences-system-symbolic",
            ),
            (
                SettingsSection::Keyboard,
                "Keyboard",
                "input-keyboard-symbolic",
            ),
            (
                SettingsSection::ResolutionScaling,
                "Resolution | Scaling",
                "video-display-symbolic",
            ),
            (SettingsSection::Groups, "Groups", "user-bookmarks-symbolic"),
            (
                SettingsSection::Profiles,
                "Profiles",
                "avatar-default-symbolic",
            ),
            (SettingsSection::Vpn, "VPN", "network-vpn-symbolic"),
        ];

        for (sec, label, icon_name) in expected_settings_icons {
            assert_eq!(sec.label(), label);
            assert_eq!(sec.icon_name(), icon_name);
            assert!(
                settings_texts.iter().any(|t| t == label),
                "expected Settings nav label {label:?} in view_settings_drawer()"
            );
            assert!(
                settings_texts.iter().any(|t| t == icon_name),
                "expected Settings symbolic icon {icon_name:?} in view_settings_drawer()"
            );
        }

        let settings_scene = settings_view.layout_scene(1440.0, 900.0);
        let settings_nav = settings_scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_DRAWER_NAV))
            .expect("missing rferram-drawer-nav in view_settings_drawer");
        let settings_content = settings_scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_DRAWER_CONTENT))
            .expect("missing rferram-drawer-content in view_settings_drawer");
        assert!(
            settings_content.x >= settings_nav.x + settings_nav.width,
            "rferram-drawer-content (x={}) must sit to the right of rferram-drawer-nav (right={})",
            settings_content.x,
            settings_nav.x + settings_nav.width
        );
        assert!(
            settings_content.width > settings_nav.width,
            "rferram-drawer-content (w={}) must expand wider than rferram-drawer-nav (w={})",
            settings_content.width,
            settings_nav.width
        );

        let general_nav_btn = settings_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Button {
                        label,
                        icon_name: Some(icon),
                        style: cosmic::ButtonStyle::NavRow,
                        is_active: true,
                        ..
                    } if label == "General" && icon == "preferences-system-symbolic"
                )
            })
            .expect("missing active General NavRow button in view_settings_drawer");
        assert!(general_nav_btn.x >= settings_nav.x);

        // 2. Verify Help Drawer two-pane split layout and symbolic icon mapping
        let _ = app.update(Message::OpenDiagnosticsDrawer);
        let help_view = app.view_help_drawer();
        let help_texts = help_view.collect_text();

        let expected_help_icons = [
            (
                HelpSection::Documentation,
                "Documentation",
                "document-open-symbolic",
            ),
            (
                HelpSection::Troubleshooting,
                "Troubleshooting",
                "tools-symbolic",
            ),
            (
                HelpSection::ReportBug,
                "Report Bug",
                "dialog-warning-symbolic",
            ),
            (
                HelpSection::FeatureRequest,
                "Feature Request",
                "lab-symbolic",
            ),
        ];

        for (sec, label, icon_name) in expected_help_icons {
            assert_eq!(sec.label(), label);
            assert_eq!(sec.icon_name(), icon_name);
            assert!(
                help_texts.iter().any(|t| t == label),
                "expected Help nav label {label:?} in view_help_drawer()"
            );
            assert!(
                help_texts.iter().any(|t| t == icon_name),
                "expected Help symbolic icon {icon_name:?} in view_help_drawer()"
            );
        }

        let help_scene = help_view.layout_scene(1440.0, 900.0);
        let help_nav = help_scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_DRAWER_NAV))
            .expect("missing rferram-drawer-nav in view_help_drawer");
        let help_content = help_scene
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(cosmic::CLASS_DRAWER_CONTENT))
            .expect("missing rferram-drawer-content in view_help_drawer");
        assert!(
            help_content.x >= help_nav.x + help_nav.width,
            "rferram-drawer-content must sit to the right of rferram-drawer-nav in view_help_drawer"
        );
        assert!(
            help_content.width > help_nav.width,
            "rferram-drawer-content must expand wider than rferram-drawer-nav in view_help_drawer"
        );

        let doc_nav_btn = help_scene
            .boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    cosmic::WidgetNode::Button {
                        label,
                        icon_name: Some(icon),
                        style: cosmic::ButtonStyle::NavRow,
                        is_active: true,
                        ..
                    } if label == "Documentation" && icon == "document-open-symbolic"
                )
            })
            .expect("missing active Documentation NavRow button in view_help_drawer");
        assert!(doc_nav_btn.x >= help_nav.x);
    }
}
