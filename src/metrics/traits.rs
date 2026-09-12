use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::model::metric_result::MetricResult;
use crate::sources::battery::BatterySource;
use crate::sources::network::NetworkSource;
use crate::sources::procfs::ProcFsSource;

pub struct MetricContext {
    pub cancellation: CancellationToken,
    pub http_client: reqwest::Client,
    pub battery: Arc<Mutex<BatterySource>>,
    pub network: Mutex<NetworkSource>,
    pub procfs: Mutex<ProcFsSource>,
    pub thermal_root: PathBuf,
}

impl MetricContext {
    pub fn new(
        cancellation: CancellationToken,
        http_client: reqwest::Client,
        battery_root: std::path::PathBuf,
        procfs_root: std::path::PathBuf,
        thermal_root: std::path::PathBuf,
    ) -> Self {
        Self {
            cancellation,
            http_client,
            battery: Arc::new(Mutex::new(BatterySource::new(battery_root))),
            network: Mutex::new(NetworkSource::new()),
            procfs: Mutex::new(ProcFsSource::new(procfs_root)),
            thermal_root,
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
