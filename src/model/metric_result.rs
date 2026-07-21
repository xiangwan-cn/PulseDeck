use serde::{Deserialize, Serialize};

use super::card_model::CardValue;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricResult {
    pub value: CardValue,
    pub subtitle: Option<String>,
    pub tooltip: Option<String>,
    pub state: MetricState,
    pub cached: bool,
    pub metadata: Option<serde_json::Value>,
}

impl Default for MetricResult {
    fn default() -> Self {
        Self {
            value: CardValue::Empty,
            subtitle: None,
            tooltip: None,
            state: MetricState::Loading,
            cached: false,
            metadata: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricState {
    Normal,
    Loading,
    Unavailable,
    Error,
    Stale,
}

impl MetricResult {
    #[allow(dead_code)]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            value: CardValue::Text("不可用".into()),
            subtitle: None,
            tooltip: Some(reason.into()),
            state: MetricState::Unavailable,
            cached: false,
            metadata: None,
        }
    }

    #[allow(dead_code)]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            value: CardValue::Text("错误".into()),
            subtitle: None,
            tooltip: Some(message.into()),
            state: MetricState::Error,
            cached: false,
            metadata: None,
        }
    }

    #[allow(dead_code)]
    pub fn loading() -> Self {
        Self {
            value: CardValue::Text("等待中...".into()),
            subtitle: None,
            tooltip: None,
            state: MetricState::Loading,
            cached: false,
            metadata: None,
        }
    }

    #[allow(dead_code)]
    pub fn text(value: impl Into<String>) -> Self {
        Self {
            value: CardValue::Text(value.into()),
            subtitle: None,
            tooltip: None,
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        }
    }

    #[allow(dead_code)]
    pub fn percentage(value: f64) -> Self {
        Self {
            value: CardValue::Percentage(value.clamp(0.0, 100.0)),
            subtitle: None,
            tooltip: None,
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        }
    }
}
