use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct ThermalReading {
    pub kind: String,
    pub celsius: f64,
}

/// Read thermal-zone sensors once and normalize the common millidegree,
/// decidegree, and degree representations at the source boundary.
pub fn readings(root: &Path) -> Vec<ThermalReading> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("thermal_zone")
        })
        .filter_map(|entry| {
            let path = entry.path();
            let kind = fs::read_to_string(path.join("type"))
                .ok()?
                .trim()
                .to_owned();
            let raw = fs::read_to_string(path.join("temp"))
                .ok()?
                .trim()
                .parse::<f64>()
                .ok()?;
            let celsius = normalize_temperature(raw);
            celsius
                .is_finite()
                .then_some(ThermalReading { kind, celsius })
        })
        .collect()
}

/// Return the hottest CPU/SoC/package sensor for safety policy evaluation.
pub fn hottest_relevant(root: &Path) -> Option<f64> {
    select_cpu_temperature_with_hwmon(root, &root.with_file_name("hwmon"))
        .map(|reading| reading.celsius)
}

/// Return the hottest relevant sensor, never an unrelated Wi-Fi/GPU sensor.
pub fn select_cpu_temperature(root: &Path) -> Option<ThermalReading> {
    readings(root)
        .into_iter()
        .filter(|reading| is_relevant(&reading.kind))
        .max_by(|left, right| left.celsius.total_cmp(&right.celsius))
}

/// Include CPU hwmon sensors on AMD/Intel machines whose thermal zones only
/// contain peripheral temperatures. Both the card and safety policy use this.
pub fn select_cpu_temperature_with_hwmon(
    thermal_root: &Path,
    hwmon_root: &Path,
) -> Option<ThermalReading> {
    select_cpu_temperature(thermal_root)
        .into_iter()
        .chain(cpu_hwmon_readings(hwmon_root))
        .max_by(|left, right| left.celsius.total_cmp(&right.celsius))
}

fn cpu_hwmon_readings(root: &Path) -> Vec<ThermalReading> {
    let Ok(devices) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for device in devices.flatten() {
        let path = device.path();
        let Ok(name) = fs::read_to_string(path.join("name")) else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        // A known CPU driver is required. In particular, amdgpu, nvme and
        // iwlwifi temperatures must not be relabelled as CPU temperatures.
        if !matches!(
            name.as_str(),
            "k10temp" | "k8temp" | "coretemp" | "zenpower" | "cpu_thermal"
        ) {
            continue;
        }
        let Ok(attributes) = fs::read_dir(&path) else {
            continue;
        };
        let mut device_readings = Vec::new();
        for attribute in attributes.flatten() {
            let filename = attribute.file_name();
            let filename = filename.to_string_lossy();
            let Some(index) = filename
                .strip_prefix("temp")
                .and_then(|s| s.strip_suffix("_input"))
            else {
                continue;
            };
            if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let label = fs::read_to_string(path.join(format!("temp{index}_label")))
                .unwrap_or_else(|_| format!("temp{index}"));
            let Ok(raw) = fs::read_to_string(attribute.path()) else {
                continue;
            };
            let Ok(raw) = raw.trim().parse::<f64>() else {
                continue;
            };
            // hwmon temp*_input is always millidegrees Celsius.
            let celsius = raw / 1000.0;
            if celsius.is_finite() && (-40.0..=150.0).contains(&celsius) {
                device_readings.push(ThermalReading {
                    kind: format!("{name} {}", label.trim()),
                    celsius,
                });
            }
        }
        // Tctl may include an offset on AMD CPUs; prefer physical die/core
        // readings when present, while retaining Tctl on Tctl-only devices.
        let has_die_reading = device_readings.iter().any(|reading| {
            let label = reading.kind.to_ascii_lowercase();
            label.ends_with("tdie") || label.contains("tccd")
        });
        result.extend(device_readings.into_iter().filter(|reading| {
            !has_die_reading || !reading.kind.to_ascii_lowercase().ends_with("tctl")
        }));
    }
    result
}

fn is_relevant(kind: &str) -> bool {
    let kind = kind.to_ascii_lowercase();
    kind.contains("cpu") || kind.contains("soc") || kind.contains("package")
}

pub fn normalize_temperature(value: f64) -> f64 {
    if value.abs() > 5000.0 {
        value / 1000.0
    } else if value.abs() > 150.0 {
        value / 10.0
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_zone(root: &Path, name: &str, kind: &str, temp: f64) -> std::path::PathBuf {
        let zone = root.join(name);
        fs::create_dir_all(&zone).unwrap();
        fs::write(zone.join("type"), format!("{kind}\n")).unwrap();
        fs::write(zone.join("temp"), format!("{temp}\n")).unwrap();
        zone
    }

    #[test]
    fn normalizes_common_temperature_units() {
        assert_eq!(normalize_temperature(42_000.0), 42.0);
        assert_eq!(normalize_temperature(420.0), 42.0);
        assert_eq!(normalize_temperature(42.0), 42.0);
    }

    #[test]
    fn selects_hottest_relevant_sensor_for_policy_and_card() {
        let root = std::env::temp_dir().join(format!(
            "pulsedeck-thermal-source-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        setup_zone(&root, "thermal_zone0", "cpu-thermal", 71_000.0);
        setup_zone(&root, "thermal_zone1", "soc", 420.0);
        assert_eq!(hottest_relevant(&root), Some(71.0));
        assert_eq!(
            select_cpu_temperature(&root).map(|reading| reading.celsius),
            Some(71.0)
        );
        let _ = fs::remove_dir_all(root);
    }
}
