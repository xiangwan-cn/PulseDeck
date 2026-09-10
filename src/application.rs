use std::rc::Rc;

use glib::prelude::ObjectExt;

use crate::core::config::{
    config_dir, config_modules_dir, config_path, optional_system_cards, ConfigFragment,
    ConfigManager,
};
use crate::window::MonitorWindow;

// The initial card set lives in a real TOML file. Adding or changing a
// configuration-driven card no longer requires touching Rust source.
const DEFAULT_CONFIG: &str = include_str!("../config/config.example.toml");
const MONITOR_WINDOW_DATA_KEY: &str = "pulsedeck-monitor-window";

fn generated_module_disabled(file_name: &str) -> bool {
    generated_module_disabled_in(&config_modules_dir(), file_name)
}

fn generated_module_disabled_in(directory: &std::path::Path, file_name: &str) -> bool {
    let mut disabled_name = std::ffi::OsString::from(file_name);
    disabled_name.push(".disabled");
    directory.join(disabled_name).exists()
}

pub fn build_app(app: &adw::Application) {
    // Activation can be emitted more than once for a single application. The
    // controller owns the scheduler and all event subscriptions, so reusing
    // it is the lifecycle boundary that prevents duplicate async loops.
    unsafe {
        if let Some(window) = app.data::<Rc<MonitorWindow>>(MONITOR_WINDOW_DATA_KEY) {
            window.as_ref().present();
            return;
        }
    }

    let config_dir_path = config_dir();
    let _ = std::fs::create_dir_all(&config_dir_path);
    let _ = std::fs::create_dir_all(config_modules_dir());

    let config_file = config_path();
    if !config_file.exists() {
        if let Err(e) = std::fs::write(&config_file, DEFAULT_CONFIG) {
            tracing::error!("failed to write default config: {}", e);
        } else {
            tracing::info!("wrote default config to {:?}", config_file);
        }
    }

    let mut cfg = ConfigManager::new(config_file);

    let config_loaded = match cfg.load() {
        Ok(()) => {
            tracing::info!("config loaded from {:?}", cfg.path());
            true
        }
        Err(e) => {
            tracing::warn!("config load failed: {}, using defaults", e);
            false
        }
    };

    if config_loaded {
        // Keep generated native capabilities out of the default main file.
        let optional_cards: Vec<_> = optional_system_cards()
            .into_iter()
            .filter(|card| {
                !cfg.config()
                    .cards
                    .iter()
                    .any(|current| current.id == card.id)
            })
            .collect();
        if !optional_cards.is_empty() && !generated_module_disabled("70-system-cards.toml") {
            let fragment = ConfigFragment::with_cards(optional_cards);
            if let Err(error) = cfg.ensure_module("70-system-cards.toml", fragment) {
                tracing::warn!(%error, "failed to create optional system-card module");
            }
        }

        // Optional plugins own standalone modules. This keeps their verbose
        // settings out of config.toml while retaining ready-to-use defaults.
        #[cfg(feature = "pet-card")]
        if !generated_module_disabled("80-pet-card.toml")
            && !cfg.config().cards.iter().any(|card| card.id == "codex-pet")
        {
            let fragment =
                ConfigFragment::with_card(crate::plugins::pet_card::config::default_card());
            if let Err(error) = cfg.ensure_module("80-pet-card.toml", fragment) {
                tracing::warn!(%error, "failed to create PetCard config module");
            }
        }
        #[cfg(feature = "scrcpy-forge")]
        if !generated_module_disabled("90-scrcpy-forge.toml")
            && !cfg
                .config()
                .pages
                .iter()
                .any(|page| page.id == "scrcpy-forge")
        {
            let fragment =
                ConfigFragment::with_page(crate::plugins::scrcpy_forge::config::default_page());
            if let Err(error) = cfg.ensure_module("90-scrcpy-forge.toml", fragment) {
                tracing::warn!(%error, "failed to create ScrcpyForge config module");
            }
        }
    }

    let window = Rc::new(MonitorWindow::new(app, cfg));
    window.present();
    // MonitorWindow owns file/network/power subscriptions and fallback timers.
    // Keep that controller alive for the application lifetime rather than
    // dropping it when this activation callback returns.
    unsafe {
        app.set_data(MONITOR_WINDOW_DATA_KEY, window);
    }
}

pub fn release_app(app: &adw::Application) {
    // Stealing the retained controller before application teardown runs its
    // Drop implementation and removes the remaining GLib sources.
    unsafe {
        let _: Option<Rc<MonitorWindow>> = app.steal_data(MONITOR_WINDOW_DATA_KEY);
    }
}

#[cfg(test)]
mod tests {
    use super::generated_module_disabled_in;

    #[test]
    fn disabled_generated_module_is_an_explicit_opt_out() {
        let directory =
            std::env::temp_dir().join(format!("pulsedeck-disabled-module-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("80-plugin.toml.disabled"), "").unwrap();

        assert!(generated_module_disabled_in(&directory, "80-plugin.toml"));
        assert!(!generated_module_disabled_in(&directory, "90-other.toml"));

        let _ = std::fs::remove_dir_all(directory);
    }
}
