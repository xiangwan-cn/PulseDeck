use chrono::{Duration as ChronoDuration, Local, NaiveTime, TimeZone};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleState {
    /// Stable cache key for the most recent eligible execution slot.
    pub period: Option<String>,
    /// Seconds until the next slot; suitable as the scheduler delay.
    pub next_delay_seconds: u64,
}

/// Evaluate a configuration-driven wall-clock schedule.
///
/// Supported forms:
/// - `daily@08:05,12:05,16:05,20:05`
///
/// A card runs at most once per period when paired with the disk cache. Starting
/// the application after a slot still runs the latest missed slot, which avoids
/// requiring the application to be open at the exact minute.
pub fn evaluate(spec: &str) -> Result<ScheduleState, String> {
    let times = parse_times(spec)?;
    let now = Local::now();
    let today = now.date_naive();

    let mut latest = None;
    let mut next = None;
    for time in &times {
        let candidate = Local
            .from_local_datetime(&today.and_time(*time))
            .single()
            .ok_or_else(|| format!("计划时间在当前时区不唯一: {time}"))?;
        if candidate <= now {
            latest = Some(candidate);
        } else if next.is_none() {
            next = Some(candidate);
        }
    }

    let next = match next {
        Some(value) => value,
        None => {
            let tomorrow = today + ChronoDuration::days(1);
            Local
                .from_local_datetime(&tomorrow.and_time(times[0]))
                .single()
                .ok_or_else(|| "无法计算下一计划时间".to_string())?
        }
    };

    Ok(ScheduleState {
        period: latest.map(|slot| slot.format("%Y-%m-%dT%H:%M").to_string()),
        next_delay_seconds: (next - now).num_seconds().max(1) as u64,
    })
}

fn parse_times(spec: &str) -> Result<Vec<NaiveTime>, String> {
    let values = spec
        .strip_prefix("daily@")
        .ok_or_else(|| format!("不支持的计划格式: {spec}"))?;
    let mut times = values
        .split(',')
        .map(|value| {
            NaiveTime::parse_from_str(value.trim(), "%H:%M")
                .map_err(|_| format!("无效计划时间: {value}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if times.is_empty() {
        return Err("计划至少需要一个时间点".into());
    }
    times.sort_unstable();
    times.dedup();
    Ok(times)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_daily_schedules() {
        assert_eq!(parse_times("daily@20:00,08:00,20:00").unwrap().len(), 2);
        assert!(parse_times("weekly@08:00").is_err());
        assert!(parse_times("daily@25:00").is_err());
    }
}
