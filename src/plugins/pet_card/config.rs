use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::core::config::{CardConfig, CardRuntimeConfig, DisplayConfig};
use crate::model::card_model::RendererKind;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PetConfig {
    #[serde(default = "default_state_file")]
    pub state_file: PathBuf,
    #[serde(default = "default_presentation_file")]
    pub presentation_file: PathBuf,
    #[serde(default)]
    pub asset_root: Option<PathBuf>,
    #[serde(default = "default_offline_after")]
    pub offline_after_seconds: u64,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_done_hold")]
    pub done_hold_seconds: u64,
    #[serde(default = "default_offline_normal_after")]
    pub offline_normal_after_seconds: u64,
    #[serde(default = "default_true")]
    pub pause_when_unmapped: bool,
    #[serde(default = "default_true")]
    pub show_status: bool,
    #[serde(default)]
    pub completion_sound_file: Option<PathBuf>,
    #[serde(default)]
    pub animations: HashMap<String, AnimationConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnimationConfig {
    #[serde(default)]
    pub frames: Vec<PathBuf>,
    #[serde(default)]
    pub fps: Option<u32>,
    #[serde(default = "default_true")]
    pub r#loop: bool,
}

fn default_state_file() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("pulsedeck-{}", unsafe { libc::geteuid() }))
        });
    base.join("pulsedeck/codex-pet.json")
}

fn default_presentation_file() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("pulsedeck/pet-card-presentation")
}

fn default_offline_after() -> u64 {
    180
}

fn default_fps() -> u32 {
    12
}

fn default_done_hold() -> u64 {
    5
}

fn default_offline_normal_after() -> u64 {
    300
}

fn default_true() -> bool {
    true
}

impl Default for PetConfig {
    fn default() -> Self {
        Self {
            state_file: default_state_file(),
            presentation_file: default_presentation_file(),
            asset_root: None,
            offline_after_seconds: default_offline_after(),
            fps: default_fps(),
            done_hold_seconds: default_done_hold(),
            offline_normal_after_seconds: default_offline_normal_after(),
            pause_when_unmapped: true,
            show_status: true,
            completion_sound_file: None,
            animations: HashMap::new(),
        }
    }
}

/// Card written to a standalone config module when this feature is compiled
/// in. Empty plugin options select PetConfig's safe defaults.
pub(crate) fn default_card() -> CardConfig {
    CardConfig {
        id: "codex-pet".into(),
        title: "Codex Pet".into(),
        page: "monitor".into(),
        order: 5,
        renderer: RendererKind::Value,
        refresh_interval: 60,
        enabled: true,
        icon: None,
        description: Some("Codex 运行状态".into()),
        source: None,
        display: Some(DisplayConfig {
            minimum_change: None,
            columns_after: None,
            columns: None,
            card_width: None,
            card_height: Some(133),
            fixed_size: Some(true),
            colors: Default::default(),
            states: Vec::new(),
            transition: None,
        }),
        cache_ttl_seconds: None,
        schedule: None,
        click_action: None,
        kind: Some("pet-card".into()),
        plugin: Some(toml::Value::Table(Default::default())),
        runtime: CardRuntimeConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{default_card, PetConfig};

    #[test]
    fn bundled_example_is_valid() {
        let config: crate::core::config::ConfigFragment =
            toml::from_str(include_str!("config.example.toml")).unwrap();
        let card = &config.cards[0];
        assert_eq!(card.kind.as_deref(), Some("pet-card"));
        let plugin: PetConfig = card.plugin.clone().unwrap().try_into().unwrap();
        assert_eq!(plugin.fps, 12);
        assert_eq!(plugin.offline_normal_after_seconds, 300);
        assert!(plugin.completion_sound_file.is_none());
        assert!(plugin.animations.contains_key("offline"));
        assert!(plugin.animations.contains_key("done"));
    }

    #[test]
    fn compiled_feature_card_uses_self_contained_defaults() {
        let card = default_card();
        assert_eq!(card.id, "codex-pet");
        assert_eq!(card.kind.as_deref(), Some("pet-card"));
        assert!(card.enabled);
        let config: PetConfig = card.plugin.unwrap().try_into().unwrap();
        assert_eq!(config.fps, 12);
        assert!(config.asset_root.is_none());
    }
}
