use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gio::prelude::*;

use crate::sources::battery::{select_battery, BatterySource};
use crate::sources::thermal;

use super::runtime::{Activity, PowerVerdict, RuntimeHandle, ThermalVerdict, Visibility};

pub struct PowerSupplyMonitor {
    runtime: RuntimeHandle,
    root: PathBuf,
    thermal_root: PathBuf,
    battery: Arc<Mutex<BatterySource>>,
    power_source: RefCell<Option<glib::SourceId>>,
    thermal_source: RefCell<Option<glib::SourceId>>,
    power_event_source: RefCell<Option<glib::SourceId>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    upower_connection: RefCell<Option<gio::DBusConnection>>,
    upower_subscription: RefCell<Option<gio::SignalSubscriptionId>>,
    power_event: Rc<dyn Fn()>,
    thermal_event: Rc<dyn Fn()>,
}

impl PowerSupplyMonitor {
    pub fn start_with_shared_battery(
        runtime: RuntimeHandle,
        root: PathBuf,
        thermal_root: PathBuf,
        battery: Arc<Mutex<BatterySource>>,
        power_event: impl Fn() + 'static,
        thermal_event: impl Fn() + 'static,
    ) -> Rc<Self> {
        let this = Rc::new(Self {
            runtime,
            root,
            thermal_root,
            battery,
            power_source: RefCell::new(None),
            thermal_source: RefCell::new(None),
            power_event_source: RefCell::new(None),
            monitor: RefCell::new(None),
            upower_connection: RefCell::new(None),
            upower_subscription: RefCell::new(None),
            power_event: Rc::new(power_event),
            thermal_event: Rc::new(thermal_event),
        });
        Self::install_monitor(&this);
        Self::install_upower_monitor(&this);
        Self::sample_power_and_schedule(&this);
        Self::sample_thermal_and_schedule(&this);
        this
    }

    fn install_monitor(this: &Rc<Self>) {
        let Ok(monitor) = gio::File::for_path(&this.root)
            .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        else {
            return;
        };
        let weak = Rc::downgrade(this);
        monitor.connect_changed(move |_, _, _, _| {
            if let Some(this) = weak.upgrade() {
                Self::queue_power_sample(&this);
            }
        });
        this.monitor.replace(Some(monitor));
    }

    fn install_upower_monitor(this: &Rc<Self>) {
        let Ok(connection) = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE) else {
            tracing::debug!("UPower system bus unavailable; using sysfs fallback sampling");
            return;
        };
        let weak = Rc::downgrade(this);
        let subscription = connection.signal_subscribe(
            Some("org.freedesktop.UPower"),
            Some("org.freedesktop.DBus.Properties"),
            Some("PropertiesChanged"),
            None,
            Some("org.freedesktop.UPower.Device"),
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, _| {
                if let Some(this) = weak.upgrade() {
                    Self::queue_power_sample(&this);
                }
            },
        );
        this.upower_subscription.replace(Some(subscription));
        this.upower_connection.replace(Some(connection));
    }

    fn queue_power_sample(this: &Rc<Self>) {
        if this.power_event_source.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(this);
        let source = glib::timeout_add_local_once(Duration::from_millis(100), move || {
            if let Some(this) = weak.upgrade() {
                this.power_event_source.borrow_mut().take();
                Self::sample_power_and_schedule(&this);
            }
        });
        this.power_event_source.replace(Some(source));
    }

    fn sample_power_and_schedule(this: &Rc<Self>) {
        // A fallback may fire while a debounced event is waiting. Consume that
        // pending event before sampling so one edge produces one policy update.
        if let Some(source) = this.power_event_source.borrow_mut().take() {
            source.remove();
        }
        if let Some(source) = this.power_source.borrow_mut().take() {
            source.remove();
        }
        let verdict = read_power_source(&this.root);
        let battery_capacity = this
            .battery
            .lock()
            .ok()
            .and_then(|mut battery| battery.capacity());
        this.runtime.set_power_source(verdict);
        this.runtime.set_battery_capacity(battery_capacity);
        (this.power_event)();
        let cfg = this.runtime.config();

        let snapshot = this.runtime.snapshot();
        let seconds = power_fallback_seconds(verdict, snapshot.visibility, snapshot.activity, &cfg);
        let weak = Rc::downgrade(this);
        let source = glib::timeout_add_local_once(Duration::from_secs(seconds), move || {
            if let Some(this) = weak.upgrade() {
                this.power_source.borrow_mut().take();
                Self::sample_power_and_schedule(&this);
            }
        });
        this.power_source.replace(Some(source));
    }

    fn sample_thermal_and_schedule(this: &Rc<Self>) {
        if let Some(source) = this.thermal_source.borrow_mut().take() {
            source.remove();
        }
        let battery_temperature = self_battery_temperature(this);
        this.runtime
            .set_thermal(read_thermal(&this.thermal_root, battery_temperature));
        (this.thermal_event)();
        let cfg = this.runtime.config();
        let snapshot = this.runtime.snapshot();
        let seconds = thermal_fallback_seconds(snapshot.visibility, snapshot.activity, &cfg);
        let weak = Rc::downgrade(this);
        let source = glib::timeout_add_local_once(Duration::from_secs(seconds), move || {
            if let Some(this) = weak.upgrade() {
                this.thermal_source.borrow_mut().take();
                Self::sample_thermal_and_schedule(&this);
            }
        });
        this.thermal_source.replace(Some(source));
    }
}

impl Drop for PowerSupplyMonitor {
    fn drop(&mut self) {
        if let Some(source) = self.power_source.borrow_mut().take() {
            source.remove();
        }
        if let Some(source) = self.thermal_source.borrow_mut().take() {
            source.remove();
        }
        if let Some(source) = self.power_event_source.borrow_mut().take() {
            source.remove();
        }
        if let (Some(connection), Some(subscription)) = (
            self.upower_connection.borrow().as_ref(),
            self.upower_subscription.borrow_mut().take(),
        ) {
            connection.signal_unsubscribe(subscription);
        }
        self.upower_connection.borrow_mut().take();
        self.monitor.borrow_mut().take();
    }
}

const MIN_POWER_FALLBACK_SECONDS: u64 = 15;
const MAX_POWER_FALLBACK_SECONDS: u64 = 300;

fn power_fallback_seconds(
    power: PowerVerdict,
    visibility: Visibility,
    activity: Activity,
    cfg: &crate::core::config::RuntimeConfig,
) -> u64 {
    let _ = (power, visibility, activity);
    // Policy signals retain a bounded fallback regardless of focus, local
    // idle, or whether the window is currently active. Events are a hint, not
    // the sole source of truth.
    cfg.power_sample_seconds
        .clamp(MIN_POWER_FALLBACK_SECONDS, MAX_POWER_FALLBACK_SECONDS)
}

fn thermal_fallback_seconds(
    visibility: Visibility,
    activity: Activity,
    cfg: &crate::core::config::RuntimeConfig,
) -> u64 {
    let _ = (visibility, activity);
    cfg.thermal_sample_seconds.clamp(15, 60)
}

fn read_power_source(root: &Path) -> PowerVerdict {
    let has_battery = select_battery(root).is_some();
    let Ok(entries) = std::fs::read_dir(root) else {
        return PowerVerdict::Unknown;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = read(&path.join("type")).unwrap_or_default();
        if matches!(
            kind.as_str(),
            "Mains" | "USB" | "USB_C" | "USB_PD" | "Wireless"
        ) && read(&path.join("online")).as_deref() == Some("1")
        {
            return PowerVerdict::External;
        }
    }
    if has_battery {
        PowerVerdict::Battery
    } else {
        PowerVerdict::Unknown
    }
}

fn self_battery_temperature(this: &PowerSupplyMonitor) -> Option<f64> {
    this.battery
        .lock()
        .ok()
        .and_then(|mut battery| battery.snapshot().ok())
        .and_then(|snapshot| snapshot.temperature)
}

fn read_thermal(root: &Path, battery_temp: Option<f64>) -> ThermalVerdict {
    if thermal_pressure_active() {
        return ThermalVerdict::Throttled;
    }
    let hottest_soc = thermal::hottest_relevant(root);
    classify_thermal(battery_temp, hottest_soc)
}

fn classify_thermal(battery_temp: Option<f64>, hottest_soc: Option<f64>) -> ThermalVerdict {
    if battery_temp.is_some_and(|temp| temp >= 48.0) || hottest_soc.is_some_and(|temp| temp >= 90.0)
    {
        ThermalVerdict::Hot
    } else if battery_temp.is_some_and(|temp| temp >= 42.0)
        || hottest_soc.is_some_and(|temp| temp >= 80.0)
    {
        ThermalVerdict::Warm
    } else if battery_temp.is_some() || hottest_soc.is_some() {
        ThermalVerdict::Normal
    } else {
        ThermalVerdict::Unknown
    }
}

fn thermal_pressure_active() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu") else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.file_name().to_string_lossy().starts_with("cpu")
            && number_path(&entry.path().join("thermal_pressure")).is_some_and(|value| value > 0.0)
    })
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
}
fn number_path(path: &Path) -> Option<f64> {
    read(path)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::RuntimeConfig;

    #[test]
    fn policy_signal_fallbacks_are_bounded_and_focus_independent() {
        let cfg = RuntimeConfig::default();
        for (power, visibility, activity) in [
            (
                PowerVerdict::External,
                Visibility::MappedActive,
                Activity::Engaged,
            ),
            (
                PowerVerdict::Battery,
                Visibility::MappedActive,
                Activity::Idle,
            ),
            (PowerVerdict::External, Visibility::Unmapped, Activity::Idle),
        ] {
            assert_eq!(
                power_fallback_seconds(power, visibility, activity, &cfg),
                30
            );
        }
        assert_eq!(
            thermal_fallback_seconds(Visibility::MappedActive, Activity::Engaged, &cfg),
            30
        );
        assert_eq!(
            thermal_fallback_seconds(Visibility::MappedInactive, Activity::Idle, &cfg),
            30
        );
        let short = RuntimeConfig {
            power_sample_seconds: 1,
            thermal_sample_seconds: 1,
            ..cfg.clone()
        };
        assert_eq!(
            power_fallback_seconds(
                PowerVerdict::Battery,
                Visibility::Unmapped,
                Activity::Idle,
                &short,
            ),
            MIN_POWER_FALLBACK_SECONDS
        );
        assert_eq!(
            thermal_fallback_seconds(Visibility::Unmapped, Activity::Idle, &short),
            15
        );
        let long = RuntimeConfig {
            power_sample_seconds: 3600,
            ..cfg
        };
        assert_eq!(
            power_fallback_seconds(
                PowerVerdict::External,
                Visibility::MappedActive,
                Activity::Engaged,
                &long,
            ),
            MAX_POWER_FALLBACK_SECONDS
        );
    }

    #[test]
    fn power_source_uses_only_online_state_and_ignores_device_batteries() {
        let root = std::env::temp_dir().join(format!(
            "pulsedeck-power-source-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let ac = root.join("ac");
        let device_battery = root.join("mouse");
        std::fs::create_dir_all(&ac).unwrap();
        std::fs::create_dir_all(&device_battery).unwrap();
        std::fs::write(ac.join("type"), "Mains\n").unwrap();
        std::fs::write(ac.join("online"), "1\n").unwrap();
        std::fs::write(device_battery.join("type"), "Battery\n").unwrap();
        std::fs::write(device_battery.join("scope"), "Device\n").unwrap();

        assert_eq!(read_power_source(&root), PowerVerdict::External);
        std::fs::write(ac.join("online"), "0\n").unwrap();
        assert_eq!(read_power_source(&root), PowerVerdict::Unknown);
        std::fs::remove_dir_all(&ac).unwrap();
        assert_eq!(read_power_source(&root), PowerVerdict::Unknown);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(read_power_source(&root), PowerVerdict::Unknown);
    }

    #[test]
    fn power_source_reuses_battery_presence_selection() {
        let root = std::env::temp_dir().join(format!(
            "pulsedeck-power-source-battery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let absent = root.join("BAT0");
        std::fs::create_dir_all(&absent).unwrap();
        std::fs::write(absent.join("type"), "Battery\n").unwrap();
        std::fs::write(absent.join("present"), "0\n").unwrap();
        std::fs::write(absent.join("capacity"), "0\n").unwrap();
        assert_eq!(select_battery(&root), None);
        assert_eq!(read_power_source(&root), PowerVerdict::Unknown);

        std::fs::remove_file(absent.join("present")).unwrap();
        assert!(select_battery(&root).is_some());
        assert_eq!(read_power_source(&root), PowerVerdict::Battery);

        let charge_only = root.join("BAT1");
        std::fs::create_dir_all(&charge_only).unwrap();
        std::fs::write(charge_only.join("type"), "Battery\n").unwrap();
        std::fs::write(charge_only.join("present"), "1\n").unwrap();
        std::fs::write(charge_only.join("charge_now"), "50\n").unwrap();
        std::fs::write(charge_only.join("charge_full"), "100\n").unwrap();
        assert_eq!(read_power_source(&root), PowerVerdict::Battery);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mobile_thermal_thresholds_do_not_throttle_normal_cpu_bursts() {
        assert_eq!(
            classify_thermal(Some(35.4), Some(71.8)),
            ThermalVerdict::Normal
        );
        assert_eq!(
            classify_thermal(Some(42.0), Some(75.0)),
            ThermalVerdict::Warm
        );
        assert_eq!(
            classify_thermal(Some(35.0), Some(80.0)),
            ThermalVerdict::Warm
        );
        assert_eq!(
            classify_thermal(Some(35.0), Some(90.0)),
            ThermalVerdict::Hot
        );
        assert_eq!(
            classify_thermal(Some(48.0), Some(70.0)),
            ThermalVerdict::Hot
        );
    }

    #[test]
    fn temperature_units_are_normalized() {
        assert_eq!(thermal::normalize_temperature(42_000.0), 42.0);
        assert_eq!(thermal::normalize_temperature(420.0), 42.0);
        assert_eq!(thermal::normalize_temperature(42.0), 42.0);
    }
}
