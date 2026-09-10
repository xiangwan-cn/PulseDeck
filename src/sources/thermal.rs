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
    readings(root)
        .into_iter()
        .filter(|reading| is_relevant(&reading.kind))
        .map(|reading| reading.celsius)
        .max_by(f64::total_cmp)
}

/// Return the same deterministic sensor selection for the CPU-temperature
/// card: hottest relevant sensor first, then the first available sensor.
pub fn select_cpu_temperature(root: &Path) -> Option<ThermalReading> {
    let readings = readings(root);
    readings
        .iter()
        .filter(|reading| is_relevant(&reading.kind))
        .max_by(|left, right| left.celsius.total_cmp(&right.celsius))
        .cloned()
        .or_else(|| readings.first().cloned())
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
