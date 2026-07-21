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
    pub pages: Vec<PageConfig>,
    #[serde(default)]
    pub cards: Vec<CardConfig>,
    #[serde(default)]
    pub actions: Vec<ActionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSection {
    #[serde(default = "default_title")]
    pub title: String,
    #[serde(default = "default_true")]
    pub pause_when_inactive: bool,
    #[serde(default = "default_true")]
    pub reload_on_change: bool,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_true")]
    pub keep_screen_on: bool,
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
            pause_when_inactive: default_true(),
            reload_on_change: default_true(),
            log_level: default_log_level(),
            keep_screen_on: default_true(),
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
    /// ScrcpyForge plugin settings are absent from builds without that feature.
    #[cfg(feature = "scrcpy-forge")]
    #[serde(default)]
    pub scrcpy_forge: Option<crate::plugins::scrcpy_forge::PageConfig>,
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
    #[serde(default)]
    pub plugin_id: Option<String>,
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
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default)]
    pub confirm: bool,
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

    pub fn load_json_for_migration(
        &self,
        json_path: &std::path::Path,
    ) -> Result<AppConfig, AppError> {
        let content = std::fs::read_to_string(json_path).map_err(|e| AppError::ConfigParse {
            path: json_path.to_path_buf(),
            message: e.to_string(),
        })?;

        serde_json::from_str(&content).map_err(|e| AppError::ConfigParse {
            path: json_path.to_path_buf(),
            message: e.to_string(),
        })
    }

    pub fn save(&self) -> Result<(), AppError> {
        let tmp = self.path.with_extension("toml.tmp");
        let content =
            toml::to_string_pretty(&self.config).map_err(|e| AppError::Config(e.to_string()))?;
        std::fs::write(&tmp, &content)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn set_config(&mut self, config: AppConfig) {
        self.config = config;
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
                    plugin_id: None,
                }),
                display: None,
                cache_ttl_seconds: None,
                schedule: None,
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

pub fn secrets_path() -> std::path::PathBuf {
    config_dir().join("secrets.toml")
}

pub fn cache_dir() -> std::path::PathBuf {
    dirs_cache().join("pulsedeck")
}

pub fn plugins_dir() -> std::path::PathBuf {
    dirs_data().join("pulsedeck").join("plugins")
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

fn dirs_data() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("XDG_DATA_HOME") {
        std::path::PathBuf::from(p)
    } else {
        dirs_home().join(".local").join("share")
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
        assert!(config.actions.iter().any(|action| action.id == "system-summary"));
    }

    #[cfg(feature = "scrcpy-forge")]
    #[test]
    fn scrcpy_forge_plugin_example_is_valid() {
        let source = format!(
            "{}\n{}",
            include_str!("../../config/config.example.toml"),
            include_str!("../plugins/scrcpy_forge/config.example.toml")
        );
        let config: AppConfig = toml::from_str(&source).unwrap();
        let page = config
            .pages
            .iter()
            .find(|page| page.kind.as_deref() == Some("scrcpy-forge"))
            .unwrap();
        assert_eq!(
            page.scrcpy_forge.as_ref().unwrap().endpoints.tasks,
            "scripts"
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
