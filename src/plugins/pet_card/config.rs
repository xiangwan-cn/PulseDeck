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
    /// Deprecated v4 compatibility field. B2 always hard-stops animation when
    /// the card is unmapped, regardless of this decoded value.
    #[allow(dead_code)]
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

impl PetConfig {
    pub(crate) fn validate(&self) -> Result<(), String> {
        const MAX_ANIMATIONS: usize = 32;
        const MAX_FRAMES_PER_ANIMATION: usize = 256;
        const MAX_PATH_LENGTH: usize = 4096;

        if self.fps == 0 || self.fps > 12 {
            return Err("fps must be between 1 and 12".into());
        }
        if self.offline_after_seconds == 0 || self.offline_after_seconds > 7 * 24 * 60 * 60 {
            return Err("offline_after_seconds must be between 1 and 604800".into());
        }
        if self.done_hold_seconds > 60 * 60 {
            return Err("done_hold_seconds must be at most 3600".into());
        }
        if self.offline_normal_after_seconds < self.offline_after_seconds
            || self.offline_normal_after_seconds > 30 * 24 * 60 * 60
        {
            return Err(
                "offline_normal_after_seconds must be >= offline_after_seconds and at most 2592000"
                    .into(),
            );
        }
        if self.animations.len() > MAX_ANIMATIONS {
            return Err(format!(
                "animations cannot contain more than {MAX_ANIMATIONS} states"
            ));
        }
        for (state, animation) in &self.animations {
            if state.trim().is_empty() || state.len() > 128 {
                return Err("animation state names must be 1..=128 bytes".into());
            }
            if animation.frames.len() > MAX_FRAMES_PER_ANIMATION {
                return Err(format!(
                    "animation `{state}` cannot contain more than {MAX_FRAMES_PER_ANIMATION} frames"
                ));
            }
            if animation.fps.is_some_and(|fps| fps == 0 || fps > self.fps) {
                return Err(format!(
                    "animation `{state}` fps must be between 1 and the global fps"
                ));
            }
            for frame in &animation.frames {
                if frame.as_os_str().to_string_lossy().len() > MAX_PATH_LENGTH {
                    return Err(format!(
                        "animation `{state}` contains an excessively long path"
                    ));
                }
            }
        }
        for (name, path) in [
            ("state_file", &self.state_file),
            ("presentation_file", &self.presentation_file),
        ] {
            if path.as_os_str().to_string_lossy().len() > MAX_PATH_LENGTH {
                return Err(format!("{name} path is too long"));
            }
        }
        if self
            .asset_root
            .as_ref()
            .is_some_and(|path| path.as_os_str().to_string_lossy().len() > MAX_PATH_LENGTH)
        {
            return Err("asset_root path is too long".into());
        }
        if self
            .completion_sound_file
            .as_ref()
            .is_some_and(|path| path.as_os_str().to_string_lossy().len() > MAX_PATH_LENGTH)
        {
            return Err("completion_sound_file path is too long".into());
        }
        Ok(())
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
            logo_svg: None,
            background_svg: None,
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
