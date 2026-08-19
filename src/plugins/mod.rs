//! Compile-time plugin boundary for optional pages and cards.
//!
//! Plugins are registered only when their Cargo feature is enabled. Core
//! configuration stays generic and each plugin owns decoding its options.

use crate::core::config::{CardConfig, PageConfig};
use crate::core::error::AppError;

#[cfg(feature = "pet-card")]
pub mod pet_card;
#[cfg(feature = "scrcpy-forge")]
pub mod scrcpy_forge;

#[derive(Clone)]
pub struct PluginContext {
    #[cfg_attr(not(feature = "scrcpy-forge"), allow(dead_code))]
    pub handle: tokio::runtime::Handle,
    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub presentation: Option<CardPresentationHandle>,
    #[cfg_attr(
        not(any(feature = "pet-card", feature = "scrcpy-forge")),
        allow(dead_code)
    )]
    pub runtime: crate::core::runtime::RuntimeHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardPresentation {
    Normal,
    Quad,
    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    Expanded,
    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    Fullscreen,
}

#[derive(Clone)]
pub struct CardPresentationHandle {
    sender: async_channel::Sender<CardPresentation>,
}

impl CardPresentationHandle {
    pub fn channel() -> (Self, async_channel::Receiver<CardPresentation>) {
        let (sender, receiver) = async_channel::unbounded();
        (Self { sender }, receiver)
    }

    pub fn request(&self, presentation: CardPresentation) {
        let _ = self.sender.try_send(presentation);
    }
}

pub trait PagePlugin {
    fn kind(&self) -> &'static str;
    fn build(
        &self,
        context: &PluginContext,
        page: &PageConfig,
        options: &toml::Value,
    ) -> Result<gtk::Widget, AppError>;
}

pub trait CardPlugin {
    fn kind(&self) -> &'static str;
    fn build(
        &self,
        context: &PluginContext,
        card: &CardConfig,
        options: &toml::Value,
    ) -> Result<gtk::Widget, AppError>;
}

/// Validate plugin-owned options without constructing GTK widgets. This is
/// used by the configuration checker and keeps plugin schema errors tied to
/// configuration rather than UI startup.
pub fn validate_config(config: &crate::core::config::AppConfig) -> Result<(), AppError> {
    for page in &config.pages {
        let Some(kind) = page.kind.as_deref() else {
            continue;
        };
        let options = page
            .plugin
            .clone()
            .unwrap_or_else(|| toml::Value::Table(Default::default()));
        match kind {
            #[cfg(feature = "scrcpy-forge")]
            "scrcpy-forge" => {
                let _: scrcpy_forge::PageConfig = options.try_into().map_err(|error| {
                    AppError::Plugin(format!("invalid scrcpy-forge config: {error}"))
                })?;
            }
            _ => {
                let _ = options;
                return Err(AppError::Unsupported(format!(
                    "page plugin `{kind}` is not available in this build"
                )));
            }
        }
    }
    for card in &config.cards {
        let Some(kind) = card.kind.as_deref() else {
            continue;
        };
        let options = card
            .plugin
            .clone()
            .unwrap_or_else(|| toml::Value::Table(Default::default()));
        match kind {
            #[cfg(feature = "pet-card")]
            "pet-card" => {
                let _: pet_card::config::PetConfig = options.try_into().map_err(|error| {
                    AppError::Plugin(format!("invalid pet-card config: {error}"))
                })?;
            }
            _ => {
                let _ = options;
                return Err(AppError::Unsupported(format!(
                    "card plugin `{kind}` is not available in this build"
                )));
            }
        }
    }
    Ok(())
}

fn page_plugins() -> Vec<Box<dyn PagePlugin>> {
    #[allow(unused_mut)]
    let mut plugins: Vec<Box<dyn PagePlugin>> = Vec::new();
    #[cfg(feature = "scrcpy-forge")]
    plugins.push(Box::new(scrcpy_forge::Plugin));
    plugins
}

fn card_plugins() -> Vec<Box<dyn CardPlugin>> {
    #[allow(unused_mut)]
    let mut plugins: Vec<Box<dyn CardPlugin>> = Vec::new();
    #[cfg(feature = "pet-card")]
    plugins.push(Box::new(pet_card::Plugin));
    plugins
}

pub fn build_page(
    context: &PluginContext,
    page: &PageConfig,
) -> Result<Option<gtk::Widget>, AppError> {
    let Some(kind) = page.kind.as_deref() else {
        return Ok(None);
    };
    let Some(plugin) = page_plugins()
        .into_iter()
        .find(|plugin| plugin.kind() == kind)
    else {
        return Err(AppError::Unsupported(format!(
            "page plugin `{kind}` is not available in this build"
        )));
    };
    let options = page
        .plugin
        .as_ref()
        .cloned()
        .unwrap_or_else(|| toml::Value::Table(Default::default()));
    plugin.build(context, page, &options).map(Some)
}

pub fn build_card(
    context: &PluginContext,
    card: &CardConfig,
) -> Result<Option<gtk::Widget>, AppError> {
    let Some(kind) = card.kind.as_deref() else {
        return Ok(None);
    };
    let Some(plugin) = card_plugins()
        .into_iter()
        .find(|plugin| plugin.kind() == kind)
    else {
        return Err(AppError::Unsupported(format!(
            "card plugin `{kind}` is not available in this build"
        )));
    };
    let options = card
        .plugin
        .clone()
        .unwrap_or_else(|| toml::Value::Table(Default::default()));
    plugin.build(context, card, &options).map(Some)
}
