use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

pub struct FileMetric {
    path: std::path::PathBuf,
    first_line_only: bool,
}

impl FileMetric {
    pub fn new(path: std::path::PathBuf, first_line_only: bool) -> Self {
        Self {
            path,
            first_line_only,
        }
    }

    pub fn collect(&mut self, _ctx: &MetricContext) -> MetricResult {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(e) => {
                return MetricResult {
                    value: CardValue::Text("不可用".into()),
                    subtitle: None,
                    tooltip: Some(format!("读取文件失败 {}: {}", self.path.display(), e)),
                    state: MetricState::Unavailable,
                    cached: false,
                    metadata: None,
                }
            }
        };

        let text = if self.first_line_only {
            content.lines().next().unwrap_or("").to_string()
        } else {
            content
        };

        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            MetricResult {
                value: CardValue::Empty,
                subtitle: None,
                tooltip: None,
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            }
        } else {
            MetricResult {
                value: CardValue::Text(trimmed),
                subtitle: None,
                tooltip: None,
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            }
        }
    }
}
