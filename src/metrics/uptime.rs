use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

pub struct UptimeMetric;

impl UptimeMetric {
    pub fn new() -> Self {
        Self
    }

    pub fn collect(&mut self, ctx: &MetricContext) -> MetricResult {
        let seconds = match ctx.procfs.lock().unwrap().read_uptime() {
            Ok(s) => s,
            Err(e) => {
                return MetricResult {
                    value: CardValue::Text("不可用".into()),
                    subtitle: None,
                    tooltip: Some(format!("读取 /proc/uptime 失败: {}", e)),
                    state: MetricState::Unavailable,
                    cached: false,
                    metadata: None,
                }
            }
        };

        let formatted = format_uptime_chinese(seconds);

        MetricResult {
            value: CardValue::Text(formatted.clone()),
            subtitle: None,
            tooltip: Some(format!("已运行: {}", formatted)),
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        }
    }
}

fn format_uptime_chinese(total_seconds: f64) -> String {
    let secs = total_seconds as u64;
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{}天", days));
    }
    if hours > 0 {
        parts.push(format!("{}小时", hours));
    }
    if minutes > 0 || parts.is_empty() {
        parts.push(format!("{}分钟", minutes));
    }

    parts.join(" ")
}
