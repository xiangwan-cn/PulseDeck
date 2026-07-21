//! Optional built-in integration for the independently distributed ScrcpyForge service.
//!
//! This entire module is excluded unless the `scrcpy-forge` Cargo feature is enabled.

mod config;
mod page;
mod service;

pub use config::PageConfig;

pub fn accepts(kind: Option<&str>) -> bool {
    kind == Some("scrcpy-forge")
}

pub fn build(
    handle: tokio::runtime::Handle,
    page_config: &crate::core::config::PageConfig,
) -> gtk::ScrolledWindow {
    page::build(handle, page_config)
}
