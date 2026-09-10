use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};
use std::io::Read;

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

    pub fn collect(&mut self, _ctx: &MetricContext, max_output: usize) -> MetricResult {
        let limit = max_output.max(1);
        let file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
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
        let mut bytes = Vec::with_capacity(limit.min(8192).saturating_add(1));
        if let Err(error) = file
            .take((limit as u64).saturating_add(1))
            .read_to_end(&mut bytes)
        {
            return MetricResult {
                value: CardValue::Text("不可用".into()),
                subtitle: None,
                tooltip: Some(format!("读取文件失败 {}: {}", self.path.display(), error)),
                state: MetricState::Unavailable,
                cached: false,
                metadata: None,
            };
        }
        if bytes.len() > limit {
            return MetricResult::error(format!(
                "文件内容超过 {} 字节限制: {}",
                limit,
                self.path.display()
            ));
        }
        let content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(error) => {
                return MetricResult::error(format!(
                    "文件不是有效 UTF-8 {}: {}",
                    self.path.display(),
                    error
                ));
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
