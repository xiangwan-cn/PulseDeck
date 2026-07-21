use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginMode {
    OneShot,
    Persistent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol_version: u32,
    pub mode: PluginMode,
    pub program: String,
    pub args: Vec<String>,
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout_seconds: u64,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_seconds: u64,
    #[serde(default)]
    pub restart_on_crash: bool,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
}

fn default_startup_timeout() -> u64 {
    5
}
fn default_request_timeout() -> u64 {
    10
}
fn default_max_restarts() -> u32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRequest {
    pub protocol: u32,
    pub request_id: u64,
    #[serde(rename = "type")]
    pub request_type: String,
    pub metric: Option<String>,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginResponse {
    pub protocol: u32,
    pub request_id: u64,
    pub ok: bool,
    pub result: Option<PluginResult>,
    pub error: Option<PluginError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginResult {
    pub kind: Option<String>,
    pub value: serde_json::Value,
    pub subtitle: Option<String>,
    pub state: Option<String>,
    pub tooltip: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}
