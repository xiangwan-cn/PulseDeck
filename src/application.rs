use crate::core::config::{config_dir, config_path, optional_system_cards, ConfigManager};
use crate::window::MonitorWindow;

// The initial card set lives in a real TOML file. Adding or changing a
// configuration-driven card no longer requires touching Rust source.
const DEFAULT_CONFIG: &str = include_str!("../config/config.example.toml");

pub fn build_app(app: &adw::Application) {
    let config_dir_path = config_dir();
    let _ = std::fs::create_dir_all(&config_dir_path);

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

    // Make newly added native capabilities discoverable in Settings while
    // keeping them disabled and absent from the monitor page by default.
    let mut changed = false;
    for card in optional_system_cards() {
        if !cfg
            .config()
            .cards
            .iter()
            .any(|current| current.id == card.id)
        {
            cfg.config_mut().cards.push(card);
            changed = true;
        }
    }
    #[cfg(feature = "pet-card")]
    if !cfg.config().cards.iter().any(|card| card.id == "codex-pet") {
        cfg.config_mut()
            .cards
            .push(crate::plugins::pet_card::config::default_card());
        changed = true;
    }
    #[cfg(feature = "scrcpy-forge")]
    if !cfg
        .config()
        .pages
        .iter()
        .any(|page| page.id == "scrcpy-forge")
    {
        cfg.config_mut()
            .pages
            .push(crate::plugins::scrcpy_forge::config::default_page());
        changed = true;
    }
    // A rejected schema is never rewritten as a side effect of falling back.
    // The user updates the versioned configuration explicitly.
    if changed && config_loaded {
        let _ = cfg.save();
    }

    let window = MonitorWindow::new(app, cfg);
    window.present();
}
