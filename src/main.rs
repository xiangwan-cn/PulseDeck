mod actions;
mod application;
mod core;
mod execution;
mod metrics;
mod model;
mod parsers;
mod plugins;
mod rendering;
mod services;
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
