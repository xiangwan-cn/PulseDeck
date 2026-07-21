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
                    metadata: None,
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
            metadata: None,
        }
    }
}
