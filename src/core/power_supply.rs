use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gio::prelude::*;

use super::runtime::{PowerVerdict, RuntimeHandle, RuntimeMode, ThermalVerdict};

pub struct PowerSupplyMonitor {
    runtime: RuntimeHandle,
    root: PathBuf,
    thermal_root: PathBuf,
    source: RefCell<Option<glib::SourceId>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    positive: Cell<u32>,
    negative: Cell<u32>,
    active: Cell<bool>,
    last_energy: Cell<Option<f64>>,
}

impl PowerSupplyMonitor {
    pub fn start(runtime: RuntimeHandle, root: PathBuf, thermal_root: PathBuf) -> Rc<Self> {
        let this = Rc::new(Self {
            runtime,
            root,
            thermal_root,
            source: RefCell::new(None),
            monitor: RefCell::new(None),
            positive: Cell::new(0),
            negative: Cell::new(0),
            active: Cell::new(false),
            last_energy: Cell::new(None),
        });
        Self::install_monitor(&this);
        Self::sample_and_schedule(&this);
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
                Self::sample_and_schedule(&this);
            }
        });
        this.monitor.replace(Some(monitor));
    }

    fn sample_and_schedule(this: &Rc<Self>) {
        if let Some(source) = this.source.borrow_mut().take() {
            source.remove();
        }
        let sample = read_sample(&this.root, &this.thermal_root, this.last_energy.get());
        this.last_energy.set(sample.energy);
        let cfg = this.runtime.config();
        let thermal = sample.thermal;

        let positive = sample.online
            && sample.trustworthy
            && !sample.discharging
            && !sample.energy_declining
            && sample.power_margin_nonnegative
            && !matches!(thermal, ThermalVerdict::Hot | ThermalVerdict::Throttled);
        let negative = !sample.online
            || sample.discharging
            || sample.energy_declining
            || !sample.trustworthy
            || matches!(thermal, ThermalVerdict::Hot | ThermalVerdict::Throttled);

        let next = advance_hysteresis(
            HysteresisState {
                positive: this.positive.get(),
                negative: this.negative.get(),
                active: this.active.get(),
            },
            sample.online,
            positive,
            negative,
            cfg.external_enter_samples,
            cfg.external_exit_samples,
        );
        this.positive.set(next.positive);
        this.negative.set(next.negative);
        this.active.set(next.active);

        let verdict = if !sample.online {
            PowerVerdict::Battery
        } else if this.active.get() {
            PowerVerdict::ExternalSufficient
        } else if negative {
            PowerVerdict::ExternalInsufficient
        } else {
            PowerVerdict::ExternalUnstable
        };
        this.runtime.set_power(verdict, thermal);

        let seconds = match this.runtime.snapshot().mode {
            RuntimeMode::Background | RuntimeMode::ForegroundIdle => {
                cfg.external_sample_seconds.max(10).saturating_mul(3)
            }
            _ => cfg.external_sample_seconds.max(5),
        };
        let weak = Rc::downgrade(this);
        let source = glib::timeout_add_local_once(Duration::from_secs(seconds), move || {
            if let Some(this) = weak.upgrade() {
                this.source.borrow_mut().take();
                Self::sample_and_schedule(&this);
            }
        });
        this.source.replace(Some(source));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HysteresisState {
    positive: u32,
    negative: u32,
    active: bool,
}

fn advance_hysteresis(
    mut state: HysteresisState,
    online: bool,
    positive: bool,
    negative: bool,
    enter_samples: u32,
    exit_samples: u32,
) -> HysteresisState {
    if !online {
        state.positive = 0;
        state.negative = exit_samples.max(1);
        state.active = false;
    } else if positive {
        state.negative = 0;
        state.positive = state.positive.saturating_add(1);
        if state.positive >= enter_samples.max(1) {
            state.active = true;
        }
    } else if negative {
        state.positive = 0;
        state.negative = state.negative.saturating_add(1);
        if state.negative >= exit_samples.max(1) {
            state.active = false;
        }
    }
    state
}

impl Drop for PowerSupplyMonitor {
    fn drop(&mut self) {
        if let Some(source) = self.source.borrow_mut().take() {
            source.remove();
        }
        self.monitor.borrow_mut().take();
    }
}

struct SupplySample {
    online: bool,
    discharging: bool,
    trustworthy: bool,
    energy: Option<f64>,
    energy_declining: bool,
    power_margin_nonnegative: bool,
    thermal: ThermalVerdict,
}

fn read_sample(root: &Path, thermal_root: &Path, previous_energy: Option<f64>) -> SupplySample {
    let mut online = false;
    let mut input_power_w = None::<f64>;
    let mut battery = None;
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let kind = read(&path.join("type")).unwrap_or_default();
            if kind == "Battery" {
                if read(&path.join("scope")).as_deref() != Some("Device") {
                    battery = Some(path);
                }
            } else if matches!(
                kind.as_str(),
                "Mains" | "USB" | "USB_C" | "USB_PD" | "Wireless"
            ) && read(&path.join("online")).as_deref() == Some("1")
            {
                online = true;
                let reported = number(&path, "power_now")
                    .or_else(|| number(&path, "input_power_limit"))
                    .map(|value| value / 1_000_000.0)
                    .or_else(|| {
                        let voltage = number(&path, "voltage_now")
                            .or_else(|| number(&path, "voltage_max"))?;
                        let current = number(&path, "current_now")
                            .or_else(|| number(&path, "current_max"))?;
                        Some(voltage * current / 1_000_000_000_000.0)
                    });
                if let Some(reported) = reported {
                    input_power_w =
                        Some(input_power_w.map_or(reported, |current| current.max(reported)));
                }
            }
        }
    }
    let (status, energy, battery_temp) = battery
        .as_ref()
        .map(|path| {
            let status = read(&path.join("status")).unwrap_or_default();
            let energy = number(path, "energy_now")
                .or_else(|| number(path, "charge_now"))
                .map(|value| value / 1_000_000.0);
            let temp = number(path, "temp")
                .or_else(|| number(path, "temperature"))
                .map(normalize_temp);
            (status, energy, temp)
        })
        .unwrap_or_default();
    let energy_declining = match (previous_energy, energy) {
        (Some(previous), Some(current)) => current + 0.005 < previous,
        _ => false,
    };
    let discharging = status == "Discharging";
    let trustworthy = battery.is_some()
        && matches!(
            status.as_str(),
            "Charging" | "Full" | "Not charging" | "Discharging"
        );
    SupplySample {
        online,
        discharging,
        trustworthy,
        energy,
        energy_declining,
        // When input power is exposed, a zero reading is not a trustworthy
        // high-realtime supply. Otherwise battery direction/trend is the
        // conservative fallback for estimating available margin.
        power_margin_nonnegative: input_power_w.map_or(!discharging, |watts| watts > 0.5),
        thermal: read_thermal(thermal_root, battery_temp),
    }
}

fn read_thermal(root: &Path, battery_temp: Option<f64>) -> ThermalVerdict {
    if thermal_pressure_active() {
        return ThermalVerdict::Throttled;
    }
    let mut hottest_soc = None::<f64>;
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with("thermal_zone")
            {
                continue;
            }
            let kind = read(&path.join("type"))
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !(kind.contains("cpu") || kind.contains("soc") || kind.contains("package")) {
                continue;
            }
            if let Some(value) = number_path(&path.join("temp")).map(normalize_temp) {
                hottest_soc = Some(hottest_soc.map_or(value, |old| old.max(value)));
            }
        }
    }
    if battery_temp.is_some_and(|temp| temp >= 48.0) || hottest_soc.is_some_and(|temp| temp >= 80.0)
    {
        ThermalVerdict::Hot
    } else if battery_temp.is_some_and(|temp| temp >= 42.0)
        || hottest_soc.is_some_and(|temp| temp >= 70.0)
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

fn number(root: &Path, name: &str) -> Option<f64> {
    number_path(&root.join(name))
}

fn number_path(path: &Path) -> Option<f64> {
    read(path)?.parse().ok()
}

fn normalize_temp(value: f64) -> f64 {
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
    use super::{advance_hysteresis, normalize_temp, HysteresisState};

    #[test]
    fn external_power_enters_slowly_and_exits_faster() {
        let mut state = HysteresisState {
            positive: 0,
            negative: 0,
            active: false,
        };
        for _ in 0..2 {
            state = advance_hysteresis(state, true, true, false, 3, 2);
            assert!(!state.active);
        }
        state = advance_hysteresis(state, true, true, false, 3, 2);
        assert!(state.active);
        state = advance_hysteresis(state, true, false, true, 3, 2);
        assert!(state.active);
        state = advance_hysteresis(state, true, false, true, 3, 2);
        assert!(!state.active);
    }

    #[test]
    fn unplug_exits_without_waiting_for_hysteresis() {
        let state = advance_hysteresis(
            HysteresisState {
                positive: 3,
                negative: 0,
                active: true,
            },
            false,
            false,
            true,
            3,
            2,
        );
        assert!(!state.active);
        assert_eq!(state.positive, 0);
    }

    #[test]
    fn temperature_units_are_normalized() {
        assert_eq!(normalize_temp(42_000.0), 42.0);
        assert_eq!(normalize_temp(420.0), 42.0);
        assert_eq!(normalize_temp(42.0), 42.0);
    }
}
