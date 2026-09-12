use crate::core::text::{
    bounded_lines, bounded_tail, bounded_text, MAX_UI_ERROR_BYTES, MAX_UI_TEXT_BYTES,
};
use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
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

    pub async fn collect_async(
        &self,
        global_max_output: usize,
        cancellation: CancellationToken,
    ) -> MetricResult {
        let max_output = self.max_output_bytes.min(global_max_output).max(1);
        let output = crate::execution::subprocess::run_command_with_cancellation(
            &self.program,
            &self.args,
            self.timeout_secs,
            max_output,
            cancellation,
        )
        .await;
        match output {
            Ok(output) if output.success => self.render_success(output.stdout),
            Ok(output) => MetricResult {
                value: CardValue::Text("错误".into()),
                subtitle: Some(format!(
                    "退出码 {}: {}",
                    output.exit_code,
                    command_error_summary(&output.stderr)
                )),
                tooltip: Some(if output.stderr.trim().is_empty() {
                    "命令执行失败".into()
                } else {
                    bounded_tail(output.stderr.trim(), MAX_UI_ERROR_BYTES)
                }),
                state: MetricState::Error,
                cached: false,
                metadata: None,
            },
            Err(error) => MetricResult {
                value: CardValue::Text("错误".into()),
                subtitle: Some(bounded_text(&error, MAX_UI_ERROR_BYTES)),
                tooltip: Some(bounded_text(&error, MAX_UI_ERROR_BYTES)),
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
        let (first, rest) = trimmed
            .split_once('\n')
            .map_or((trimmed, ""), |(first, rest)| (first, rest));
        let first = bounded_text(first.trim_end_matches('\r'), MAX_UI_TEXT_BYTES);
        let (value, subtitle) = if self.reverse {
            let content = bounded_lines(rest, 0, MAX_UI_TEXT_BYTES);
            (content, Some(first).filter(|value| !value.is_empty()))
        } else {
            let subtitle = (!rest.is_empty())
                .then(|| bounded_lines(rest, self.max_subtitle_lines, MAX_UI_TEXT_BYTES));
            (first, subtitle)
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
}

fn command_error_summary(stderr: &str) -> String {
    let final_line = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("命令执行失败");
    final_line
        .strip_prefix("RuntimeError:")
        .unwrap_or(final_line)
        .trim()
        .chars()
        .take(120)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::command_error_summary;

    #[test]
    fn traceback_is_reduced_to_its_final_message() {
        let traceback =
            "Traceback (most recent call last):\n  File \"card.py\", line 1\nRuntimeError: 网络异常\n";
        assert_eq!(command_error_summary(traceback), "网络异常");
    }
}
