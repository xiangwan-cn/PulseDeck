use crate::core::config::{ParserConfig, ParserKind};
use crate::core::text::{bounded_text, MAX_UI_ERROR_BYTES, MAX_UI_TEXT_BYTES};
use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

#[derive(Clone)]
pub struct HttpMetric {
    url: String,
    method: String,
    headers: std::collections::HashMap<String, String>,
    body: Option<String>,
    timeout_secs: u64,
    parser: Option<ParserConfig>,
    compiled_regex: Option<Result<regex::Regex, String>>,
    max_output_bytes: usize,
}

impl HttpMetric {
    pub fn new(
        url: String,
        method: Option<String>,
        headers: Option<std::collections::HashMap<String, String>>,
        body: Option<String>,
        timeout_secs: u64,
        parser: Option<ParserConfig>,
        max_output_bytes: usize,
    ) -> Self {
        let compiled_regex = parser
            .as_ref()
            .filter(|parser| parser.parser_type == ParserKind::Regex)
            .and_then(|parser| parser.pattern.as_ref())
            .map(|pattern| regex::Regex::new(pattern).map_err(|error| error.to_string()));
        Self {
            url,
            method: method.unwrap_or_else(|| "GET".to_string()),
            headers: headers.unwrap_or_default(),
            timeout_secs,
            body,
            parser,
            compiled_regex,
            max_output_bytes,
        }
    }

    /// Collect without entering Tokio's blocking pool. HTTP I/O is already
    /// asynchronous; wrapping it in `spawn_blocking + block_on` would consume
    /// one of the very small local blocking slots while the socket waits.
    pub async fn collect_async(
        &self,
        ctx: &MetricContext,
        global_max_output: usize,
    ) -> MetricResult {
        let max_output = self.max_output_bytes.min(global_max_output).max(1);
        let cancellation = ctx.cancellation.clone();
        let result = tokio::select! {
            result = http_fetch(
                &ctx.http_client,
                &self.url,
                &self.method,
                &self.headers,
                self.body.as_deref(),
                self.timeout_secs,
                max_output,
            ) => result,
            _ = cancellation.cancelled() => Err("HTTP 请求因应用关闭而取消".to_string()),
        };

        match result {
            Ok(body) => {
                if let Some(ref parser) = self.parser {
                    parse_response(&body, parser, self.compiled_regex.as_ref())
                } else {
                    MetricResult {
                        value: CardValue::Text(bounded_text(body.trim(), MAX_UI_TEXT_BYTES)),
                        subtitle: None,
                        tooltip: None,
                        state: MetricState::Normal,
                        cached: false,
                        metadata: None,
                    }
                }
            }
            Err(e) => MetricResult {
                value: CardValue::Text("错误".into()),
                subtitle: None,
                tooltip: Some(bounded_text(
                    &format!("HTTP 请求失败: {e}"),
                    MAX_UI_ERROR_BYTES,
                )),
                state: MetricState::Error,
                cached: false,
                metadata: None,
            },
        }
    }
}

async fn http_fetch(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    headers: &std::collections::HashMap<String, String>,
    body: Option<&str>,
    timeout_secs: u64,
    max_output_bytes: usize,
) -> Result<String, String> {
    crate::core::power_debug::increment(crate::core::power_debug::Counter::HttpRequest);
    let mut req = match method.to_uppercase().as_str() {
        "GET" => client.get(url),
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        "PATCH" => client.patch(url),
        _ => return Err(format!("不支持的 HTTP 方法: {}", method)),
    };

    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }

    if let Some(b) = body {
        req = req.body(b.to_string());
    }

    let resp = req
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .send()
        .await
        .map_err(|error| format!("请求失败 ({})", reqwest_error_kind(&error)))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    if resp
        .content_length()
        .is_some_and(|length| length > max_output_bytes as u64)
    {
        return Err(format!("响应超过 {} 字节限制", max_output_bytes));
    }

    let bytes = response_bytes_limited(resp, max_output_bytes).await?;
    String::from_utf8(bytes).map_err(|_| "响应不是有效 UTF-8".to_string())
}

async fn response_bytes_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let limit = limit.max(1);
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .map_or(64 * 1024, |length| length.min(limit as u64) as usize),
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("读取响应失败 ({})", reqwest_error_kind(&error)))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(format!("响应超过 {} 字节限制", limit));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_response(
    body: &str,
    parser: &ParserConfig,
    compiled_regex: Option<&Result<regex::Regex, String>>,
) -> MetricResult {
    match parser.parser_type {
        ParserKind::JsonPath => {
            let value: serde_json::Value = match serde_json::from_str(body) {
                Ok(v) => v,
                Err(e) => {
                    return MetricResult {
                        value: CardValue::Text("解析错误".into()),
                        subtitle: None,
                        tooltip: Some(bounded_text(
                            &format!("JSON 解析失败: {e}"),
                            MAX_UI_ERROR_BYTES,
                        )),
                        state: MetricState::Error,
                        cached: false,
                        metadata: None,
                    }
                }
            };

            let extracted = if let Some(ref path) = parser.path {
                extract_json_path(&value, path)
            } else {
                value.to_string()
            };

            let text = match serde_json::from_str::<serde_json::Value>(&extracted) {
                Ok(serde_json::Value::String(s)) => s,
                Ok(v) => v.to_string(),
                Err(_) => extracted,
            };

            if parser.as_percentage.unwrap_or(false) {
                if let Ok(n) = text.parse::<f64>() {
                    if !n.is_finite() {
                        return MetricResult {
                            value: CardValue::Text("解析错误".into()),
                            subtitle: None,
                            tooltip: Some("百分比不是有限数值".into()),
                            state: MetricState::Error,
                            cached: false,
                            metadata: None,
                        };
                    }
                    return MetricResult {
                        value: CardValue::Percentage(n.clamp(0.0, 100.0)),
                        subtitle: None,
                        tooltip: Some(bounded_text(&text, MAX_UI_TEXT_BYTES)),
                        state: MetricState::Normal,
                        cached: false,
                        metadata: None,
                    };
                }
            }

            MetricResult {
                value: CardValue::Text(bounded_text(&text, MAX_UI_TEXT_BYTES)),
                subtitle: None,
                tooltip: None,
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            }
        }
        ParserKind::Regex => {
            let pattern = match &parser.pattern {
                Some(p) => p,
                None => {
                    return MetricResult {
                        value: CardValue::Text(bounded_text(body, MAX_UI_TEXT_BYTES)),
                        subtitle: None,
                        tooltip: None,
                        state: MetricState::Normal,
                        cached: false,
                        metadata: None,
                    }
                }
            };

            let fallback;
            let re = match compiled_regex {
                Some(Ok(regex)) => regex,
                Some(Err(error)) => {
                    return MetricResult {
                        value: CardValue::Text("解析错误".into()),
                        subtitle: None,
                        tooltip: Some(bounded_text(
                            &format!("正则表达式错误: {error}"),
                            MAX_UI_ERROR_BYTES,
                        )),
                        state: MetricState::Error,
                        cached: false,
                        metadata: None,
                    }
                }
                None => {
                    fallback = match regex::Regex::new(pattern) {
                        Ok(regex) => regex,
                        Err(error) => {
                            return MetricResult {
                                value: CardValue::Text("解析错误".into()),
                                subtitle: None,
                                tooltip: Some(bounded_text(
                                    &format!("正则表达式错误: {error}"),
                                    MAX_UI_ERROR_BYTES,
                                )),
                                state: MetricState::Error,
                                cached: false,
                                metadata: None,
                            }
                        }
                    };
                    &fallback
                }
            };

            if let Some(caps) = re.captures(body) {
                let capture_idx = parser.capture.unwrap_or(1);
                if let Some(m) = caps.get(capture_idx) {
                    let text = bounded_text(m.as_str(), MAX_UI_TEXT_BYTES);
                    return MetricResult {
                        value: CardValue::Text(text),
                        subtitle: None,
                        tooltip: None,
                        state: MetricState::Normal,
                        cached: false,
                        metadata: None,
                    };
                }
            }

            MetricResult {
                value: CardValue::Text("无匹配".into()),
                subtitle: None,
                tooltip: Some("正则表达式无匹配".into()),
                state: MetricState::Error,
                cached: false,
                metadata: None,
            }
        }
        ParserKind::Number => {
            let multiplier = parser.multiplier.unwrap_or(1.0);
            let divisor = parser.divisor.unwrap_or(1.0);
            let decimals = parser.decimal_places.unwrap_or(1);

            let num = body.trim().parse::<f64>().map(|n| n * multiplier / divisor);

            match num {
                Ok(n) if n.is_finite() => MetricResult {
                    value: CardValue::Number {
                        value: n,
                        unit: parser.suffix.clone(),
                        decimals,
                    },
                    subtitle: None,
                    tooltip: None,
                    state: MetricState::Normal,
                    cached: false,
                    metadata: None,
                },
                _ => MetricResult {
                    value: CardValue::Text("解析错误".into()),
                    subtitle: None,
                    tooltip: Some(bounded_text(
                        &format!(
                            "无法将 '{}' 解析为数字",
                            bounded_text(body.trim(), MAX_UI_ERROR_BYTES)
                        ),
                        MAX_UI_ERROR_BYTES,
                    )),
                    state: MetricState::Error,
                    cached: false,
                    metadata: None,
                },
            }
        }
        ParserKind::FirstLine => {
            let line = body.lines().next().unwrap_or("");
            let suffix = parser.suffix.as_deref().unwrap_or("");
            MetricResult {
                value: CardValue::Text(bounded_text(&format!("{line}{suffix}"), MAX_UI_TEXT_BYTES)),
                subtitle: None,
                tooltip: None,
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            }
        }
    }
}

fn reqwest_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_decode() {
        "decode"
    } else {
        "request"
    }
}

fn extract_json_path(value: &serde_json::Value, path: &str) -> String {
    let segments: Vec<&str> = path.trim_matches('.').split('.').collect();
    let mut current = value;

    for seg in segments {
        current = match current {
            serde_json::Value::Object(map) => {
                if let Some(v) = map.get(seg) {
                    v
                } else {
                    return format!("null (key '{}' not found)", seg);
                }
            }
            serde_json::Value::Array(arr) => {
                if let Ok(idx) = seg.parse::<usize>() {
                    if let Some(v) = arr.get(idx) {
                        v
                    } else {
                        return format!("null (index {} out of bounds)", idx);
                    }
                } else {
                    return "null (expected array index)".to_string();
                }
            }
            _ => return current.to_string(),
        };
    }

    current.to_string()
}
