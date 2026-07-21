use crate::core::config::ParserConfig;
use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

pub struct HttpMetric {
    url: String,
    method: String,
    headers: std::collections::HashMap<String, String>,
    body: Option<String>,
    timeout_secs: u64,
    parser: Option<ParserConfig>,
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
        Self {
            url,
            method: method.unwrap_or_else(|| "GET".to_string()),
            headers: headers.unwrap_or_default(),
            timeout_secs,
            body,
            parser,
            max_output_bytes,
        }
    }

    pub fn collect(&mut self, ctx: &MetricContext) -> MetricResult {
        let result = ctx.runtime.block_on(http_fetch(
            &ctx.http_client,
            &self.url,
            &self.method,
            &self.headers,
            self.body.as_deref(),
            self.timeout_secs,
            self.max_output_bytes,
        ));

        match result {
            Ok(body) => {
                if let Some(ref parser) = self.parser {
                    parse_response(&body, parser)
                } else {
                    MetricResult {
                        value: CardValue::Text(body.trim().to_string()),
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
                tooltip: Some(format!("HTTP 请求失败: {}", e)),
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
        .map_err(|e| format!("请求失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    if resp
        .content_length()
        .is_some_and(|length| length > max_output_bytes as u64)
    {
        return Err(format!("响应超过 {} 字节限制", max_output_bytes));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;
    if bytes.len() > max_output_bytes {
        return Err(format!("响应超过 {} 字节限制", max_output_bytes));
    }
    String::from_utf8(bytes.to_vec()).map_err(|e| format!("响应不是有效 UTF-8: {e}"))
}

fn parse_response(body: &str, parser: &ParserConfig) -> MetricResult {
    match parser.parser_type.as_str() {
        "json_path" => {
            let value: serde_json::Value = match serde_json::from_str(body) {
                Ok(v) => v,
                Err(e) => {
                    return MetricResult {
                        value: CardValue::Text("解析错误".into()),
                        subtitle: None,
                        tooltip: Some(format!("JSON 解析失败: {}", e)),
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
                    return MetricResult {
                        value: CardValue::Percentage(n.clamp(0.0, 100.0)),
                        subtitle: None,
                        tooltip: Some(text.clone()),
                        state: MetricState::Normal,
                        cached: false,
                        metadata: None,
                    };
                }
            }

            MetricResult {
                value: CardValue::Text(text),
                subtitle: None,
                tooltip: None,
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            }
        }
        "regex" => {
            let pattern = match &parser.pattern {
                Some(p) => p,
                None => {
                    return MetricResult {
                        value: CardValue::Text(body.to_string()),
                        subtitle: None,
                        tooltip: None,
                        state: MetricState::Normal,
                        cached: false,
                        metadata: None,
                    }
                }
            };

            let re = match regex::Regex::new(pattern) {
                Ok(r) => r,
                Err(e) => {
                    return MetricResult {
                        value: CardValue::Text("解析错误".into()),
                        subtitle: None,
                        tooltip: Some(format!("正则表达式错误: {}", e)),
                        state: MetricState::Error,
                        cached: false,
                        metadata: None,
                    }
                }
            };

            if let Some(caps) = re.captures(body) {
                let capture_idx = parser.capture.unwrap_or(1);
                if let Some(m) = caps.get(capture_idx) {
                    let text = m.as_str().to_string();
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
        "number" => {
            let multiplier = parser.multiplier.unwrap_or(1.0);
            let divisor = parser.divisor.unwrap_or(1.0);
            let decimals = parser.decimal_places.unwrap_or(1);

            let num = body.trim().parse::<f64>().map(|n| n * multiplier / divisor);

            match num {
                Ok(n) => MetricResult {
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
                Err(_) => MetricResult {
                    value: CardValue::Text("解析错误".into()),
                    subtitle: None,
                    tooltip: Some(format!("无法将 '{}' 解析为数字", body.trim())),
                    state: MetricState::Error,
                    cached: false,
                    metadata: None,
                },
            }
        }
        "first_line" => {
            let line = body.lines().next().unwrap_or("").to_string();
            let suffix = parser.suffix.as_deref().unwrap_or("");
            MetricResult {
                value: CardValue::Text(format!("{}{}", line, suffix)),
                subtitle: None,
                tooltip: None,
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            }
        }
        _ => MetricResult {
            value: CardValue::Text(body.to_string()),
            subtitle: None,
            tooltip: Some(format!("未知解析器类型: {}", parser.parser_type)),
            state: MetricState::Error,
            cached: false,
            metadata: None,
        },
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
