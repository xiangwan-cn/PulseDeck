use serde::{Deserialize, Serialize};

use crate::core::error::AppError;
use crate::model::card_model::RendererKind;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub app: AppSection,
    #[serde(default)]
    pub ui: UiSection,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub pages: Vec<PageConfig>,
    #[serde(default)]
    pub cards: Vec<CardConfig>,
    #[serde(default)]
    pub actions: Vec<ActionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_true")]
    pub keep_screen_on: bool,
    #[serde(default = "default_true")]
    pub idle_power_saving: bool,
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_seconds: u64,
    #[serde(default = "default_idle_stability")]
    pub idle_stability_seconds: u64,
    #[serde(default = "default_idle_brightness")]
    pub idle_visual_brightness_percent: u8,
    #[serde(default = "default_refresh_saving")]
    pub refresh_saving_strength: String,
    #[serde(default = "default_true")]
    pub external_realtime: bool,
    #[serde(default = "default_true")]
    pub external_prevents_idle: bool,
    #[serde(default = "default_power_sample_seconds")]
    pub external_sample_seconds: u64,
    #[serde(default = "default_power_enter_samples")]
    pub external_enter_samples: u32,
    #[serde(default = "default_power_exit_samples")]
    pub external_exit_samples: u32,
    #[serde(default = "default_true")]
    pub codex_keep_bright: bool,
    #[serde(default = "default_codex_protection_minutes")]
    pub codex_protection_minutes: u64,
    #[serde(default = "default_codex_attention_seconds")]
    pub codex_attention_seconds: u64,
    #[serde(default = "default_true")]
    pub codex_completion_sound: bool,
    #[serde(default)]
    pub bring_to_foreground_on_attention: bool,
    #[serde(default)]
    pub cpu_activity_hint: bool,
    #[serde(default = "default_idle_display")]
    pub idle_display: String,
}

fn default_idle_timeout() -> u64 {
    60
}
fn default_idle_stability() -> u64 {
    10
}
fn default_idle_brightness() -> u8 {
    15
}
fn default_refresh_saving() -> String {
    "balanced".into()
}
fn default_power_sample_seconds() -> u64 {
    10
}
fn default_power_enter_samples() -> u32 {
    3
}
fn default_power_exit_samples() -> u32 {
    2
}
fn default_codex_protection_minutes() -> u64 {
    60
}
fn default_codex_attention_seconds() -> u64 {
    15
}
fn default_idle_display() -> String {
    "dim".into()
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            keep_screen_on: true,
            idle_power_saving: true,
            idle_timeout_seconds: default_idle_timeout(),
            idle_stability_seconds: default_idle_stability(),
            idle_visual_brightness_percent: default_idle_brightness(),
            refresh_saving_strength: default_refresh_saving(),
            external_realtime: true,
            external_prevents_idle: true,
            external_sample_seconds: default_power_sample_seconds(),
            external_enter_samples: default_power_enter_samples(),
            external_exit_samples: default_power_exit_samples(),
            codex_keep_bright: true,
            codex_protection_minutes: default_codex_protection_minutes(),
            codex_attention_seconds: default_codex_attention_seconds(),
            codex_completion_sound: true,
            bring_to_foreground_on_attention: false,
            cpu_activity_hint: false,
            idle_display: default_idle_display(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSection {
    #[serde(default = "default_title")]
    pub title: String,
    #[serde(default = "default_true")]
    pub reload_on_change: bool,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_max_output")]
    pub max_output_bytes: usize,
}

fn default_title() -> String {
    "PulseDeck".into()
}
fn default_true() -> bool {
    true
}
fn default_log_level() -> String {
    "info".into()
}
fn default_max_output() -> usize {
    20000
}

impl Default for AppSection {
    fn default() -> Self {
        Self {
            title: default_title(),
            reload_on_change: default_true(),
            log_level: default_log_level(),
            max_output_bytes: default_max_output(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSection {
    #[serde(default = "default_page")]
    pub default_page: String,
    #[serde(default = "default_true")]
    pub compact: bool,
    /// Number of cards in each row on regular pages.
    #[serde(default = "default_card_columns")]
    pub card_columns: u32,
    /// Optional minimum card width. When omitted the flow box shares the row.
    #[serde(default)]
    pub card_width: Option<i32>,
    /// Card height used by the default 3 x 3 landscape layout.
    #[serde(default = "default_card_height")]
    pub card_height: i32,
    /// Keep content inside the configured height instead of growing with text.
    #[serde(default = "default_true")]
    pub fixed_card_size: bool,
}

fn default_page() -> String {
    "monitor".into()
}
fn default_card_columns() -> u32 {
    3
}
fn default_card_height() -> i32 {
    133
}

impl Default for UiSection {
    fn default() -> Self {
        Self {
            default_page: default_page(),
            compact: true,
            card_columns: default_card_columns(),
            card_width: None,
            card_height: default_card_height(),
            fixed_card_size: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageConfig {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub order: i32,
    /// 页面实现类型；省略时为普通指标页。
    #[serde(default)]
    pub kind: Option<String>,
    /// Generic configuration owned and decoded by the selected page plugin.
    #[serde(default)]
    pub plugin: Option<toml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardConfig {
    pub id: String,
    pub title: String,
    pub page: String,
    #[serde(default)]
    pub order: i32,
    #[serde(default = "default_renderer")]
    pub renderer: RendererKind,
    #[serde(default = "default_interval")]
    pub refresh_interval: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub source: Option<SourceConfig>,
    #[serde(default)]
    pub display: Option<DisplayConfig>,
    #[serde(default)]
    pub cache_ttl_seconds: Option<u64>,
    #[serde(default)]
    pub schedule: Option<String>,
    /// Optional action id executed when this standard card is clicked.
    #[serde(default)]
    pub click_action: Option<String>,
    /// Optional custom card implementation. Standard metric cards omit this.
    #[serde(default)]
    pub kind: Option<String>,
    /// Generic configuration owned and decoded by the selected card plugin.
    #[serde(default)]
    pub plugin: Option<toml::Value>,
    #[serde(default)]
    pub runtime: CardRuntimeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardRuntimeConfig {
    #[serde(default = "default_auto")]
    pub class: String,
    #[serde(default = "default_throttle")]
    pub idle_behavior: String,
    #[serde(default)]
    pub idle_multiplier: Option<f64>,
    #[serde(default)]
    pub external_realtime: Option<bool>,
    #[serde(default)]
    pub realtime_multiplier: Option<f64>,
    #[serde(default)]
    pub minimum_interval_seconds: Option<u64>,
}

fn default_auto() -> String {
    "auto".into()
}
fn default_throttle() -> String {
    "throttle".into()
}

impl Default for CardRuntimeConfig {
    fn default() -> Self {
        Self {
            class: default_auto(),
            idle_behavior: default_throttle(),
            idle_multiplier: None,
            external_realtime: None,
            realtime_multiplier: None,
            minimum_interval_seconds: None,
        }
    }
}

fn default_renderer() -> RendererKind {
    RendererKind::Value
}
fn default_interval() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(default)]
    pub metric: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub program: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_max_output")]
    pub max_output_bytes: usize,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub shell: Option<bool>,
    #[serde(default)]
    pub options: Option<toml::Value>,
    #[serde(default)]
    pub parser: Option<ParserConfig>,
}

fn default_timeout() -> u64 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParserConfig {
    #[serde(rename = "type")]
    pub parser_type: String,
    #[serde(default)]
    pub divisor: Option<f64>,
    #[serde(default)]
    pub multiplier: Option<f64>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub capture: Option<usize>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub suffix: Option<String>,
    #[serde(default)]
    pub decimal_places: Option<u8>,
    #[serde(default)]
    pub as_percentage: Option<bool>,
    #[serde(default)]
    pub steps: Option<Vec<ParserConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    #[serde(default)]
    pub minimum_change: Option<f64>,
    /// 列表项目超过此数量时切换为多列。
    #[serde(default)]
    pub columns_after: Option<usize>,
    /// 列表多列模式的列数，默认 2。
    #[serde(default)]
    pub columns: Option<usize>,
    /// Per-card width override in logical pixels.
    #[serde(default)]
    pub card_width: Option<i32>,
    /// Per-card height override in logical pixels.
    #[serde(default)]
    pub card_height: Option<i32>,
    /// Per-card override for fixed versus content-driven height.
    #[serde(default)]
    pub fixed_size: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    pub page: String,
    /// Whether to render this action on its configured page.
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default)]
    pub confirm: bool,
    /// Optional confirmation dialog heading.
    #[serde(default)]
    pub confirm_title: Option<String>,
    /// Optional confirmation dialog explanation.
    #[serde(default)]
    pub confirm_detail: Option<String>,
    #[serde(default)]
    pub max_output_bytes: Option<usize>,
}

pub struct ConfigManager {
    path: std::path::PathBuf,
    config: AppConfig,
}

impl ConfigManager {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self {
            path,
            config: AppConfig::default(),
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut AppConfig {
        &mut self.config
    }

    pub fn load(&mut self) -> Result<(), AppError> {
        if !self.path.exists() {
            return Err(AppError::ConfigNotFound(self.path.clone()));
        }

        let content = std::fs::read_to_string(&self.path).map_err(|e| AppError::ConfigParse {
            path: self.path.clone(),
            message: e.to_string(),
        })?;

        if self.path.extension().map_or(false, |e| e == "json") {
            self.config = serde_json::from_str(&content).map_err(|e| AppError::ConfigParse {
                path: self.path.clone(),
                message: e.to_string(),
            })?;
        } else {
            self.config = toml::from_str(&content).map_err(|e| AppError::ConfigParse {
                path: self.path.clone(),
                message: e.to_string(),
            })?;
        }

        Ok(())
    }

    pub fn save(&self) -> Result<(), AppError> {
        let tmp = self.path.with_extension("toml.tmp");
        let content =
            toml::to_string_pretty(&self.config).map_err(|e| AppError::Config(e.to_string()))?;
        std::fs::write(&tmp, &content)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

pub fn optional_system_cards() -> Vec<CardConfig> {
    let definitions = [
        (
            "system-load",
            "系统负载",
            "load_average",
            "utilities-system-monitor-symbolic",
            RendererKind::Value,
            30,
        ),
        (
            "system-swap",
            "交换空间",
            "swap",
            "drive-harddisk-symbolic",
            RendererKind::Progress,
            30,
        ),
        (
            "system-processes",
            "进程数量",
            "process_count",
            "view-list-symbolic",
            RendererKind::Value,
            30,
        ),
        (
            "system-cpu-temp",
            "CPU 温度",
            "cpu_temperature",
            "sensors-temperature-symbolic",
            RendererKind::Value,
            30,
        ),
        (
            "system-filesystem",
            "磁盘空间",
            "filesystem",
            "drive-harddisk-symbolic",
            RendererKind::Progress,
            60,
        ),
        (
            "system-network-traffic",
            "网络速率",
            "network_traffic",
            "network-transmit-receive-symbolic",
            RendererKind::Value,
            5,
        ),
    ];
    definitions
        .into_iter()
        .enumerate()
        .map(
            |(index, (id, title, metric, icon, renderer, interval))| CardConfig {
                id: id.into(),
                title: title.into(),
                page: "monitor".into(),
                order: 100 + index as i32 * 10,
                renderer,
                refresh_interval: interval,
                enabled: false,
                icon: Some(icon.into()),
                description: Some("可选原生系统指标".into()),
                source: Some(SourceConfig {
                    source_type: "builtin".into(),
                    metric: Some(metric.into()),
                    path: None,
                    program: None,
                    args: None,
                    timeout_seconds: 10,
                    max_output_bytes: 20000,
                    method: None,
                    url: None,
                    headers: None,
                    body: None,
                    shell: None,
                    options: None,
                    parser: None,
                }),
                display: None,
                cache_ttl_seconds: None,
                schedule: None,
                click_action: None,
                kind: None,
                plugin: None,
                runtime: CardRuntimeConfig::default(),
            },
        )
        .collect()
}

pub fn config_dir() -> std::path::PathBuf {
    dirs_config().join("pulsedeck")
}

pub fn config_path() -> std::path::PathBuf {
    config_dir().join("config.toml")
}

pub fn cache_dir() -> std::path::PathBuf {
    dirs_cache().join("pulsedeck")
}

fn dirs_config() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("XDG_CONFIG_HOME") {
        std::path::PathBuf::from(p)
    } else {
        dirs_home().join(".config")
    }
}

fn dirs_cache() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("XDG_CACHE_HOME") {
        std::path::PathBuf::from(p)
    } else {
        dirs_home().join(".cache")
    }
}

fn dirs_home() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("HOME") {
        std::path::PathBuf::from(p)
    } else {
        std::path::PathBuf::from("/tmp")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_example_config_is_valid() {
        let config: AppConfig =
            toml::from_str(include_str!("../../config/config.example.toml")).unwrap();
        assert!(config
            .cards
            .iter()
            .any(|card| card.id == "scheduled-command-example"
                && card.schedule.as_deref() == Some("daily@08:00,20:00")
                && card
                    .source
                    .as_ref()
                    .is_some_and(|source| source.source_type == "command")));
    }

    #[test]
    fn current_json_example_is_valid() {
        let config: AppConfig =
            serde_json::from_str(include_str!("../../config/config.example.json")).unwrap();
        assert_eq!(config.app.title, "PulseDeck");
        assert!(config.cards.iter().any(|card| card.id == "cpu"));
        assert!(config
            .actions
            .iter()
            .any(|action| action.id == "system-summary"));
    }

    #[test]
    fn plugin_page_options_are_generic() {
        let config: AppConfig = toml::from_str(
            "[[pages]]\nid='plugin-page'\ntitle='Plugin'\nkind='example'\n[pages.plugin]\nvalue=7\n",
        )
        .unwrap();
        let page = &config.pages[0];
        let options = page.plugin.as_ref().unwrap();
        assert_eq!(page.kind.as_deref(), Some("example"));
        assert_eq!(options["value"].as_integer(), Some(7));
    }

    #[test]
    fn cards_can_reference_hidden_click_actions() {
        let config: AppConfig = toml::from_str(
            "[[cards]]\nid='service'\ntitle='Service'\npage='monitor'\nclick_action='toggle-service'\n\
             [[actions]]\nid='toggle-service'\nname='Toggle'\npage='actions'\nvisible=false\nconfirm=true\n\
             confirm_title='Confirm toggle?'\nconfirm_detail='Changes the service state.'\n",
        )
        .unwrap();
        assert_eq!(
            config.cards[0].click_action.as_deref(),
            Some("toggle-service")
        );
        assert!(!config.actions[0].visible);
        assert!(config.actions[0].confirm);
        assert_eq!(
            config.actions[0].confirm_title.as_deref(),
            Some("Confirm toggle?")
        );
        assert_eq!(
            config.actions[0].confirm_detail.as_deref(),
            Some("Changes the service state.")
        );
    }

    #[test]
    fn old_ui_config_receives_layout_defaults() {
        let config: AppConfig = toml::from_str("[ui]\ndefault_page='monitor'\n").unwrap();
        assert_eq!(config.ui.card_columns, 3);
        assert_eq!(config.ui.card_height, 133);
        assert!(config.ui.fixed_card_size);
    }
}
