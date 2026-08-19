mod application;
mod core;
mod execution;
mod metrics;
mod model;
mod parsers;
mod plugins;
mod rendering;
mod sources;
mod ui;
mod window;

use std::sync::LazyLock;

use gio::prelude::{ApplicationExt, ApplicationExtManual};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

static TOKIO_RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(4)
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime")
});

pub fn tokio_handle() -> tokio::runtime::Handle {
    TOKIO_RT.handle().clone()
}

fn main() -> glib::ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new("--check-config")) {
        let path = arguments
            .next()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(core::config::config_path);
        if arguments.next().is_some() {
            eprintln!("usage: pulsedeck --check-config [CONFIG_FILE]");
            return glib::ExitCode::FAILURE;
        }
        let mut manager = core::config::ConfigManager::new(path.clone());
        return match manager
            .load()
            .and_then(|()| plugins::validate_config(manager.config()))
        {
            Ok(()) => {
                println!(
                    "configuration valid: {} modules, {} pages, {} cards, {} actions",
                    manager.loaded_module_count(),
                    manager.config().pages.len(),
                    manager.config().cards.len(),
                    manager.config().actions.len()
                );
                glib::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("configuration invalid at {}: {error}", path.display());
                glib::ExitCode::FAILURE
            }
        };
    }

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let _ = &*TOKIO_RT;

    let app = adw::Application::new(
        Some("io.github.pulsedeck.PulseDeck"),
        gio::ApplicationFlags::default(),
    );

    app.connect_activate(|app| {
        application::build_app(app);
    });

    app.run()
}
