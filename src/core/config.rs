use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::model::card_model::{CardState, RendererKind, StatusLevel};

pub const CONFIG_SCHEMA_VERSION: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub schema_version: u32,
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

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            app: AppSection::default(),
            ui: UiSection::default(),
            runtime: RuntimeConfig::default(),
            pages: Vec::new(),
            cards: Vec::new(),
            actions: Vec::new(),
        }
    }
}

/// A portable configuration module loaded from the `config.d` directory.
/// Ordinary exports contain entries only; a named overlay must opt into
/// replacement before it can own global application, UI, or runtime settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFragment {
    pub schema_version: u32,
    /// Human-readable module name used in diagnostics and exported files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Permit this module to replace earlier entries with the same id. Files
    /// are applied in lexical file-name order.
    #[serde(default, skip_serializing_if = "is_false")]
    pub replace_existing: bool,
    /// Optional field-level global overrides. Keeping these optional makes
    /// ordinary card and page exports self-contained and side-effect free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppSectionOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<UiSectionOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeOverride>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<PageConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<CardConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionConfig>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

macro_rules! apply_overrides {
    ($source:expr, $target:expr, $($field:ident),+ $(,)?) => {
        $(if let Some(value) = &$source.$field {
            $target.$field = value.clone();
        })+
    };
}

macro_rules! record_overrides {
    ($patch:expr, $previous:expr, $next:expr, $($field:ident),+ $(,)?) => {
        $(if $previous.$field != $next.$field {
            $patch.$field = Some($next.$field.clone());
        })+
    };
}

impl Default for ConfigFragment {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            name: None,
            replace_existing: false,
            app: None,
            ui: None,
            runtime: None,
            pages: Vec::new(),
            cards: Vec::new(),
            actions: Vec::new(),
        }
    }
}

impl ConfigFragment {
    #[cfg(feature = "pet-card")]
    pub(crate) fn with_card(card: CardConfig) -> Self {
        Self::with_cards(vec![card])
    }

    pub(crate) fn with_cards(cards: Vec<CardConfig>) -> Self {
        Self {
            cards,
            ..Self::default()
        }
    }

    #[cfg(feature = "scrcpy-forge")]
    pub(crate) fn with_page(page: PageConfig) -> Self {
        Self {
            pages: vec![page],
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeProfile {
    Performance,
    #[default]
    Balanced,
    Eco,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenInhibitMode {
    Never,
    #[default]
    WhileActive,
    WhileMapped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SuspendInhibitMode {
    #[default]
    Never,
    WhileActive,
    WhileMapped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IdleViewMode {
    #[default]
    None,
    Dim,
    Minimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub profile: RuntimeProfile,
    #[serde(default = "default_inactive_grace")]
    pub inactive_grace_seconds: u64,
    #[serde(default)]
    pub screen_inhibit: ScreenInhibitMode,
    /// Prevent the desktop session from automatically suspending while the
    /// selected PulseDeck window state is present. This is opt-in because it
    /// can increase energy use and should remain independent from idle blanking.
    #[serde(default)]
    pub suspend_inhibit: SuspendInhibitMode,
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_seconds: u64,
    #[serde(default = "default_idle_stability")]
    pub idle_stability_seconds: u64,
    #[serde(default)]
    pub idle_view: IdleViewMode,
    #[serde(default = "default_idle_brightness")]
    pub idle_visual_brightness_percent: u8,
    #[serde(default)]
    pub quiet_hours_enabled: bool,
    #[serde(default)]
    pub quiet_hours_start_hour: u8,
    #[serde(default = "default_quiet_hours_end")]
    pub quiet_hours_end_hour: u8,
    #[serde(default)]
    pub external_boost: bool,
    #[serde(default = "default_power_sample_seconds")]
    pub power_sample_seconds: u64,
    #[serde(default = "default_thermal_sample_seconds")]
    pub thermal_sample_seconds: u64,
    #[serde(default = "default_agent_attention_seconds")]
    pub agent_attention_seconds: u64,
    #[serde(default = "default_true")]
    pub agent_completion_sound: bool,
    #[serde(default)]
    pub bring_to_foreground_on_attention: bool,
    /// A real user-observation lease temporarily permits explicitly observed
    /// work during quiet hours. It is capped at five minutes by the runtime.
    #[serde(default = "default_observation_lease_seconds")]
    pub observation_lease_seconds: u64,
    /// Battery safety thresholds are intentionally separate from display
    /// colour thresholds. Enter/exit pairs provide generic hysteresis.
    #[serde(default = "default_battery_low_enter_percent")]
    pub battery_low_enter_percent: u8,
    #[serde(default = "default_battery_low_exit_percent")]
    pub battery_low_exit_percent: u8,
    #[serde(default = "default_battery_critical_enter_percent")]
    pub battery_critical_enter_percent: u8,
    #[serde(default = "default_battery_critical_exit_percent")]
    pub battery_critical_exit_percent: u8,
}

pub const HARD_MAX_OBSERVATION_LEASE_SECONDS: u64 = 300;
pub const DEFAULT_BATTERY_LOW_ENTER_PERCENT: u8 = 20;
pub const DEFAULT_BATTERY_LOW_EXIT_PERCENT: u8 = 25;
pub const DEFAULT_BATTERY_CRITICAL_ENTER_PERCENT: u8 = 10;
pub const DEFAULT_BATTERY_CRITICAL_EXIT_PERCENT: u8 = 15;

fn default_inactive_grace() -> u64 {
    15
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
fn default_quiet_hours_end() -> u8 {
    8
}
fn default_power_sample_seconds() -> u64 {
    30
}
fn default_thermal_sample_seconds() -> u64 {
    30
}
fn default_agent_attention_seconds() -> u64 {
    15
}
fn default_observation_lease_seconds() -> u64 {
    HARD_MAX_OBSERVATION_LEASE_SECONDS
}
fn default_battery_low_enter_percent() -> u8 {
    DEFAULT_BATTERY_LOW_ENTER_PERCENT
}
fn default_battery_low_exit_percent() -> u8 {
    DEFAULT_BATTERY_LOW_EXIT_PERCENT
}
fn default_battery_critical_enter_percent() -> u8 {
    DEFAULT_BATTERY_CRITICAL_ENTER_PERCENT
}
fn default_battery_critical_exit_percent() -> u8 {
    DEFAULT_BATTERY_CRITICAL_EXIT_PERCENT
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            profile: RuntimeProfile::Balanced,
            inactive_grace_seconds: default_inactive_grace(),
            screen_inhibit: ScreenInhibitMode::WhileActive,
            suspend_inhibit: SuspendInhibitMode::Never,
            idle_timeout_seconds: default_idle_timeout(),
            idle_stability_seconds: default_idle_stability(),
            idle_view: IdleViewMode::None,
            idle_visual_brightness_percent: default_idle_brightness(),
            quiet_hours_enabled: false,
            quiet_hours_start_hour: 0,
            quiet_hours_end_hour: default_quiet_hours_end(),
            external_boost: false,
            power_sample_seconds: default_power_sample_seconds(),
            thermal_sample_seconds: default_thermal_sample_seconds(),
            agent_attention_seconds: default_agent_attention_seconds(),
            agent_completion_sound: true,
            bring_to_foreground_on_attention: false,
            observation_lease_seconds: default_observation_lease_seconds(),
            battery_low_enter_percent: default_battery_low_enter_percent(),
            battery_low_exit_percent: default_battery_low_exit_percent(),
            battery_critical_enter_percent: default_battery_critical_enter_percent(),
            battery_critical_exit_percent: default_battery_critical_exit_percent(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<RuntimeProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inactive_grace_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_inhibit: Option<ScreenInhibitMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspend_inhibit: Option<SuspendInhibitMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_stability_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_view: Option<IdleViewMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_visual_brightness_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_hours_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_hours_start_hour: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_hours_end_hour: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_boost: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_sample_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal_sample_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_attention_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_completion_sound: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bring_to_foreground_on_attention: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_lease_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_low_enter_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_low_exit_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_critical_enter_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_critical_exit_percent: Option<u8>,
}

impl RuntimeOverride {
    pub(crate) fn apply_to(&self, target: &mut RuntimeConfig) {
        apply_overrides!(
            self,
            target,
            profile,
            inactive_grace_seconds,
            screen_inhibit,
            suspend_inhibit,
            idle_timeout_seconds,
            idle_stability_seconds,
            idle_view,
            idle_visual_brightness_percent,
            quiet_hours_enabled,
            quiet_hours_start_hour,
            quiet_hours_end_hour,
            external_boost,
            power_sample_seconds,
            thermal_sample_seconds,
            agent_attention_seconds,
            agent_completion_sound,
            bring_to_foreground_on_attention,
            observation_lease_seconds,
            battery_low_enter_percent,
            battery_low_exit_percent,
            battery_critical_enter_percent,
            battery_critical_exit_percent
        );
    }

    pub(crate) fn record_changes(&mut self, previous: &RuntimeConfig, next: &RuntimeConfig) {
        record_overrides!(
            self,
            previous,
            next,
            profile,
            inactive_grace_seconds,
            screen_inhibit,
            suspend_inhibit,
            idle_timeout_seconds,
            idle_stability_seconds,
            idle_view,
            idle_visual_brightness_percent,
            quiet_hours_enabled,
            quiet_hours_start_hour,
            quiet_hours_end_hour,
            external_boost,
            power_sample_seconds,
            thermal_sample_seconds,
            agent_attention_seconds,
            agent_completion_sound,
            bring_to_foreground_on_attention,
            observation_lease_seconds,
            battery_low_enter_percent,
            battery_low_exit_percent,
            battery_critical_enter_percent,
            battery_critical_exit_percent
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSectionOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload_on_change: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<usize>,
}

impl AppSectionOverride {
    pub(crate) fn apply_to(&self, target: &mut AppSection) {
        apply_overrides!(
            self,
            target,
            title,
            reload_on_change,
            log_level,
            max_output_bytes
        );
    }

    pub(crate) fn record_changes(&mut self, previous: &AppSection, next: &AppSection) {
        record_overrides!(
            self,
            previous,
            next,
            title,
            reload_on_change,
            log_level,
            max_output_bytes
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiSectionOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_page: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_columns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_width: Option<Option<i32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_height: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_card_size: Option<bool>,
}

impl UiSectionOverride {
    pub(crate) fn apply_to(&self, target: &mut UiSection) {
        apply_overrides!(
            self,
            target,
            default_page,
            compact,
            card_columns,
            card_width,
            card_height,
            fixed_card_size
        );
    }

    pub(crate) fn record_changes(&mut self, previous: &UiSection, next: &UiSection) {
        record_overrides!(
            self,
            previous,
            next,
            default_page,
            compact,
            card_columns,
            card_width,
            card_height,
            fixed_card_size
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct CardConfig {
    pub id: String,
    pub title: String,
    pub page: String,
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub order: i32,
    #[serde(
        default = "default_renderer",
        skip_serializing_if = "is_default_renderer"
    )]
    pub renderer: RendererKind,
    #[serde(
        rename = "refresh",
        default = "default_interval",
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration",
        skip_serializing_if = "is_default_interval"
    )]
    pub refresh_interval: u64,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<DisplayConfig>,
    #[serde(
        rename = "cache_ttl",
        default,
        deserialize_with = "deserialize_optional_duration",
        serialize_with = "serialize_optional_duration",
        skip_serializing_if = "Option::is_none"
    )]
    pub cache_ttl_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    /// Optional action id executed when this standard card is clicked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click_action: Option<String>,
    /// Optional custom card implementation. Standard metric cards omit this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Generic configuration owned and decoded by the selected card plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "is_default_card_runtime")]
    pub runtime: CardRuntimeConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardRuntimeConfig {
    #[serde(default, skip_serializing_if = "is_default_workload")]
    pub workload: CardWorkload,
    #[serde(default, skip_serializing_if = "is_inherit_behavior")]
    pub inactive_behavior: CardWorkBehavior,
    #[serde(default, skip_serializing_if = "is_inherit_behavior")]
    pub idle_behavior: CardWorkBehavior,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inactive_interval_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_interval_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_interval_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CardWorkload {
    #[default]
    Auto,
    Live,
    Normal,
    Expensive,
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CardWorkBehavior {
    #[default]
    Inherit,
    Keep,
    Throttle,
    Pause,
}

impl Default for CardRuntimeConfig {
    fn default() -> Self {
        Self {
            workload: CardWorkload::Auto,
            inactive_behavior: CardWorkBehavior::Inherit,
            idle_behavior: CardWorkBehavior::Inherit,
            inactive_interval_seconds: None,
            idle_interval_seconds: None,
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

fn is_zero_i32(value: &i32) -> bool {
    *value == 0
}

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_default_renderer(value: &RendererKind) -> bool {
    *value == RendererKind::Value
}

fn is_default_interval(value: &u64) -> bool {
    *value == default_interval()
}

fn is_default_timeout(value: &u64) -> bool {
    *value == default_timeout()
}

fn is_default_max_output(value: &usize) -> bool {
    *value == default_max_output()
}

fn is_default_card_runtime(value: &CardRuntimeConfig) -> bool {
    *value == CardRuntimeConfig::default()
}

fn is_default_workload(value: &CardWorkload) -> bool {
    *value == CardWorkload::Auto
}

fn is_inherit_behavior(value: &CardWorkBehavior) -> bool {
    *value == CardWorkBehavior::Inherit
}

pub(crate) fn parse_duration(value: &str) -> Result<u64, String> {
    let value = value.trim();
    let split = value
        .find(|character: char| !character.is_ascii_digit())
        .ok_or_else(|| "duration requires a unit: s, m, h, or d".to_string())?;
    let (amount, unit) = value.split_at(split);
    let amount = amount
        .parse::<u64>()
        .map_err(|_| format!("invalid duration: {value}"))?;
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        _ => {
            return Err(format!(
                "invalid duration unit in {value}; use s, m, h, or d"
            ))
        }
    };
    amount
        .checked_mul(multiplier)
        .ok_or_else(|| format!("duration is too large: {value}"))
}

fn format_duration(seconds: u64) -> String {
    if seconds > 0 && seconds % 86_400 == 0 {
        format!("{}d", seconds / 86_400)
    } else if seconds > 0 && seconds % 3_600 == 0 {
        format!("{}h", seconds / 3_600)
    } else if seconds > 0 && seconds % 60 == 0 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

fn deserialize_duration<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse_duration(&value).map_err(serde::de::Error::custom)
}

fn serialize_duration<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&format_duration(*value))
}

fn deserialize_optional_duration<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(|value| parse_duration(&value).map_err(serde::de::Error::custom))
        .transpose()
}

fn serialize_optional_duration<S>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(seconds) => serializer.serialize_some(&format_duration(*seconds)),
        None => serializer.serialize_none(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceConfig {
    Builtin(String),
    File(FileSourceConfig),
    Command(CommandSourceConfig),
    Http(Box<HttpSourceConfig>),
    Text(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSourceConfig {
    pub path: String,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub first_line: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSourceConfig {
    pub run: Vec<String>,
    #[serde(
        rename = "timeout",
        default = "default_timeout",
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration",
        skip_serializing_if = "is_default_timeout"
    )]
    pub timeout_seconds: u64,
    #[serde(
        rename = "max_output",
        default = "default_max_output",
        skip_serializing_if = "is_default_max_output"
    )]
    pub max_output_bytes: usize,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reverse_lines: bool,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub subtitle_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpSourceConfig {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(
        rename = "timeout",
        default = "default_timeout",
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration",
        skip_serializing_if = "is_default_timeout"
    )]
    pub timeout_seconds: u64,
    #[serde(
        rename = "max_output",
        default = "default_max_output",
        skip_serializing_if = "is_default_max_output"
    )]
    pub max_output_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parser: Option<ParserConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Builtin,
    File,
    Command,
    Http,
    Text,
}

fn default_timeout() -> u64 {
    10
}

impl SourceConfig {
    pub fn kind(&self) -> SourceKind {
        match self {
            Self::Builtin(_) => SourceKind::Builtin,
            Self::File(_) => SourceKind::File,
            Self::Command(_) => SourceKind::Command,
            Self::Http(_) => SourceKind::Http,
            Self::Text(_) => SourceKind::Text,
        }
    }

    pub fn builtin_metric(&self) -> Option<&str> {
        match self {
            Self::Builtin(metric) => Some(metric),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParserConfig {
    #[serde(rename = "type")]
    pub parser_type: ParserKind,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParserKind {
    JsonPath,
    Regex,
    Number,
    FirstLine,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_change: Option<f64>,
    /// 列表项目超过此数量时切换为多列。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns_after: Option<usize>,
    /// 列表多列模式的列数，默认 2。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<usize>,
    /// Per-card width override in logical pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_width: Option<i32>,
    /// Per-card height override in logical pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_height: Option<i32>,
    /// Per-card override for fixed versus content-driven height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_size: Option<bool>,
    /// Optional SVG replacement for the top-right refresh icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_svg: Option<CardLogoSvgConfig>,
    /// Optional static SVG decoration behind the card content. Relative paths
    /// are resolved from the PulseDeck configuration directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_svg: Option<CardBackgroundSvgConfig>,
    /// Optional colors for standard (non-plugin) cards. Empty fields preserve
    /// the application theme and renderer defaults.
    #[serde(default, skip_serializing_if = "is_default_card_colors")]
    pub colors: CardColorsConfig,
    /// First matching rule selects the card's named visual state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<CardVisualStateConfig>,
    /// Smooth color changes without adding an animation timer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<CardTransitionConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardLogoSvgConfig {
    pub path: String,
    #[serde(
        default = "default_card_logo_svg_size",
        skip_serializing_if = "is_default_card_logo_svg_size"
    )]
    pub size: i32,
    #[serde(
        default = "default_card_logo_svg_opacity",
        skip_serializing_if = "is_default_card_logo_svg_opacity"
    )]
    pub opacity: f64,
}

impl CardLogoSvgConfig {
    pub(crate) fn resolve_path(
        &self,
        config_directory: &std::path::Path,
    ) -> Result<std::path::PathBuf, String> {
        resolve_local_svg_path(config_directory, &self.path)
    }
}

fn default_card_logo_svg_size() -> i32 {
    24
}

fn is_default_card_logo_svg_size(value: &i32) -> bool {
    *value == default_card_logo_svg_size()
}

fn default_card_logo_svg_opacity() -> f64 {
    0.90
}

fn is_default_card_logo_svg_opacity(value: &f64) -> bool {
    *value == default_card_logo_svg_opacity()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardBackgroundSvgConfig {
    pub path: String,
    #[serde(
        default = "default_card_background_svg_opacity",
        skip_serializing_if = "is_default_card_background_svg_opacity"
    )]
    pub opacity: f64,
    #[serde(default, skip_serializing_if = "is_default_card_background_svg_fit")]
    pub fit: CardBackgroundSvgFit,
    #[serde(
        default,
        skip_serializing_if = "is_default_card_background_svg_position"
    )]
    pub position: CardBackgroundSvgPosition,
}

impl CardBackgroundSvgConfig {
    pub(crate) fn resolve_path(
        &self,
        config_directory: &std::path::Path,
    ) -> Result<std::path::PathBuf, String> {
        resolve_local_svg_path(config_directory, &self.path)
    }

    pub(crate) fn opacity_percent(&self) -> f64 {
        if self.opacity.is_finite() {
            self.opacity.clamp(0.0, 1.0) * 100.0
        } else {
            default_card_background_svg_opacity() * 100.0
        }
    }
}

fn resolve_local_svg_path(
    config_directory: &std::path::Path,
    configured: &str,
) -> Result<std::path::PathBuf, String> {
    let configured = configured.trim();
    if configured.is_empty() {
        return Err("path cannot be empty".into());
    }
    let path = std::path::Path::new(configured);
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        return Err("path must name an .svg file".into());
    }
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_directory.join(path)
    };
    let resolved = std::fs::canonicalize(&path)
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    if !resolved.is_file() {
        return Err(format!("{} is not a regular file", resolved.display()));
    }
    Ok(resolved)
}

fn default_card_background_svg_opacity() -> f64 {
    0.10
}

fn is_default_card_background_svg_opacity(value: &f64) -> bool {
    *value == default_card_background_svg_opacity()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CardBackgroundSvgFit {
    #[default]
    Contain,
    Cover,
}

impl CardBackgroundSvgFit {
    pub(crate) fn as_css(self) -> &'static str {
        match self {
            Self::Contain => "contain",
            Self::Cover => "cover",
        }
    }
}

fn is_default_card_background_svg_fit(value: &CardBackgroundSvgFit) -> bool {
    *value == CardBackgroundSvgFit::default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CardBackgroundSvgPosition {
    #[default]
    Center,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl CardBackgroundSvgPosition {
    pub(crate) fn as_css(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::Left => "left center",
            Self::Right => "right center",
            Self::Top => "center top",
            Self::Bottom => "center bottom",
            Self::TopLeft => "left top",
            Self::TopRight => "right top",
            Self::BottomLeft => "left bottom",
            Self::BottomRight => "right bottom",
        }
    }
}

fn is_default_card_background_svg_position(value: &CardBackgroundSvgPosition) -> bool {
    *value == CardBackgroundSvgPosition::default()
}

fn is_default_card_colors(value: &CardColorsConfig) -> bool {
    *value == CardColorsConfig::default()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CardColorsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    /// One color creates a tint; multiple colors create a subtle gradient.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background: Vec<String>,
    /// Opacity applied to every background color (default: 0.12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_opacity: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardTransitionConfig {
    #[serde(default = "default_card_transition_ms")]
    pub duration_ms: u32,
    #[serde(default = "default_card_transition_easing")]
    pub easing: String,
}

fn default_card_transition_ms() -> u32 {
    180
}

fn default_card_transition_easing() -> String {
    "ease-out".into()
}

impl Default for CardTransitionConfig {
    fn default() -> Self {
        Self {
            duration_ms: default_card_transition_ms(),
            easing: default_card_transition_easing(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardVisualStateConfig {
    /// Stable, user-defined state name used for diagnostics and CSS switching.
    pub name: String,
    /// Match the source lifecycle state before evaluating value conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_state: Option<CardState>,
    /// Match the semantic level produced by a status value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_level: Option<StatusLevel>,
    /// Inclusive numeric bounds. Both may be supplied to form a range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Text matchers are combined with the other supplied conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub ignore_case: bool,
    /// Optional presentation overrides applied while this rule matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "is_default_card_colors")]
    pub colors: CardColorsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

mod loader;
pub use loader::ConfigManager;
pub(crate) use loader::ConfigModuleInfo;

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
                source: Some(SourceConfig::Builtin(metric.into())),
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

pub fn config_modules_dir() -> std::path::PathBuf {
    config_dir().join("config.d")
}

/// Create a configuration directory with owner-only permissions. Configuration
/// modules may contain HTTP credentials or command arguments, so the directory
/// boundary is part of the configuration security contract.
pub fn ensure_private_directory(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Create the parent of a user-selected state/output file without changing
/// permissions on an already-existing directory. Plugin paths are allowed to
/// point at locations such as `/tmp` or a user-managed data directory; making
/// those existing parents mode 0700 would be a surprising and potentially
/// destructive side effect. Newly-created directories still get the private
/// mode before the file is written.
#[cfg(feature = "pet-card")]
pub fn ensure_private_parent(path: &std::path::Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut missing = Vec::new();
    let mut current = parent.to_path_buf();
    while !current.exists() {
        missing.push(current.clone());
        let Some(next) = current.parent() else {
            break;
        };
        if next == current {
            break;
        }
        current = next.to_path_buf();
    }
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    for directory in missing {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
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
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let suffix = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pulsedeck-config-test-{}-{suffix}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn current_example_config_is_valid() {
        let config: AppConfig =
            toml::from_str(include_str!("../../config/config.example.toml")).unwrap();
        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
        assert!(config
            .cards
            .iter()
            .any(|card| card.id == "scheduled-command-example"
                && card.schedule.as_deref() == Some("daily@08:00,20:00")
                && card
                    .source
                    .as_ref()
                    .is_some_and(|source| source.kind() == SourceKind::Command)));
    }

    #[test]
    fn empty_root_with_disabled_capability_module_keeps_default_registry() {
        let dir = TestDir::new();
        let root = dir.path().join("config.toml");
        let modules = dir.path().join("config.d");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(&root, "schema_version = 4\n").unwrap();
        std::fs::write(
            modules.join("70-system-cards.toml"),
            "schema_version = 4\n[[cards]]\nid = 'system-load'\ntitle = 'Load'\npage = 'monitor'\nenabled = false\n",
        )
        .unwrap();

        let mut manager = ConfigManager::new(root);
        manager.load().unwrap();
        assert!(manager.config().cards.iter().all(|card| !card.enabled));
        assert!(manager.uses_default_card_registry());
    }

    #[test]
    fn current_json_example_is_valid() {
        let config: AppConfig =
            serde_json::from_str(include_str!("../../config/config.example.json")).unwrap();
        let toml_config: AppConfig =
            toml::from_str(include_str!("../../config/config.example.toml")).unwrap();
        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
        assert_eq!(config.runtime, toml_config.runtime);
        assert_eq!(config.app.title, "PulseDeck");
        assert!(config.cards.iter().any(|card| card.id == "cpu"));
        assert!(config.cards.iter().any(|card| {
            card.id == "battery-temp"
                && card
                    .display
                    .as_ref()
                    .is_some_and(|display| display.states.len() == 3)
        }));
        assert!(config
            .actions
            .iter()
            .any(|action| action.id == "system-summary"));
    }

    #[test]
    fn plugin_page_options_are_generic() {
        let config: AppConfig = toml::from_str(
            "schema_version=4\n[[pages]]\nid='plugin-page'\ntitle='Plugin'\nkind='example'\n[pages.plugin]\nvalue=7\n",
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
            "schema_version=4\n[[cards]]\nid='service'\ntitle='Service'\npage='monitor'\nclick_action='toggle-service'\n\
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
    fn omitted_optional_ui_fields_receive_current_defaults() {
        let config: AppConfig =
            toml::from_str("schema_version=4\n[ui]\ndefault_page='monitor'\n").unwrap();
        assert_eq!(config.ui.card_columns, 3);
        assert_eq!(config.ui.card_height, 133);
        assert!(config.ui.fixed_card_size);
    }

    #[test]
    fn standard_cards_decode_visual_states_and_multicolor_backgrounds() {
        let config: AppConfig = toml::from_str(
            "schema_version=4\n[[cards]]\nid='thermal'\ntitle='Thermal'\npage='monitor'\n\
             [cards.display.transition]\nduration_ms=240\n\
             [[cards.display.states]]\nname='hot'\nmin=45.0\nlabel='Too hot'\n\
             [cards.display.states.colors]\naccent='#e01b24'\nbackground=['#e01b24','#9141ac']\n",
        )
        .unwrap();
        let display = config.cards[0].display.as_ref().unwrap();
        assert_eq!(display.transition.as_ref().unwrap().duration_ms, 240);
        assert_eq!(display.states[0].name, "hot");
        assert_eq!(display.states[0].min, Some(45.0));
        assert_eq!(display.states[0].colors.background.len(), 2);
    }

    #[test]
    fn standard_cards_decode_restrained_svg_artwork_options() {
        let config: AppConfig = toml::from_str(
            "schema_version=4\n[[cards]]\nid='illustrated'\ntitle='Illustrated'\npage='monitor'\n\
             [cards.display.logo_svg]\npath='logos/card.svg'\nsize=28\nopacity=0.95\n\
             [cards.display.background_svg]\npath='backgrounds/card.svg'\nopacity=0.16\nfit='cover'\nposition='top-right'\n",
        )
        .unwrap();
        let display = config.cards[0].display.as_ref().unwrap();
        let logo = display.logo_svg.as_ref().unwrap();
        assert_eq!(logo.path, "logos/card.svg");
        assert_eq!(logo.size, 28);
        assert_eq!(logo.opacity, 0.95);
        let background = display.background_svg.as_ref().unwrap();
        assert_eq!(background.path, "backgrounds/card.svg");
        assert_eq!(background.opacity, 0.16);
        assert_eq!(background.fit, CardBackgroundSvgFit::Cover);
        assert_eq!(background.position, CardBackgroundSvgPosition::TopRight);

        let serialized = toml::to_string_pretty(&config).unwrap();
        assert!(serialized.contains("[cards.display.logo_svg]"));
        assert!(serialized.contains("[cards.display.background_svg]"));
        assert!(serialized.contains("position = \"top-right\""));
    }

    #[test]
    fn obsolete_v3_runtime_fields_and_unknown_v4_enums_are_rejected() {
        for runtime in [
            "keep_screen_on=true",
            "idle_power_saving=true",
            "codex_protection_minutes=60",
            "profile='turbo'",
            "screen_inhibit='always'",
        ] {
            let document = format!("schema_version=4\n[runtime]\n{runtime}\n");
            assert!(toml::from_str::<AppConfig>(&document).is_err(), "{runtime}");
        }
    }

    #[test]
    fn obsolete_generic_card_fields_are_rejected() {
        assert!(toml::from_str::<AppConfig>("[app]\ntitle='missing version'\n").is_err());
        assert!(toml::from_str::<AppConfig>(
            "schema_version=4\n[[cards]]\nid='legacy'\ntitle='Legacy'\npage='monitor'\n\
             [cards.source]\ntype='command'\nshell=true\n"
        )
        .is_err());
        assert!(toml::from_str::<AppConfig>(
            "schema_version=4\n[[cards]]\nid='legacy'\ntitle='Legacy'\npage='monitor'\n\
             [cards.source]\ntype='http'\n[cards.source.parser]\ntype='number'\nsteps=[]\n"
        )
        .is_err());
        assert!(toml::from_str::<AppConfig>(
            "schema_version=4\n[[cards]]\nid='legacy'\ntitle='Legacy'\npage='monitor'\n\
             [cards.source]\ntype='static'\n"
        )
        .is_err());
        assert!(toml::from_str::<AppConfig>(
            "schema_version=4\n[[cards]]\nid='legacy'\ntitle='Legacy'\npage='monitor'\n\
             [cards.runtime]\nclass='system'\n"
        )
        .is_err());
        assert!(toml::from_str::<AppConfig>(
            "schema_version=4\n[[cards]]\nid='legacy'\ntitle='Legacy'\npage='monitor'\n\
             [cards.runtime]\nworkload='burst'\n"
        )
        .is_err());
        assert!(toml::from_str::<AppConfig>(
            "schema_version=4\n[[cards]]\nid='legacy'\ntitle='Legacy'\npage='monitor'\nrefresh_interval=5\n"
        )
        .is_err());
    }

    #[test]
    fn compact_v4_card_syntax_round_trips_without_default_noise() {
        let config: ConfigFragment = toml::from_str(
            "schema_version=4\nname='personal'\nreplace_existing=true\n\
             [[cards]]\nid='kernel'\ntitle='Kernel'\npage='monitor'\nrefresh='1h'\n\
             source={command={run=['uname','-r'],timeout='5s'}}\n",
        )
        .unwrap();
        let card = &config.cards[0];
        assert_eq!(card.refresh_interval, 3600);
        assert!(matches!(card.source, Some(SourceConfig::Command(_))));
        let serialized = toml::to_string_pretty(&config).unwrap();
        assert!(serialized.contains("refresh = \"1h\""));
        assert!(serialized.contains("run = ["));
        assert!(!serialized.contains("enabled = true"));
        assert!(!serialized.contains("[cards.runtime]"));
        assert!(!serialized.contains("max_output"));
    }

    #[test]
    fn config_directory_loads_toml_and_json_modules_in_file_name_order() {
        let directory = TestDir::new();
        let root = directory.path().join("config.toml");
        let modules = directory.path().join("config.d");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(
            &root,
            "schema_version=4\n[[pages]]\nid='monitor'\ntitle='Monitor'\n",
        )
        .unwrap();
        std::fs::write(
            modules.join("20-second.json"),
            r#"{"schema_version":4,"cards":[{"id":"second","title":"Second","page":"monitor"}]}"#,
        )
        .unwrap();
        std::fs::write(
            modules.join("10-first.toml"),
            "schema_version=4\n[[cards]]\nid='first'\ntitle='First'\npage='monitor'\n",
        )
        .unwrap();
        std::fs::write(
            modules.join("00-disabled.toml.disabled"),
            "this file is intentionally ignored",
        )
        .unwrap();

        let mut manager = ConfigManager::new(root);
        manager.load().unwrap();
        assert_eq!(
            manager
                .config()
                .cards
                .iter()
                .map(|card| card.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn config_loader_validates_svg_background_assets_without_losing_last_good_state() {
        let directory = TestDir::new();
        let root = directory.path().join("config.toml");
        let backgrounds = directory.path().join("backgrounds");
        std::fs::create_dir_all(&backgrounds).unwrap();
        std::fs::write(
            backgrounds.join("card.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"/>"#,
        )
        .unwrap();
        std::fs::write(
            &root,
            "schema_version=4\n[[cards]]\nid='svg-card'\ntitle='SVG'\npage='monitor'\n\
             [cards.display.logo_svg]\npath='backgrounds/card.svg'\n\
             [cards.display.background_svg]\npath='backgrounds/card.svg'\n",
        )
        .unwrap();

        let mut manager = ConfigManager::new(root.clone());
        manager.load().unwrap();
        assert_eq!(manager.config().cards[0].id, "svg-card");

        std::fs::write(
            &root,
            "schema_version=4\n[[cards]]\nid='svg-card'\ntitle='SVG'\npage='monitor'\n\
             [cards.display.background_svg]\npath='backgrounds/missing.svg'\n",
        )
        .unwrap();
        let error = manager.load().unwrap_err().to_string();
        assert!(error.contains("background_svg"));
        assert!(error.contains("missing.svg"));
        assert_eq!(manager.config().cards[0].id, "svg-card");
    }

    #[test]
    fn config_loader_rejects_non_svg_backgrounds_and_out_of_range_opacity() {
        let directory = TestDir::new();
        let root = directory.path().join("config.toml");
        std::fs::write(
            &root,
            "schema_version=4\n[[cards]]\nid='image-card'\ntitle='Image'\npage='monitor'\n\
             [cards.display.background_svg]\npath='card.png'\n",
        )
        .unwrap();
        let mut manager = ConfigManager::new(root.clone());
        let error = manager.load().unwrap_err().to_string();
        assert!(error.contains("path must name an .svg file"));

        std::fs::write(
            directory.path().join("card.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#,
        )
        .unwrap();
        std::fs::write(
            &root,
            "schema_version=4\n[[cards]]\nid='image-card'\ntitle='Image'\npage='monitor'\n\
             [cards.display.background_svg]\npath='card.svg'\nopacity=1.1\n",
        )
        .unwrap();
        let error = manager.load().unwrap_err().to_string();
        assert!(error.contains("opacity must be between 0.0 and 1.0"));

        std::fs::write(
            &root,
            "schema_version=4\n[[cards]]\nid='image-card'\ntitle='Image'\npage='monitor'\n\
             [cards.display.logo_svg]\npath='card.svg'\nsize=65\n",
        )
        .unwrap();
        let error = manager.load().unwrap_err().to_string();
        assert!(error.contains("logo_svg size must be between 8 and 64"));
    }

    #[test]
    fn quiet_hours_reject_out_of_range_hours() {
        let dir = TestDir::new();
        let root = dir.path().join("config.toml");
        std::fs::write(
            &root,
            "schema_version=4\n[runtime]\nquiet_hours_enabled=true\nquiet_hours_start_hour=24\nquiet_hours_end_hour=8\n",
        )
        .unwrap();
        let mut manager = ConfigManager::new(root);
        assert!(manager
            .load()
            .unwrap_err()
            .to_string()
            .contains("quiet_hours_start_hour"));
    }

    #[test]
    fn observation_lease_and_battery_thresholds_decode_with_defaults() {
        let config: AppConfig = toml::from_str("schema_version=4\n").unwrap();
        assert_eq!(config.runtime.observation_lease_seconds, 300);
        assert_eq!(config.runtime.battery_low_enter_percent, 20);
        assert_eq!(config.runtime.battery_low_exit_percent, 25);
        assert_eq!(config.runtime.battery_critical_enter_percent, 10);
        assert_eq!(config.runtime.battery_critical_exit_percent, 15);
    }

    #[test]
    fn invalid_observation_lease_is_rejected() {
        let dir = TestDir::new();
        let root = dir.path().join("config.toml");
        std::fs::write(
            &root,
            "schema_version=4\n[runtime]\nobservation_lease_seconds=0\n",
        )
        .unwrap();
        let mut manager = ConfigManager::new(root);
        assert!(manager
            .load()
            .unwrap_err()
            .to_string()
            .contains("observation_lease_seconds"));
    }

    #[test]
    fn invalid_battery_hysteresis_is_rejected() {
        let dir = TestDir::new();
        let root = dir.path().join("config.toml");
        std::fs::write(
            &root,
            "schema_version=4\n[runtime]\nbattery_low_enter_percent=20\nbattery_low_exit_percent=15\nbattery_critical_enter_percent=10\nbattery_critical_exit_percent=12\n",
        )
        .unwrap();
        let mut manager = ConfigManager::new(root);
        assert!(manager
            .load()
            .unwrap_err()
            .to_string()
            .contains("battery thresholds"));
    }

    #[test]
    fn old_schema_is_rejected_without_migration() {
        let directory = TestDir::new();
        let root = directory.path().join("config.toml");
        std::fs::write(&root, "schema_version=2\n").unwrap();
        let mut manager = ConfigManager::new(root);
        let error = manager.load().unwrap_err().to_string();
        assert!(error.contains("unsupported schema_version 2; expected 4"));
    }

    #[test]
    fn duplicate_module_ids_reject_reload_and_keep_last_good_config() {
        let directory = TestDir::new();
        let root = directory.path().join("config.toml");
        let modules = directory.path().join("config.d");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(&root, "schema_version=4\n").unwrap();
        std::fs::write(
            modules.join("10-card.toml"),
            "schema_version=4\n[[cards]]\nid='same'\ntitle='First'\npage='monitor'\n",
        )
        .unwrap();

        let mut manager = ConfigManager::new(root);
        manager.load().unwrap();
        std::fs::write(
            modules.join("20-duplicate.toml"),
            "schema_version=4\n[[cards]]\nid='same'\ntitle='Duplicate'\npage='monitor'\n",
        )
        .unwrap();

        let error = manager.load().unwrap_err().to_string();
        assert!(error.contains("duplicate card id: same"));
        assert_eq!(manager.config().cards.len(), 1);
        assert_eq!(manager.config().cards[0].title, "First");
    }

    #[test]
    fn save_keeps_module_entries_in_their_source_file() {
        let directory = TestDir::new();
        let root = directory.path().join("config.toml");
        let modules = directory.path().join("config.d");
        let card_module = modules.join("10-card.toml");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(&root, "schema_version=4\n").unwrap();
        std::fs::write(
            &card_module,
            "schema_version=4\n[[cards]]\nid='module-card'\ntitle='Module'\npage='monitor'\n",
        )
        .unwrap();

        let mut manager = ConfigManager::new(root.clone());
        manager.load().unwrap();
        manager.config_mut().runtime.screen_inhibit = ScreenInhibitMode::Never;
        manager.config_mut().cards[0].enabled = false;
        manager.save().unwrap();

        let saved_root: AppConfig =
            toml::from_str(&std::fs::read_to_string(&root).unwrap()).unwrap();
        let saved_module: ConfigFragment =
            toml::from_str(&std::fs::read_to_string(&card_module).unwrap()).unwrap();
        assert_eq!(saved_root.runtime.screen_inhibit, ScreenInhibitMode::Never);
        assert!(saved_root.cards.is_empty());
        assert!(!saved_module.cards[0].enabled);
    }

    #[test]
    fn modules_only_accept_exportable_entry_sections() {
        let directory = TestDir::new();
        let root = directory.path().join("config.toml");
        let modules = directory.path().join("config.d");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(&root, "schema_version=4\n").unwrap();
        std::fs::write(
            modules.join("10-invalid.toml"),
            "schema_version=4\n[app]\ntitle='Missing explicit replacement'\n",
        )
        .unwrap();
        let mut manager = ConfigManager::new(root);
        let error = manager.load().unwrap_err().to_string();
        assert!(error.contains("replace_existing = true"));
    }

    #[test]
    fn named_override_owns_replaced_entries_and_global_settings() {
        let directory = TestDir::new();
        let root = directory.path().join("config.toml");
        let modules = directory.path().join("config.d");
        let override_module = modules.join("50-personal.toml");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(
            &root,
            "schema_version=4\n[runtime]\nscreen_inhibit='never'\nprofile='performance'\n\
             [[cards]]\nid='shared'\ntitle='Default'\npage='monitor'\n",
        )
        .unwrap();
        std::fs::write(
            &override_module,
            "schema_version=4\nname='personal'\nreplace_existing=true\n\
             [runtime]\nscreen_inhibit='while-mapped'\n\
             [[cards]]\nid='shared'\ntitle='Custom'\npage='monitor'\n",
        )
        .unwrap();

        let mut manager = ConfigManager::new(root.clone());
        manager.load().unwrap();
        assert_eq!(manager.config().cards[0].title, "Custom");
        assert_eq!(
            manager.config().runtime.screen_inhibit,
            ScreenInhibitMode::WhileMapped
        );
        assert_eq!(
            manager.config().runtime.profile,
            RuntimeProfile::Performance
        );
        manager.config_mut().cards[0].title = "Saved Custom".into();
        manager.config_mut().runtime.screen_inhibit = ScreenInhibitMode::Never;
        manager.save().unwrap();

        let saved_root: AppConfig =
            toml::from_str(&std::fs::read_to_string(&root).unwrap()).unwrap();
        let saved_override: ConfigFragment =
            toml::from_str(&std::fs::read_to_string(&override_module).unwrap()).unwrap();
        assert_eq!(saved_root.cards[0].title, "Default");
        assert_eq!(saved_root.runtime.screen_inhibit, ScreenInhibitMode::Never);
        assert_eq!(saved_override.cards[0].title, "Saved Custom");
        assert_eq!(
            saved_override
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.screen_inhibit),
            Some(ScreenInhibitMode::Never)
        );
        assert_eq!(
            saved_override
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.profile),
            None
        );
    }
}
