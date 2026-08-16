//! Optional built-in integration for the independently distributed ScrcpyForge service.
//!
//! This entire module is excluded unless the `scrcpy-forge` Cargo feature is enabled.

pub(crate) mod config;
mod page;
mod service;

pub use config::PageConfig;

pub struct Plugin;

impl crate::plugins::PagePlugin for Plugin {
    fn kind(&self) -> &'static str {
        "scrcpy-forge"
    }

    fn build(
        &self,
        context: &crate::plugins::PluginContext,
        _page: &crate::core::config::PageConfig,
        options: &toml::Value,
    ) -> Result<gtk::Widget, crate::core::error::AppError> {
        use gtk::prelude::Cast;
        let config: PageConfig = options.clone().try_into().map_err(|error| {
            crate::core::error::AppError::Plugin(format!("invalid scrcpy-forge config: {error}"))
        })?;
        Ok(page::build(context.handle.clone(), config, context.runtime.clone()).upcast())
    }
}
