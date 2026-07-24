use std::sync::Mutex;

use crate::model::metric_result::MetricResult;
use crate::sources::battery::BatterySource;
use crate::sources::procfs::ProcFsSource;

pub struct MetricContext {
    pub runtime: tokio::runtime::Handle,
    pub http_client: reqwest::Client,
    pub battery: Mutex<BatterySource>,
    pub procfs: Mutex<ProcFsSource>,
}

impl MetricContext {
    pub fn new(
        runtime: tokio::runtime::Handle,
        http_client: reqwest::Client,
        battery_root: std::path::PathBuf,
        procfs_root: std::path::PathBuf,
    ) -> Self {
        Self {
            runtime,
            http_client,
            battery: Mutex::new(BatterySource::new(battery_root)),
            procfs: Mutex::new(ProcFsSource::new(procfs_root)),
        }
    }
}

pub enum BuiltinMetric {
    Cpu(crate::metrics::cpu::CpuMetric),
    Memory(crate::metrics::memory::MemoryMetric),
    Uptime(crate::metrics::uptime::UptimeMetric),
    BatteryCapacity(crate::metrics::battery_capacity::BatteryCapacityMetric),
    BatteryTemperature(crate::metrics::battery_temperature::BatteryTemperatureMetric),
    Power(crate::metrics::power::PowerMetric),
    Network(crate::metrics::network::NetworkMetric),
    System(crate::metrics::system::SystemMetric),
}

impl BuiltinMetric {
    pub fn collect(&mut self, ctx: &MetricContext) -> MetricResult {
        match self {
            BuiltinMetric::Cpu(m) => m.collect(ctx),
            BuiltinMetric::Memory(m) => m.collect(ctx),
            BuiltinMetric::Uptime(m) => m.collect(ctx),
            BuiltinMetric::BatteryCapacity(m) => m.collect(ctx),
            BuiltinMetric::BatteryTemperature(m) => m.collect(ctx),
            BuiltinMetric::Power(m) => m.collect(ctx),
            BuiltinMetric::Network(m) => m.collect(ctx),
            BuiltinMetric::System(m) => m.collect(ctx),
        }
    }
}
