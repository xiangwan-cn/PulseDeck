use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

pub struct BatteryCapacityMetric;

impl BatteryCapacityMetric {
    pub fn new() -> Self {
        Self
    }

    pub fn collect(&mut self, ctx: &MetricContext) -> MetricResult {
        let snapshot = match ctx.battery.lock().unwrap().snapshot() {
            Ok(s) => s,
            Err(e) => {
                return MetricResult {
                    value: CardValue::Text("不可用".into()),
                    subtitle: None,
                    tooltip: Some(format!("读取电池状态失败: {}", e)),
                    state: MetricState::Unavailable,
                    cached: false,
                    metadata: Some(serde_json::json!({ "value_level": "normal" })),
                }
            }
        };

        let capacity = snapshot.capacity.clamp(0.0, 100.0);
        let status_text = snapshot.status.as_str();

        MetricResult {
            value: CardValue::Percentage(capacity),
            subtitle: Some(status_text.to_string()),
            tooltip: Some(format!("电池电量: {:.0}% ({})", capacity, status_text)),
            state: MetricState::Normal,
            cached: false,
            metadata: Some(serde_json::json!({ "value_level": value_level(capacity) })),
        }
    }
}

fn value_level(capacity: f64) -> &'static str {
    if capacity >= 80.0 {
        "good"
    } else if capacity <= 20.0 {
        "critical"
    } else {
        "normal"
    }
}

#[cfg(test)]
mod tests {
    use super::value_level;

    #[test]
    fn battery_capacity_uses_high_and_low_value_colors() {
        assert_eq!(value_level(80.0), "good");
        assert_eq!(value_level(50.0), "normal");
        assert_eq!(value_level(20.0), "critical");
    }
}
