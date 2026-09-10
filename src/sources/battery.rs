use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    Full,
    NotCharging,
    Unknown,
}

impl BatteryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            BatteryStatus::Charging => "充电中",
            BatteryStatus::Discharging => "放电中",
            BatteryStatus::Full => "已充满",
            BatteryStatus::NotCharging => "未充电",
            BatteryStatus::Unknown => "未知",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BatterySnapshot {
    pub capacity: f64,
    pub status: BatteryStatus,
    pub temperature: Option<f64>,
    pub power_now: Option<f64>,
    pub time_to_empty_now: Option<f64>,
    pub energy_now: f64,
    pub energy_full: Option<f64>,
    pub charge_now: Option<f64>,
    pub charge_full: Option<f64>,
    pub voltage_now: Option<f64>,
}

pub struct BatterySource {
    root: PathBuf,
    battery_path: Option<PathBuf>,
    cached_snapshot: Option<BatterySnapshot>,
    last_read: Instant,
    cache_ttl: Duration,
}

impl BatterySource {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            battery_path: None,
            cached_snapshot: None,
            last_read: Instant::now(),
            cache_ttl: Duration::from_millis(500),
        }
    }

    /// Invalidate the short-lived shared snapshot after a power-supply edge.
    /// The next consumer performs one fresh read instead of receiving the
    /// pre-edge value.
    pub fn invalidate(&mut self) {
        self.cached_snapshot = None;
        self.last_read = Instant::now()
            .checked_sub(self.cache_ttl)
            .unwrap_or_else(Instant::now);
    }

    pub fn snapshot(&mut self) -> Result<BatterySnapshot, anyhow::Error> {
        let now = Instant::now();
        if let Some(ref snap) = self.cached_snapshot {
            if now.duration_since(self.last_read) < self.cache_ttl {
                return Ok(snap.clone());
            }
        }
        let snapshot = self.do_read()?;
        self.cached_snapshot = Some(snapshot.clone());
        self.last_read = now;
        Ok(snapshot)
    }

    fn discover(&self) -> Option<PathBuf> {
        select_battery(&self.root)
    }

    fn ensure_battery(&mut self) -> Option<PathBuf> {
        if let Some(ref path) = self.battery_path {
            if is_usable_battery(path) {
                return Some(path.clone());
            }
        }
        let new_path = self.discover()?;
        self.battery_path = Some(new_path.clone());
        Some(new_path)
    }

    /// Read only the capacity used by policy evaluation. This helper keeps
    /// policy sampling independent from any particular display card.
    pub fn capacity(&mut self) -> Option<f64> {
        self.ensure_battery()
            .and_then(|path| read_capacity(&path))
            .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
    }

    fn do_read(&mut self) -> Result<BatterySnapshot, anyhow::Error> {
        let path = self
            .ensure_battery()
            .ok_or_else(|| anyhow::anyhow!("no battery found in sysfs"))?;

        let capacity = read_capacity(&path).unwrap_or(0.0);

        let status = read_status(&path);

        let temperature = read_temperature(&path);

        let power_now = read_power_w(&path);

        let time_to_empty_now = read_time_to_empty_now(&path);

        let energy_now = read_attr_u64(&path, "energy_now")
            .map(|v| v as f64 / 1_000_000.0)
            .unwrap_or(0.0);

        let energy_full = read_attr_u64(&path, "energy_full").map(|v| v as f64 / 1_000_000.0);

        let charge_now = read_attr_u64(&path, "charge_now").map(|v| v as f64 / 1_000_000.0);

        let charge_full = read_attr_u64(&path, "charge_full").map(|v| v as f64 / 1_000_000.0);

        let voltage_now = read_attr_u64(&path, "voltage_now").map(|v| v as f64 / 1_000_000.0);

        Ok(BatterySnapshot {
            capacity,
            status,
            temperature,
            power_now,
            time_to_empty_now,
            energy_now,
            energy_full,
            charge_now,
            charge_full,
            voltage_now,
        })
    }
}

/// Select the same usable system battery for policy and metric consumers.
/// Explicit `present=0` entries are absent; a missing `present` attribute is
/// accepted because several platforms expose a battery without that file.
pub fn select_battery(root: &Path) -> Option<PathBuf> {
    let mut candidates = fs::read_dir(root)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| is_usable_battery(path))
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next()
}

fn is_usable_battery(path: &Path) -> bool {
    let Ok(kind) = fs::read_to_string(path.join("type")) else {
        return false;
    };
    if kind.trim() != "Battery" {
        return false;
    }
    if fs::read_to_string(path.join("scope"))
        .ok()
        .is_some_and(|scope| scope.trim() == "Device")
    {
        return false;
    }
    match fs::read_to_string(path.join("present")) {
        Ok(present) => present.trim() == "1",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

fn read_attr_u64(path: &Path, name: &str) -> Option<u64> {
    let content = fs::read_to_string(path.join(name)).ok()?;
    content.trim().parse().ok()
}

fn read_capacity(path: &Path) -> Option<f64> {
    if let Some(cap) = read_attr_u64(path, "capacity") {
        return Some(cap as f64);
    }
    if let (Some(now), Some(full)) = (
        read_attr_u64(path, "energy_now"),
        read_attr_u64(path, "energy_full"),
    ) {
        if full > 0 {
            return Some((now as f64 / full as f64) * 100.0);
        }
    }
    if let (Some(now), Some(full)) = (
        read_attr_u64(path, "charge_now"),
        read_attr_u64(path, "charge_full"),
    ) {
        if full > 0 {
            return Some((now as f64 / full as f64) * 100.0);
        }
    }
    None
}

fn read_status(path: &Path) -> BatteryStatus {
    let content = match fs::read_to_string(path.join("status")) {
        Ok(c) => c,
        Err(_) => return BatteryStatus::Unknown,
    };
    match content.trim() {
        "Charging" => BatteryStatus::Charging,
        "Discharging" => BatteryStatus::Discharging,
        "Full" => BatteryStatus::Full,
        "Not charging" => BatteryStatus::NotCharging,
        _ => BatteryStatus::Unknown,
    }
}

fn read_temperature(path: &Path) -> Option<f64> {
    let raw: f64 = fs::read_to_string(path.join("temp"))
        .or_else(|_| fs::read_to_string(path.join("temperature")))
        .ok()?
        .trim()
        .parse()
        .ok()?;

    let celsius = if raw > 5000.0 {
        raw / 1000.0
    } else if raw > 150.0 {
        raw / 10.0
    } else {
        raw
    };

    Some(celsius)
}

fn read_power_w(path: &Path) -> Option<f64> {
    if let Some(p) = read_attr_i64(path, "power_now") {
        return Some(p.unsigned_abs() as f64 / 1_000_000.0);
    }

    let voltage = read_attr_u64(path, "voltage_now").map(|v| v as f64 / 1_000_000.0);
    let current =
        read_attr_i64(path, "current_now").map(|c| (c.unsigned_abs() as f64) / 1_000_000.0);

    if let (Some(v), Some(c)) = (voltage, current) {
        return Some(v * c);
    }

    if let Some(p) = read_attr_i64(path, "power_avg") {
        return Some(p.unsigned_abs() as f64 / 1_000_000.0);
    }

    None
}

fn read_attr_i64(path: &Path, name: &str) -> Option<i64> {
    let content = fs::read_to_string(path.join(name)).ok()?;
    content.trim().parse().ok()
}

fn read_time_to_empty_now(path: &Path) -> Option<f64> {
    if let Some(secs) = read_attr_u64(path, "time_to_empty_now") {
        return Some(secs as f64);
    }
    if let Some(secs) = read_attr_u64(path, "time_to_full_now") {
        return Some(secs as f64);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_sysfs(dir: &Path, name: &str) -> PathBuf {
        let bat = dir.join(name);
        fs::create_dir_all(&bat).unwrap();
        fs::write(bat.join("type"), "Battery\n").unwrap();
        fs::write(bat.join("present"), "1\n").unwrap();
        fs::write(bat.join("status"), "Discharging\n").unwrap();
        fs::write(bat.join("capacity"), "85\n").unwrap();
        fs::write(bat.join("voltage_now"), "11000000\n").unwrap();
        fs::write(bat.join("current_now"), "-500000\n").unwrap();
        fs::write(bat.join("power_now"), "5500000\n").unwrap();
        fs::write(bat.join("energy_now"), "42000000\n").unwrap();
        fs::write(bat.join("temp"), "31000\n").unwrap();
        fs::write(bat.join("time_to_empty_now"), "7200\n").unwrap();
        bat
    }

    #[test]
    fn test_snapshot() {
        let dir = std::env::temp_dir().join("pulsedeck_test_battery_snap");
        let _ = fs::remove_dir_all(&dir);
        setup_sysfs(&dir, "BAT0");

        let mut src = BatterySource::new(dir);
        let snap = src.snapshot().unwrap();
        assert_eq!(snap.capacity, 85.0);
        assert_eq!(snap.status, BatteryStatus::Discharging);
        assert!(snap.temperature.unwrap() > 0.0);
        assert!(snap.power_now.unwrap() > 0.0);
        assert_eq!(snap.time_to_empty_now, Some(7200.0));
        assert!(snap.energy_now > 0.0);
    }

    #[test]
    fn test_cache_window() {
        let dir = std::env::temp_dir().join("pulsedeck_test_battery_cache");
        let _ = fs::remove_dir_all(&dir);
        let battery = setup_sysfs(&dir, "BAT0");

        let mut src = BatterySource::new(dir);
        let s1 = src.snapshot().unwrap();
        fs::write(battery.join("capacity"), "42\n").unwrap();
        // The short source cache is intentionally retained until an edge
        // invalidates it.
        let cached = src.snapshot().unwrap();
        assert_eq!(s1.capacity, cached.capacity);
        src.invalidate();
        assert_eq!(src.snapshot().unwrap().capacity, 42.0);
    }

    #[test]
    fn test_rediscover_on_failure() {
        let dir = std::env::temp_dir().join("pulsedeck_test_battery_rediscover");
        let _ = fs::remove_dir_all(&dir);
        let bat0 = setup_sysfs(&dir, "BAT0");

        let mut src = BatterySource::new(dir.clone());
        assert!(src.snapshot().is_ok());

        fs::remove_dir_all(&bat0).ok();
        src.cached_snapshot = None;

        setup_sysfs(&dir, "BAT1");
        assert!(src.snapshot().is_ok());
    }

    #[test]
    fn signed_power_telemetry_is_reported_as_magnitude() {
        let dir = std::env::temp_dir().join("pulsedeck_test_battery_signed_power");
        let _ = fs::remove_dir_all(&dir);
        let battery = setup_sysfs(&dir, "BAT0");
        fs::write(battery.join("power_now"), "-5500000\n").unwrap();

        let mut src = BatterySource::new(dir);
        assert_eq!(src.snapshot().unwrap().power_now, Some(5.5));
    }

    #[test]
    fn test_excludes_device_scope() {
        let dir = std::env::temp_dir().join("pulsedeck_test_battery_scope");
        let _ = fs::remove_dir_all(&dir);
        let bat = dir.join("BAT0");
        fs::create_dir_all(&bat).unwrap();
        fs::write(bat.join("type"), "Battery\n").unwrap();
        fs::write(bat.join("present"), "1\n").unwrap();
        fs::write(bat.join("scope"), "Device\n").unwrap();

        let mut src = BatterySource::new(dir);
        assert!(src.snapshot().is_err());
    }

    #[test]
    fn test_temperature_unit_detection() {
        let dir = std::env::temp_dir().join("pulsedeck_test_battery_temp_unit");
        let _ = fs::remove_dir_all(&dir);

        for (raw, expected) in [(31000.0, 31.0), (310.0, 31.0), (31.0, 31.0)] {
            let bat_dir = dir.join("BAT");
            let _ = fs::remove_dir_all(&bat_dir);
            fs::create_dir_all(&bat_dir).unwrap();
            fs::write(bat_dir.join("type"), "Battery\n").unwrap();
            fs::write(bat_dir.join("present"), "1\n").unwrap();
            fs::write(bat_dir.join("status"), "Discharging\n").unwrap();
            fs::write(bat_dir.join("capacity"), "50\n").unwrap();
            fs::write(bat_dir.join("temp"), format!("{}\n", raw)).unwrap();

            let mut src = BatterySource::new(dir.clone());
            let snap = src.snapshot().unwrap();
            assert!(
                (snap.temperature.unwrap() - expected).abs() < 0.5,
                "raw={} expected={} got={:?}",
                raw,
                expected,
                snap.temperature
            );
        }
    }

    #[test]
    fn charge_only_battery_feeds_capacity_policy_sample() {
        let dir = std::env::temp_dir().join(format!(
            "pulsedeck_test_battery_charge-only-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        let battery = setup_sysfs(&dir, "BAT0");
        fs::remove_file(battery.join("capacity")).unwrap();
        fs::remove_file(battery.join("energy_now")).unwrap();
        let _ = fs::remove_file(battery.join("energy_full"));
        fs::write(battery.join("charge_now"), "50000000\n").unwrap();
        fs::write(battery.join("charge_full"), "100000000\n").unwrap();

        let mut source = BatterySource::new(dir.clone());
        assert_eq!(source.capacity(), Some(50.0));
        assert_eq!(source.snapshot().unwrap().capacity, 50.0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_absent_batteries_are_not_selected() {
        let dir = std::env::temp_dir().join(format!(
            "pulsedeck_test_battery_absent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let battery = setup_sysfs(&dir, "BAT0");
        fs::write(battery.join("present"), "0\n").unwrap();

        assert_eq!(select_battery(&dir), None);
        let mut source = BatterySource::new(dir.clone());
        assert_eq!(source.capacity(), None);
        assert!(source.snapshot().is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_present_attribute_is_accepted() {
        let dir = std::env::temp_dir().join(format!(
            "pulsedeck_test_battery_no-present-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let battery = setup_sysfs(&dir, "BAT0");
        fs::remove_file(battery.join("present")).unwrap();

        assert_eq!(select_battery(&dir), Some(battery.clone()));
        let mut source = BatterySource::new(dir.clone());
        assert_eq!(source.capacity(), Some(85.0));
        assert_eq!(source.snapshot().unwrap().capacity, 85.0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn multiple_batteries_select_the_present_system_battery() {
        let dir = std::env::temp_dir().join(format!(
            "pulsedeck_test_battery_multiple-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let absent = setup_sysfs(&dir, "BAT0");
        fs::write(absent.join("present"), "0\n").unwrap();
        let present = setup_sysfs(&dir, "BAT1");
        fs::write(present.join("capacity"), "61\n").unwrap();

        assert_eq!(select_battery(&dir), Some(present.clone()));
        let mut source = BatterySource::new(dir.clone());
        assert_eq!(source.capacity(), Some(61.0));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_status_labels() {
        assert_eq!(BatteryStatus::Charging.as_str(), "充电中");
        assert_eq!(BatteryStatus::Discharging.as_str(), "放电中");
        assert_eq!(BatteryStatus::Full.as_str(), "已充满");
        assert_eq!(BatteryStatus::NotCharging.as_str(), "未充电");
        assert_eq!(BatteryStatus::Unknown.as_str(), "未知");
    }
}
