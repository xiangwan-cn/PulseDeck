use std::process::Command;

use crate::model::card_model::{CardValue, StatusLevel};
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

pub struct NetworkMetric;

impl NetworkMetric {
    pub fn new() -> Self {
        Self
    }

    pub fn collect(&mut self, _ctx: &MetricContext) -> MetricResult {
        let connectivity = get_connectivity();
        let ip_info = get_primary_ip();
        let wifi_name = get_active_connection_name();

        let (state_label, level) = match connectivity.as_deref() {
            Some("full") => ("已连接", StatusLevel::Good),
            Some("portal") => ("需登录", StatusLevel::Warning),
            Some("limited") => ("受限", StatusLevel::Warning),
            Some("none") => ("未连接", StatusLevel::Critical),
            Some("unknown") => ("未知", StatusLevel::Unknown),
            _ => ("未知", StatusLevel::Unknown),
        };

        let ip_clone = ip_info.clone();
        let wifi_clone = wifi_name.clone();

        let (value, subtitle) = if level == StatusLevel::Good {
            if !ip_info.is_empty() {
                (
                    CardValue::Text(ip_info.clone()),
                    Some(format!("{} · {}", wifi_name, state_label)),
                )
            } else if !wifi_name.is_empty() {
                (
                    CardValue::Text(state_label.to_string()),
                    Some(wifi_name.clone()),
                )
            } else {
                (
                    CardValue::Status {
                        label: state_label.to_string(),
                        level,
                    },
                    None,
                )
            }
        } else {
            (
                CardValue::Status {
                    label: state_label.to_string(),
                    level,
                },
                None,
            )
        };

        MetricResult {
            value,
            subtitle,
            tooltip: Some(format!(
                "连通性: {} · 连接: {} · IP: {}",
                connectivity.as_deref().unwrap_or("未知"),
                wifi_clone,
                ip_clone
            )),
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        }
    }
}

fn get_connectivity() -> Option<String> {
    let output = Command::new("nmcli")
        .args(["-t", "-f", "STATE,CONNECTIVITY", "general"])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("connectivity:") {
            return Some(rest.trim().to_string());
        }
        let parts: Vec<&str> = line.splitn(2, ':').collect();
        if parts.len() == 2 && parts[1].trim().len() < 20 {
            return Some(parts[1].trim().to_string());
        }
    }
    Some("unknown".to_string())
}

fn get_active_connection_name() -> String {
    let output = match Command::new("nmcli")
        .args(["-t", "-f", "NAME,TYPE", "connection", "show", "--active"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return String::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(2, ':').collect();
        if parts.len() == 2 {
            let name = parts[0].trim();
            let ctype = parts[1].trim();
            if ctype != "loopback" && !name.is_empty() && name != "lo" {
                return name.to_string();
            }
        }
    }
    String::new()
}

fn get_primary_ip() -> String {
    let output = match Command::new("nmcli")
        .args(["-t", "-f", "IP4.ADDRESS", "device", "show"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return String::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("IP4.ADDRESS[1]:") {
            let ip = rest.trim();
            if !ip.is_empty() && ip != "--" {
                if let Some(slash_pos) = ip.find('/') {
                    return ip[..slash_pos].to_string();
                }
                return ip.to_string();
            }
        }
    }

    String::new()
}
