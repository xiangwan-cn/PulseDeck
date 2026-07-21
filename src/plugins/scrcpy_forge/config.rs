use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageConfig {
    #[serde(default = "default_api_url")]
    pub api_url: String,
    #[serde(default = "default_program")]
    pub daemon_program: String,
    #[serde(default)]
    pub daemon_args: Vec<String>,
    #[serde(default = "default_preview_interval")]
    pub preview_interval_seconds: u64,
    #[serde(default = "default_metadata_interval")]
    pub metadata_interval_seconds: u64,
    #[serde(default = "default_health_interval")]
    pub health_interval_seconds: u64,
    #[serde(default = "default_columns")]
    pub columns: u32,
    #[serde(default = "default_card_width")]
    pub card_width: i32,
    #[serde(default = "default_card_height")]
    pub card_height: i32,
    #[serde(default = "default_preview_height")]
    pub preview_height: i32,
    #[serde(default)]
    pub endpoints: Endpoints,
    #[serde(default)]
    pub cards: Vec<CardConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoints {
    #[serde(default = "health")]
    pub health: String,
    #[serde(default = "shutdown")]
    pub shutdown: String,
    #[serde(default = "devices")]
    pub devices: String,
    #[serde(default = "connect")]
    pub connect: String,
    #[serde(default = "device_preview")]
    pub device_preview: String,
    #[serde(default = "session_preview")]
    pub session_preview: String,
    #[serde(default = "session_metrics")]
    pub session_metrics: String,
    #[serde(default = "scripts")]
    pub tasks: String,
    #[serde(default = "runs")]
    pub task_runs: String,
    #[serde(default = "sessions")]
    pub sessions: String,
    #[serde(default = "session_start")]
    pub session_start: String,
    #[serde(default = "task_run")]
    pub task_run: String,
    #[serde(default = "task_stop")]
    pub task_stop: String,
    #[serde(default = "profile")]
    pub profile: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            health: health(),
            shutdown: shutdown(),
            devices: devices(),
            connect: connect(),
            device_preview: device_preview(),
            session_preview: session_preview(),
            session_metrics: session_metrics(),
            tasks: scripts(),
            task_runs: runs(),
            sessions: sessions(),
            session_start: session_start(),
            task_run: task_run(),
            task_stop: task_stop(),
            profile: profile(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardConfig {
    pub role: String,
    pub title: String,
    #[serde(default)]
    pub order: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

fn default_api_url() -> String {
    "http://127.0.0.1:27180/api/v1".into()
}
fn default_program() -> String {
    "scrcpy-forge-daemon".into()
}
fn default_preview_interval() -> u64 {
    5
}
fn default_metadata_interval() -> u64 {
    10
}
fn default_health_interval() -> u64 {
    3
}
fn default_columns() -> u32 {
    3
}
fn default_card_width() -> i32 {
    180
}
fn default_card_height() -> i32 {
    252
}
fn default_preview_height() -> i32 {
    180
}
fn default_true() -> bool {
    true
}
fn health() -> String {
    "health".into()
}
fn shutdown() -> String {
    "shutdown".into()
}
fn devices() -> String {
    "devices".into()
}
fn connect() -> String {
    "devices/connect".into()
}
fn device_preview() -> String {
    "devices/{serial}/screenshot".into()
}
fn session_preview() -> String {
    "sessions/{serial}/frame.jpg".into()
}
fn session_metrics() -> String {
    "sessions/{serial}/metrics".into()
}
fn scripts() -> String {
    "scripts".into()
}
fn runs() -> String {
    "scripts/runs".into()
}
fn sessions() -> String {
    "sessions".into()
}
fn session_start() -> String {
    "sessions/{serial}/start".into()
}
fn task_run() -> String {
    "scripts/run-named".into()
}
fn task_stop() -> String {
    "scripts/devices/{serial}/stop".into()
}
fn profile() -> String {
    "sessions/{serial}/{kind}".into()
}

impl Default for PageConfig {
    fn default() -> Self {
        Self {
            api_url: default_api_url(),
            daemon_program: default_program(),
            daemon_args: vec![],
            preview_interval_seconds: default_preview_interval(),
            metadata_interval_seconds: default_metadata_interval(),
            health_interval_seconds: default_health_interval(),
            columns: default_columns(),
            card_width: default_card_width(),
            card_height: default_card_height(),
            preview_height: default_preview_height(),
            endpoints: Endpoints::default(),
            cards: vec![],
        }
    }
}
