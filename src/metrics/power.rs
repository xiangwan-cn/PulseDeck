use std::collections::VecDeque;

use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};
use crate::sources::battery::{BatterySnapshot, BatteryStatus};

use super::traits::MetricContext;

const DEFAULT_WINDOW_SECS: f64 = 30.0;
const SUSPEND_GAP_SECS: f64 = 120.0;
const MAX_WINDOW_SIZE: usize = 60;

#[derive(Debug, Clone)]
struct PowerSample {
    elapsed_secs: f64,
    power_w: f64,
    is_discharging: bool,
}

pub struct PowerMetric {
    samples: VecDeque<PowerSample>,
    window_secs: f64,
    anchor: std::time::Instant,
    last_anchor_secs: f64,
    prev_status: Option<BatteryStatus>,
}

impl PowerMetric {
    pub fn new(window_secs: Option<f64>) -> Self {
        Self {
            samples: VecDeque::with_capacity(MAX_WINDOW_SIZE),
            window_secs: window_secs.unwrap_or(DEFAULT_WINDOW_SECS),
            anchor: std::time::Instant::now(),
            last_anchor_secs: 0.0,
            prev_status: None,
        }
    }

    pub fn collect(&mut self, ctx: &MetricContext) -> MetricResult {
        let snapshot = match ctx.battery.lock().unwrap().snapshot() {
            Ok(s) => s,
            Err(e) => {
                return MetricResult::error(format!("读取电池状态失败: {}", e));
            }
        };

        let now_secs = self.anchor.elapsed().as_secs_f64();
        let status = snapshot.status;

        let power = snapshot.power_now.unwrap_or(0.0);

        if !power.is_finite() || power.abs() > 500.0 {
            return existing_average_result(&self.samples, status);
        }

        let is_discharging = match status {
            BatteryStatus::Charging => false,
            BatteryStatus::Discharging => true,
            BatteryStatus::Full | BatteryStatus::NotCharging | BatteryStatus::Unknown => {
                self.samples.clear();
                self.prev_status = Some(status);
                self.last_anchor_secs = now_secs;
                return MetricResult {
                    value: CardValue::Number {
                        value: 0.0,
                        unit: Some("W".to_string()),
                        decimals: 1,
                    },
                    subtitle: Some(status.as_str().to_string()),
                    tooltip: Some(status.as_str().to_string()),
                    state: MetricState::Normal,
                    cached: false,
                    metadata: None,
                };
            }
        };

        if let Some(ps) = self.prev_status {
            if ps != status {
                self.samples.clear();
            }
        }

        let elapsed = now_secs - self.last_anchor_secs;
        if self.last_anchor_secs > 0.0 && elapsed > SUSPEND_GAP_SECS {
            self.samples.clear();
        }

        self.samples.push_back(PowerSample {
            elapsed_secs: now_secs,
            power_w: power.abs(),
            is_discharging,
        });

        while self.samples.len() > MAX_WINDOW_SIZE {
            self.samples.pop_front();
        }

        let cutoff = now_secs - self.window_secs;
        while self.samples.len() > 1 {
            if self
                .samples
                .front()
                .map(|s| s.elapsed_secs)
                .unwrap_or(now_secs)
                < cutoff
            {
                self.samples.pop_front();
            } else {
                break;
            }
        }

        self.prev_status = Some(status);
        self.last_anchor_secs = now_secs;

        if self.samples.len() < 2 {
            return MetricResult {
                value: CardValue::Number {
                    value: power.abs(),
                    unit: Some("W".to_string()),
                    decimals: 1,
                },
                subtitle: Some(
                    if is_discharging {
                        "放电中 (采样中)"
                    } else {
                        "充电中 (采样中)"
                    }
                    .to_string(),
                ),
                tooltip: Some(format!("瞬时功耗: {:.1} W", power.abs())),
                state: MetricState::Loading,
                cached: false,
                metadata: None,
            };
        }

        let avg_power = trapezoidal_average(&self.samples);

        if is_discharging {
            let remaining = estimate_remaining(&snapshot, avg_power);
            MetricResult {
                value: CardValue::Number {
                    value: avg_power,
                    unit: Some("W".to_string()),
                    decimals: 1,
                },
                subtitle: Some(format!("放电 · 预计剩余 {remaining}")),
                tooltip: Some(format!(
                    "平均放电功耗: {:.1} W · 剩余: {}",
                    avg_power, remaining
                )),
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            }
        } else {
            let to_full = estimate_time_to_full(&snapshot, avg_power);
            let subtitle = if !to_full.is_empty() {
                format!("充电 · 预计充满 {to_full}")
            } else {
                "充电中".into()
            };
            MetricResult {
                value: CardValue::Number {
                    value: avg_power,
                    unit: Some("W".to_string()),
                    decimals: 1,
                },
                subtitle: Some(subtitle),
                tooltip: Some(format!("平均充电功耗: {:.1} W", avg_power)),
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            }
        }
    }
}

fn existing_average_result(samples: &VecDeque<PowerSample>, status: BatteryStatus) -> MetricResult {
    if samples.len() >= 2 {
        let avg = trapezoidal_average(samples);
        MetricResult {
            value: CardValue::Number {
                value: avg,
                unit: Some("W".to_string()),
                decimals: 1,
            },
            subtitle: Some(status.as_str().to_string()),
            tooltip: Some(format!("{} (缓存)", status.as_str())),
            state: MetricState::Stale,
            cached: true,
            metadata: None,
        }
    } else {
        MetricResult {
            value: CardValue::Text(status.as_str().to_string()),
            subtitle: Some("0.0 W".to_string()),
            tooltip: Some("无功率数据".into()),
            state: MetricState::Unavailable,
            cached: false,
            metadata: None,
        }
    }
}

fn trapezoidal_average(samples: &VecDeque<PowerSample>) -> f64 {
    if samples.len() < 2 {
        return samples.front().map(|s| s.power_w).unwrap_or(0.0);
    }

    let first = samples.front().unwrap();
    let last = samples.back().unwrap();
    let total_duration = last.elapsed_secs - first.elapsed_secs;

    if total_duration <= 0.0 {
        let sum: f64 = samples.iter().map(|s| s.power_w).sum();
        return sum / samples.len() as f64;
    }

    let mut integral = 0.0;
    let iter: Vec<&PowerSample> = samples.iter().collect();
    for w in iter.windows(2) {
        let dt = w[1].elapsed_secs - w[0].elapsed_secs;
        let avg = (w[0].power_w + w[1].power_w) / 2.0;
        integral += avg * dt;
    }

    integral / total_duration
}

fn estimate_remaining(snapshot: &BatterySnapshot, avg_power: f64) -> String {
    if let Some(secs) = snapshot.time_to_empty_now {
        if secs > 0.0 && secs < 172800.0 {
            return format_duration(secs as u64);
        }
    }

    if avg_power > 0.01 && snapshot.energy_now > 0.0 {
        let hours = snapshot.energy_now / avg_power;
        if hours > 0.0 && hours < 48.0 {
            return format_duration((hours * 3600.0) as u64);
        }
    }

    if avg_power > 0.01 {
        let pct = snapshot.capacity.clamp(1.0, 100.0);
        let wh = snapshot.energy_now.max(1.0);
        let total_hours = (wh / avg_power) / (pct / 100.0);
        let remaining_hours = total_hours * (pct / 100.0);
        if remaining_hours > 0.0 && remaining_hours < 48.0 {
            return format_duration((remaining_hours * 3600.0) as u64);
        }
    }

    "计算中...".to_string()
}

fn format_duration(seconds: u64) -> String {
    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    if h > 0 {
        format!("{}h {}m", h, m)
    } else if m > 0 {
        format!("{}m", m)
    } else {
        format!("{}s", seconds)
    }
}

fn estimate_time_to_full(snapshot: &BatterySnapshot, avg_power: f64) -> String {
    if avg_power < 0.01 {
        return String::new();
    }

    if snapshot.capacity >= 99.0 {
        return "即将充满".to_string();
    }

    let remaining_pct = (100.0 - snapshot.capacity).clamp(0.0, 100.0);

    if let Some(charge_full) = snapshot.charge_full {
        if let Some(charge_now) = snapshot.charge_now {
            let remaining_ah = (charge_full - charge_now).max(0.0);
            if let Some(voltage) = snapshot.voltage_now {
                let remaining_wh = remaining_ah * voltage;
                let hours = remaining_wh / avg_power;
                if hours > 0.0 && hours < 48.0 {
                    return format!("充满约{}", format_duration((hours * 3600.0) as u64));
                }
            }
        }
    }

    if let Some(energy_full) = snapshot.energy_full {
        let energy_now = snapshot.energy_now;
        let remaining_wh = (energy_full - energy_now).max(0.0);
        let hours = remaining_wh / avg_power;
        if hours > 0.0 && hours < 48.0 {
            return format!("充满约{}", format_duration((hours * 3600.0) as u64));
        }
    }

    if remaining_pct > 0.0 {
        let total_wh = snapshot.energy_now / (snapshot.capacity / 100.0).max(0.01);
        let remaining_wh = total_wh * (remaining_pct / 100.0);
        let hours = remaining_wh / avg_power;
        if hours > 0.0 && hours < 48.0 {
            return format!("充满约{}", format_duration((hours * 3600.0) as u64));
        }
    }

    String::new()
}
