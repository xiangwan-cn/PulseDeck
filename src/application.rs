use crate::core::config::{
    config_dir, config_modules_dir, config_path, optional_system_cards, ConfigFragment,
    ConfigManager,
};
use crate::window::MonitorWindow;

// The initial card set lives in a real TOML file. Adding or changing a
// configuration-driven card no longer requires touching Rust source.
const DEFAULT_CONFIG: &str = include_str!("../config/config.example.toml");

pub fn build_app(app: &adw::Application) {
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
        if !optional_cards.is_empty() {
            let fragment = ConfigFragment::with_cards(optional_cards);
            if let Err(error) = cfg.ensure_module("70-system-cards.toml", fragment) {
                tracing::warn!(%error, "failed to create optional system-card module");
            }
        }

        // Optional plugins own standalone modules. This keeps their verbose
        // settings out of config.toml while retaining ready-to-use defaults.
        #[cfg(feature = "pet-card")]
        if !cfg.config().cards.iter().any(|card| card.id == "codex-pet") {
            let fragment =
                ConfigFragment::with_card(crate::plugins::pet_card::config::default_card());
            if let Err(error) = cfg.ensure_module("80-pet-card.toml", fragment) {
                tracing::warn!(%error, "failed to create PetCard config module");
            }
        }
        #[cfg(feature = "scrcpy-forge")]
        if !cfg
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

    let window = MonitorWindow::new(app, cfg);
    window.present();
}
