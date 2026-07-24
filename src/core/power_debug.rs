#[derive(Debug, Clone, Copy)]
pub enum Counter {
    SchedulerWake,
    CardCollect,
    ExternalProcess,
    HttpRequest,
    ImageDecode,
    AnimationTick,
    GtkUpdate,
    DiskRead,
    DiskWrite,
}

#[cfg(feature = "power-debug")]
mod enabled {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::Counter;

    static COUNTERS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];

    pub fn increment(counter: Counter) {
        COUNTERS[counter as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot() -> [u64; 9] {
        std::array::from_fn(|index| COUNTERS[index].load(Ordering::Relaxed))
    }
}

#[cfg(not(feature = "power-debug"))]
mod enabled {
    use super::Counter;
    #[inline(always)]
    pub fn increment(_counter: Counter) {}
}

pub use enabled::increment;
#[cfg(feature = "power-debug")]
pub use enabled::snapshot;
