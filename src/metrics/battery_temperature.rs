use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

pub struct BatteryTemperatureMetric;

impl BatteryTemperatureMetric {
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

        let temp = match snapshot.temperature {
            Some(t) => t,
            None => {
                return MetricResult {
                    value: CardValue::Text("不可用".into()),
                    subtitle: None,
                    tooltip: Some("电池温度不可用".into()),
                    state: MetricState::Unavailable,
                    cached: false,
                    metadata: None,
                }
            }
        };

        MetricResult {
            value: CardValue::Number {
                value: temp,
                unit: Some("°C".to_string()),
                decimals: 1,
            },
            subtitle: None,
            tooltip: Some(format!("电池温度: {:.1}°C", temp)),
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        }
    }
}
