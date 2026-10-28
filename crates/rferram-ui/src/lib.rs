//! `rferram-ui`: Graphical Client View & Message Loop (v2 - Native COSMIC Desktop & Cross-Platform)
//!
//! Implements the primary `libcosmic` / `iced` Application state engine (`cosmic::Application`),
//! desktop connection card grids, "Add Desktop" setup modal (matching the visual layout mockup with
//! global Connection/Credentials inputs and General, Display, Devices | Audio, and Folders tabs),
//! settings/help context drawers, frosted glass WGSL shader, and `cosmic-config` profile persistence.

pub mod app;
pub mod drawers;
pub mod modal;
pub mod shader;

pub use app::*;
pub use drawers::*;
pub use modal::*;
pub use shader::*;

use cosmic::Application;
use rferram_common::ConnectionProfile;

/// Schema version for `rferram-ui` persistent configuration in `cosmic-config`.
pub const CONFIG_VERSION: u64 = 1;

/// Configuration key used to persist saved `ConnectionProfile` records in `cosmic-config`.
pub const PROFILES_CONFIG_KEY: &str = "connection_profiles";

/// Configuration key used to persist favorite `ConnectionProfile` IDs in `cosmic-config`.
pub const FAVORITE_PROFILES_CONFIG_KEY: &str = "favorite_profile_ids";

/// Opens the `cosmic-config` store for `RFerramApp::APP_ID` at `CONFIG_VERSION`.
pub fn open_ui_config() -> Result<cosmic_config::Config, cosmic_config::ConfigError> {
    cosmic_config::Config::new(RFerramApp::APP_ID, CONFIG_VERSION)
}

/// Persists a slice of `ConnectionProfile` entries into the provided `cosmic-config` store.
pub fn save_connection_profiles(
    config: &cosmic_config::Config,
    profiles: &[ConnectionProfile],
) -> Result<(), cosmic_config::ConfigError> {
    config.set(PROFILES_CONFIG_KEY, &profiles)
}

/// Loads saved `ConnectionProfile` entries from the provided `cosmic-config` store.
pub fn load_connection_profiles(
    config: &cosmic_config::Config,
) -> Result<Vec<ConnectionProfile>, cosmic_config::ConfigError> {
    config.get(PROFILES_CONFIG_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::app::Core;
    use std::sync::Once;

    static INIT_XDG_CONFIG_HOME: Once = Once::new();

    fn ensure_workspace_xdg_config_home() {
        INIT_XDG_CONFIG_HOME.call_once(|| {
            let workspace_test_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-cosmic-config");
            let _ = std::fs::create_dir_all(&workspace_test_root);
            // SAFETY: Executed at most once across the test process via `std::sync::Once`
            // before any `cosmic-config` store is opened, preventing concurrent `setenv` races
            // while confining config writes to the workspace `target/test-cosmic-config` directory.
            unsafe {
                std::env::set_var("XDG_CONFIG_HOME", &workspace_test_root);
            }
        });
    }

    fn create_isolated_test_config(test_suffix: &str) -> cosmic_config::Config {
        ensure_workspace_xdg_config_home();
        let app_id = format!(
            "{}.test.{}.{}",
            RFerramApp::APP_ID,
            test_suffix,
            uuid::Uuid::new_v4()
        );
        cosmic_config::Config::new(&app_id, CONFIG_VERSION)
            .expect("failed to create isolated cosmic-config instance")
    }

    fn cleanup_test_config(config: &cosmic_config::Config) {
        if let Some(app_dir) = config.base_dir().parent() {
            let _ = std::fs::remove_dir_all(app_dir);
        }
    }

    #[test]
    fn test_cosmic_config_profile_persistence_roundtrip() {
        let config = create_isolated_test_config("roundtrip");

        let default_ui_cfg = open_ui_config().expect("open_ui_config failed");
        assert_eq!(default_ui_cfg.app_id(), RFerramApp::APP_ID);
        assert_eq!(default_ui_cfg.version(), CONFIG_VERSION);

        let profiles = vec![
            ConnectionProfile {
                id: String::from("prof-alpha"),
                friendly_name: String::from("COSMIC Workstation Alpha"),
                group_name: String::from("Group 1"),
                host_address: String::from("10.10.0.12:4433"),
                vpn_binding: Some(String::from("wg0")),
                auto_reconnect: true,
                admin_session_takeover: false,
                ask_credentials_on_connect: true,
                folder_shares: vec![String::from("/srv/devshare")],
                enable_speaker_audio: true,
                enable_mic_passthrough: false,
            },
            ConnectionProfile {
                id: String::from("prof-beta"),
                friendly_name: String::from("Windows 11 Build Node"),
                group_name: String::from("Group 2"),
                host_address: String::from("100.64.1.25:4433"),
                vpn_binding: Some(String::from("tailscale0")),
                auto_reconnect: false,
                admin_session_takeover: true,
                ask_credentials_on_connect: false,
                folder_shares: Vec::new(),
                enable_speaker_audio: false,
                enable_mic_passthrough: true,
            },
        ];

        save_connection_profiles(&config, &profiles).expect("save_connection_profiles failed");
        let loaded = load_connection_profiles(&config).expect("load_connection_profiles failed");
        assert_eq!(loaded, profiles);

        cleanup_test_config(&config);
        cleanup_test_config(&default_ui_cfg);
    }

    #[test]
    fn test_app_profile_persistence_workflow() {
        let config = create_isolated_test_config("app_workflow");
        let (mut app, _) = RFerramApp::new(Core::new());

        // Add a new profile via modal messages and mark it as favorite
        let _ = app.update(Message::OpenAddDesktopModal);
        let _ = app.update(Message::UpdateModalName("Arch Wayland Node".to_string()));
        let _ = app.update(Message::UpdateModalHost("10.88.0.5:4433".to_string()));
        let _ = app.update(Message::SelectModalVpn(VpnAdapterBinding::UniFiVpn));
        let _ = app.update(Message::SaveConnectionProfile);
        assert_eq!(app.connection_cards.len(), 3);
        let new_id = app.connection_cards[2].profile.id.clone();
        let _ = app.update(Message::ToggleFavorite(new_id));
        assert!(app.connection_cards[0].is_favorite);
        assert!(!app.connection_cards[1].is_favorite);
        assert!(app.connection_cards[2].is_favorite);

        // Persist profiles from app to cosmic-config
        app.save_profiles_to_config(&config)
            .expect("save_profiles_to_config failed");

        // Create a fresh app instance, clear its default cards, and restore from cosmic-config
        let (mut restored_app, _) = RFerramApp::new(Core::new());
        restored_app.connection_cards.clear();
        let loaded = restored_app
            .load_profiles_from_config(&config)
            .expect("load_profiles_from_config failed");

        assert_eq!(loaded.len(), 3);
        assert_eq!(restored_app.connection_cards.len(), 3);
        assert!(restored_app.connection_cards[0].is_favorite);
        assert_eq!(
            restored_app.connection_cards[0].vpn_binding,
            VpnAdapterBinding::WireGuard(String::from("utun3"))
        );
        assert!(!restored_app.connection_cards[1].is_favorite);
        assert_eq!(
            restored_app.connection_cards[1].vpn_binding,
            VpnAdapterBinding::Tailscale
        );
        assert!(restored_app.connection_cards[2].is_favorite);
        assert_eq!(
            restored_app.connection_cards[2].profile.friendly_name,
            "Arch Wayland Node"
        );
        assert_eq!(
            restored_app.connection_cards[2].vpn_binding,
            VpnAdapterBinding::UniFiVpn
        );

        cleanup_test_config(&config);
    }

    #[test]
    fn test_cosmic_config_empty_and_missing_key_edge_cases() {
        let config = create_isolated_test_config("edge_cases");

        // Loading before any key is written must return ConfigError::Io (NotFound)
        let missing_res = load_connection_profiles(&config);
        assert!(missing_res.is_err());

        // Saving and loading an empty slice of profiles must round-trip cleanly
        save_connection_profiles(&config, &[]).expect("saving empty profile slice failed");
        let empty_loaded =
            load_connection_profiles(&config).expect("loading empty profile slice failed");
        assert!(empty_loaded.is_empty());

        cleanup_test_config(&config);
    }
}
