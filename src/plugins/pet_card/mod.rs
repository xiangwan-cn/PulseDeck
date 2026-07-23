//! Low-power, event-driven Codex pet card.

mod config;
mod runtime;

use crate::core::error::AppError;
use crate::plugins::{CardPlugin, PluginContext};

pub struct Plugin;

impl CardPlugin for Plugin {
    fn kind(&self) -> &'static str {
        "pet-card"
    }

    fn build(
        &self,
        context: &PluginContext,
        card: &crate::core::config::CardConfig,
        options: &toml::Value,
    ) -> Result<gtk::Widget, AppError> {
        use gtk::prelude::Cast;
        let config: config::PetConfig = options
            .clone()
            .try_into()
            .map_err(|error| AppError::Plugin(format!("invalid pet-card config: {error}")))?;
        Ok(runtime::build(card, config, context.presentation.clone())?.upcast())
    }
}
