mod application;
mod core;
mod execution;
mod metrics;
mod model;
mod plugins;
mod rendering;
mod sources;
mod ui;
mod window;

use std::sync::{LazyLock, OnceLock};

use gio::prelude::{ApplicationExt, ApplicationExtManual};
use tracing_subscriber::{fmt, prelude::*, reload, EnvFilter};

static TOKIO_RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(4)
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime")
});

static LOG_FILTER_HANDLE: OnceLock<reload::Handle<EnvFilter, tracing_subscriber::Registry>> =
    OnceLock::new();

pub fn tokio_handle() -> tokio::runtime::Handle {
    TOKIO_RT.handle().clone()
}

fn initial_log_filter() -> EnvFilter {
    if std::env::var_os("RUST_LOG").is_some() {
        return EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    }
    let path = core::config::config_path();
    let configured = if path.exists() {
        let mut manager = core::config::ConfigManager::new(path);
        manager
            .load()
            .ok()
            .map(|_| manager.config().app.log_level.clone())
    } else {
        None
    };
    configured
        .as_deref()
        .and_then(|level| EnvFilter::try_new(level).ok())
        .unwrap_or_else(|| EnvFilter::new("info"))
}

pub(crate) fn reload_log_filter(level: &str) {
    // RUST_LOG is an explicit operator override and must remain authoritative
    // across configuration reloads. The validated config value is still
    // useful as the default when the environment is absent.
    if std::env::var_os("RUST_LOG").is_some() {
        return;
    }
    let Ok(filter) = EnvFilter::try_new(level) else {
        tracing::warn!(
            configured = level,
            "ignored invalid log filter after reload"
        );
        return;
    };
    let Some(handle) = LOG_FILTER_HANDLE.get() else {
        return;
    };
    if let Err(error) = handle.reload(filter) {
        tracing::warn!(%error, "failed to reload log filter");
    }
}

fn main() -> glib::ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(exit_code) = core::config_cli::run_if_requested(&arguments) {
        return exit_code;
    }

    let (log_filter, log_filter_handle) = reload::Layer::new(initial_log_filter());
    let _ = LOG_FILTER_HANDLE.set(log_filter_handle);
    tracing_subscriber::registry()
        .with(log_filter)
        .with(fmt::layer())
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        git_commit = env!("PULSEDECK_GIT_COMMIT"),
        git_dirty = env!("PULSEDECK_GIT_DIRTY"),
        build_id = env!("PULSEDECK_BUILD_ID"),
        target = env!("PULSEDECK_BUILD_TARGET"),
        profile = env!("PULSEDECK_BUILD_PROFILE"),
        features = env!("PULSEDECK_FEATURES"),
        rustc = env!("PULSEDECK_BUILD_RUSTC"),
        gtk = env!("PULSEDECK_BUILD_GTK"),
        libadwaita = env!("PULSEDECK_BUILD_ADWAITA"),
        binary = ?std::env::current_exe().ok(),
        "PulseDeck starting"
    );

    let _ = &*TOKIO_RT;

    let app = adw::Application::new(
        Some("io.github.pulsedeck.PulseDeck"),
        gio::ApplicationFlags::default(),
    );

    app.connect_activate(|app| {
        application::build_app(app);
    });
    app.connect_shutdown(application::release_app);

    app.run()
}
