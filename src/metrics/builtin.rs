use super::battery_capacity::BatteryCapacityMetric;
use super::battery_temperature::BatteryTemperatureMetric;
use super::cpu::CpuMetric;
use super::memory::MemoryMetric;
use super::network::NetworkMetric;
use super::power::PowerMetric;
use super::system::SystemMetric;
use super::traits::BuiltinMetric;
use super::uptime::UptimeMetric;

/// Stateful metrics require their own fixed sample cadence and must not be
/// satisfied by card-level disk caches.
pub fn builtin_uses_stateful_sampling(name: &str) -> bool {
    matches!(name, "cpu" | "network_traffic" | "power")
}

/// These projections are refreshed by shared system signal sources with
/// bounded fallbacks rather than by an independent per-card timer.
pub fn builtin_is_event_driven(name: &str) -> bool {
    matches!(
        name,
        "battery_capacity" | "battery_temperature" | "cpu_temperature" | "network"
    )
}

pub fn create_builtin_metric(name: &str) -> Option<BuiltinMetric> {
    match name {
        "cpu" => Some(BuiltinMetric::Cpu(CpuMetric::new(None))),
        "memory" => Some(BuiltinMetric::Memory(MemoryMetric::new())),
        "uptime" => Some(BuiltinMetric::Uptime(UptimeMetric::new())),
        "battery_capacity" => Some(BuiltinMetric::BatteryCapacity(BatteryCapacityMetric::new())),
        "battery_temperature" => Some(BuiltinMetric::BatteryTemperature(
            BatteryTemperatureMetric::new(),
        )),
        "power" => Some(BuiltinMetric::Power(PowerMetric::new(None))),
        "network" => Some(BuiltinMetric::Network(NetworkMetric::new())),
        "load_average" => Some(BuiltinMetric::System(SystemMetric::LoadAverage)),
        "swap" => Some(BuiltinMetric::System(SystemMetric::Swap)),
        "process_count" => Some(BuiltinMetric::System(SystemMetric::ProcessCount)),
        "cpu_temperature" => Some(BuiltinMetric::System(SystemMetric::CpuTemperature)),
        "filesystem" => Some(BuiltinMetric::System(SystemMetric::Filesystem)),
        "network_traffic" => Some(BuiltinMetric::System(SystemMetric::NetworkTraffic {
            previous: None,
        })),
        _ => None,
    }
}
