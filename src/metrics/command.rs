use crate::execution::subprocess::run_command;
use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

pub struct CommandMetric {
    program: String,
    args: Vec<String>,
    timeout_secs: u64,
    max_output_bytes: usize,
    reverse: bool,
    max_subtitle_lines: usize,
}

impl CommandMetric {
    pub fn new(
        program: String,
        args: Vec<String>,
        timeout_secs: u64,
        max_output_bytes: usize,
        reverse: bool,
        max_subtitle_lines: usize,
    ) -> Self {
        Self {
            program,
            args,
            timeout_secs,
            max_output_bytes,
            reverse,
            max_subtitle_lines,
        }
    }

    pub fn collect_no_ctx(&mut self) -> MetricResult {
        let output = crate::tokio_handle().block_on(run_command(
            &self.program,
            &self.args,
            self.timeout_secs,
            self.max_output_bytes,
        ));
        match output {
            Ok(output) if output.success => self.render_success(output.stdout),
            Ok(output) => MetricResult {
                value: CardValue::Text("错误".into()),
                subtitle: Some(format!(
                    "退出码 {}: {}",
                    output.exit_code,
                    output.stderr.trim()
                )),
                tooltip: Some("命令执行失败".into()),
                state: MetricState::Error,
                cached: false,
                metadata: None,
            },
            Err(error) => MetricResult {
                value: CardValue::Text("错误".into()),
                subtitle: Some(error.clone()),
                tooltip: Some(error),
                state: MetricState::Error,
                cached: false,
                metadata: None,
            },
        }
    }

    fn render_success(&self, stdout: String) -> MetricResult {
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            return MetricResult {
                value: CardValue::Text("无输出".into()),
                subtitle: None,
                tooltip: Some("命令输出为空".into()),
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            };
        }
        let mut lines = trimmed.lines();
        let first = lines.next().unwrap_or_default();
        let rest: Vec<&str> = lines.collect();
        let (value, subtitle) = if self.reverse {
            let content = rest.join("\n");
            (
                content,
                Some(first.to_owned()).filter(|value| !value.is_empty()),
            )
        } else {
            let count = if self.max_subtitle_lines == 0 {
                rest.len()
            } else {
                rest.len().min(self.max_subtitle_lines)
            };
            let subtitle = (count > 0).then(|| rest[..count].join("\n"));
            (first.to_owned(), subtitle)
        };
        MetricResult {
            value: CardValue::Text(value),
            subtitle,
            tooltip: None,
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        }
    }

    pub fn collect(&mut self, _ctx: &MetricContext) -> MetricResult {
        self.collect_no_ctx()
    }
}
