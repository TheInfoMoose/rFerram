//! Settings and Help/Diagnostics Context Drawers
//!
//! Implements `SettingsSection`, `HelpSection`, `RFerramApp::view_settings_drawer`,
//! and `RFerramApp::view_help_drawer` matching the top-right Settings menu and
//! bottom-right Help menu from the visual layout mockup.

use cosmic::Element;
use cosmic::widget::{button, column, container, divider, text};

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
}

impl RFerramApp {
    /// Settings Context Drawer (`General`, `Keyboard`, `Resolution | Scaling`, `Groups`, `Profiles`, `VPN`)
    pub fn view_settings_drawer(&self) -> Element<'_, Message> {
        container(
            "rferram-settings-drawer",
            column(
                8,
                vec![
                    text(format!(
                        "Settings: {}",
                        self.active_settings_section.label()
                    ))
                    .size(16)
                    .into(),
                    divider(),
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
                    divider(),
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
                    text(format!("Help: {}", self.active_help_section.label()))
                        .size(16)
                        .into(),
                    divider(),
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
                    divider(),
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
}
