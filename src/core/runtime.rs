use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use chrono::{Local, Timelike};

use super::config::{RuntimeConfig, HARD_MAX_OBSERVATION_LEASE_SECONDS};
#[cfg_attr(not(feature = "pet-card"), allow(unused_imports))]
pub use super::runtime_policy::VisualPolicy;
use super::runtime_policy::{evaluate, quiet_hours_active, RuntimeFacts};
pub use super::runtime_policy::{
    Activity, AgentPhase, BatteryStage, IdleViewDecision, PowerVerdict, RuntimeSnapshot,
    ThermalVerdict, Visibility, WorkLevel,
};

fn deadline_after(now: Instant, duration: Duration) -> Instant {
    now.checked_add(duration).unwrap_or(now)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserActivity {
    Click,
    Scroll,
    Keyboard,
    Drag,
    PageSwitch,
    ManualRefresh,
    Dialog,
    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    PluginControl,
}

#[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportantEventKind {
    Completed,
    Failed,
    Cancelled,
    WaitingInput,
    ConfirmationRequired,
    Aborted,
}

#[derive(Debug, Clone)]
struct AgentRuntime {
    task_id: String,
    attention: Option<(String, Instant)>,
    active: bool,
}

struct RuntimeSubscriber {
    id: u64,
    sender: async_channel::Sender<RuntimeSnapshot>,
    drain: async_channel::Receiver<RuntimeSnapshot>,
}

const MAX_SEEN_AGENT_EVENTS: usize = 4096;

pub struct RuntimeManager {
    self_weak: Weak<RuntimeManager>,
    config: RefCell<RuntimeConfig>,
    snapshot: RefCell<RuntimeSnapshot>,
    subscribers: RefCell<Vec<RuntimeSubscriber>>,
    next_subscriber: Cell<u64>,
    mapped: Cell<bool>,
    active: Cell<bool>,
    inactive_since: Cell<Option<Instant>>,
    last_activity: Cell<Instant>,
    idle_candidate_since: Cell<Option<Instant>>,
    interactions: RefCell<HashMap<u64, Instant>>,
    next_interaction: Cell<u64>,
    /// Explicit dialog/plugin leases are separate from the ordinary
    /// observation lease renewed by real user input.
    observation_lease: Cell<Option<Instant>>,
    agent: RefCell<Option<AgentRuntime>>,
    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    seen_events: RefCell<HashSet<(String, String)>>,
    seen_event_order: RefCell<VecDeque<(String, String)>>,
    power: Cell<PowerVerdict>,
    thermal: Cell<ThermalVerdict>,
    battery_capacity: Cell<Option<f64>>,
    battery_stage: Cell<BatteryStage>,
    deadline_source: RefCell<Option<glib::SourceId>>,
}

#[derive(Clone)]
pub struct RuntimeHandle {
    manager: Rc<RuntimeManager>,
}

/// A runtime state subscription with an explicit lifetime boundary. The
/// manager keeps a second receiver to implement latest-wins delivery on a
/// bounded channel; this wrapper removes that subscriber when the consumer
/// task is dropped so the drain receiver cannot keep it alive indefinitely.
pub struct RuntimeSubscription {
    receiver: async_channel::Receiver<RuntimeSnapshot>,
    manager: Weak<RuntimeManager>,
    id: u64,
}

impl RuntimeSubscription {
    pub async fn recv(&self) -> Result<RuntimeSnapshot, async_channel::RecvError> {
        self.receiver.recv().await
    }

    #[cfg(test)]
    pub fn try_recv(&self) -> Result<RuntimeSnapshot, async_channel::TryRecvError> {
        self.receiver.try_recv()
    }

    #[cfg(feature = "scrcpy-forge")]
    pub fn is_closed(&self) -> bool {
        self.receiver.is_closed()
    }
}

impl Drop for RuntimeSubscription {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.upgrade() {
            manager
                .subscribers
                .borrow_mut()
                .retain(|subscriber| subscriber.id != self.id);
        }
    }
}

pub struct InteractionLease {
    manager: Weak<RuntimeManager>,
    id: u64,
}

impl Drop for InteractionLease {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.upgrade() {
            manager.interactions.borrow_mut().remove(&self.id);
            manager.recompute();
        }
    }
}

impl RuntimeManager {
    pub fn new(config: RuntimeConfig) -> Rc<Self> {
        let manager = Rc::new_cyclic(|weak| Self {
            self_weak: weak.clone(),
            config: RefCell::new(config),
            snapshot: RefCell::new(RuntimeSnapshot::default()),
            subscribers: RefCell::new(Vec::new()),
            next_subscriber: Cell::new(0),
            mapped: Cell::new(true),
            active: Cell::new(true),
            inactive_since: Cell::new(None),
            last_activity: Cell::new(Instant::now()),
            idle_candidate_since: Cell::new(None),
            interactions: RefCell::new(HashMap::new()),
            next_interaction: Cell::new(0),
            observation_lease: Cell::new(None),
            agent: RefCell::new(None),
            seen_events: RefCell::new(HashSet::new()),
            seen_event_order: RefCell::new(VecDeque::new()),
            power: Cell::new(PowerVerdict::Unknown),
            thermal: Cell::new(ThermalVerdict::Unknown),
            battery_capacity: Cell::new(None),
            battery_stage: Cell::new(BatteryStage::Unknown),
            deadline_source: RefCell::new(None),
        });
        manager.recompute();
        manager
    }

    pub fn handle(self: &Rc<Self>) -> RuntimeHandle {
        RuntimeHandle {
            manager: self.clone(),
        }
    }

    pub fn config(&self) -> RuntimeConfig {
        self.config.borrow().clone()
    }

    pub fn update_config(&self, config: RuntimeConfig) {
        self.config.replace(config);
        if let Some(capacity) = self.battery_capacity.get() {
            let config = self.config.borrow().clone();
            self.battery_stage.set(battery_stage_for(
                capacity,
                self.battery_stage.get(),
                &config,
            ));
        }
        // A shorter configured lease takes effect immediately; a longer lease
        // is applied on the next real input rather than extending an existing
        // observation without user activity.
        let now = Instant::now();
        let configured = self
            .config
            .borrow()
            .observation_lease_seconds
            .min(HARD_MAX_OBSERVATION_LEASE_SECONDS);
        let maximum = deadline_after(now, Duration::from_secs(configured));
        if self
            .observation_lease
            .get()
            .is_some_and(|deadline| deadline > now && deadline > maximum)
        {
            self.observation_lease.set(Some(maximum));
        }
        self.interactions.borrow_mut().retain(|_, deadline| {
            if *deadline <= now {
                false
            } else {
                *deadline = (*deadline).min(maximum);
                *deadline > now
            }
        });
        self.recompute();
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.snapshot.borrow().clone()
    }

    pub fn subscribe(&self) -> RuntimeSubscription {
        let (tx, rx) = async_channel::bounded(1);
        let _ = tx.try_send(self.snapshot());
        let id = self.next_subscriber.get().wrapping_add(1);
        self.next_subscriber.set(id);
        self.subscribers.borrow_mut().push(RuntimeSubscriber {
            id,
            sender: tx,
            drain: rx.clone(),
        });
        RuntimeSubscription {
            receiver: rx,
            manager: self.self_weak.clone(),
            id,
        }
    }

    /// Close controller-owned subscriptions before application teardown. The
    /// manager may outlive a dropped window through a pending local task, so
    /// dropping the window alone is not enough to wake its receivers.
    pub fn shutdown(&self) {
        self.subscribers.borrow_mut().clear();
        if let Some(source) = self.deadline_source.borrow_mut().take() {
            source.remove();
        }
    }

    pub fn set_mapped(&self, mapped: bool) {
        if self.mapped.replace(mapped) != mapped {
            if !mapped {
                // Observation is scoped to a mapped window. Do not let a
                // dialog or input lease silently survive a hide/unmap edge and
                // become active again later without fresh user input.
                self.observation_lease.take();
                self.interactions.borrow_mut().clear();
            }
            self.recompute();
        }
    }

    pub fn set_active(&self, active: bool) {
        if self.active.replace(active) == active {
            return;
        }
        self.inactive_since.set((!active).then(Instant::now));
        self.recompute();
    }

    pub fn report_activity(&self, _activity: UserActivity) {
        let now = Instant::now();
        self.last_activity.set(now);
        self.idle_candidate_since.set(None);
        if self.mapped.get() {
            let seconds = self
                .config
                .borrow()
                .observation_lease_seconds
                .min(HARD_MAX_OBSERVATION_LEASE_SECONDS);
            if seconds > 0 {
                self.observation_lease
                    .set(Some(deadline_after(now, Duration::from_secs(seconds))));
            }
        }
        self.recompute();
    }

    pub fn set_power_source(&self, power: PowerVerdict) {
        self.power.set(power);
        self.recompute();
    }

    pub fn set_thermal(&self, thermal: ThermalVerdict) {
        self.thermal.set(thermal);
        self.recompute();
    }

    /// Feed a validated capacity sample into the generic hysteretic battery
    /// stage. Missing or invalid samples retain the last known stage.
    pub fn set_battery_capacity(&self, capacity: Option<f64>) {
        let Some(capacity) =
            capacity.filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
        else {
            return;
        };
        self.battery_capacity.set(Some(capacity));
        let config = self.config.borrow().clone();
        let stage = battery_stage_for(capacity, self.battery_stage.get(), &config);
        self.battery_stage.set(stage);
        self.recompute();
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn agent_started(&self, task_id: impl Into<String>) {
        let task_id = task_id.into();
        let mut agent = self.agent.borrow_mut();
        if let Some(current) = agent.as_mut().filter(|current| current.task_id == task_id) {
            if current.active && current.attention.is_none() {
                return;
            }
            current.active = true;
            current.attention = None;
            drop(agent);
            self.recompute();
            return;
        }
        *agent = Some(AgentRuntime {
            task_id,
            attention: None,
            active: true,
        });
        drop(agent);
        self.recompute();
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn agent_event(
        &self,
        task_id: impl Into<String>,
        event_id: impl Into<String>,
        kind: ImportantEventKind,
    ) -> bool {
        let task_id = task_id.into();
        let event_id = event_id.into();
        let event_key = (task_id.clone(), event_id.clone());
        if !self.seen_events.borrow_mut().insert(event_key.clone()) {
            return false;
        }
        let mut order = self.seen_event_order.borrow_mut();
        order.push_back(event_key);
        while order.len() > MAX_SEEN_AGENT_EVENTS {
            if let Some(oldest) = order.pop_front() {
                self.seen_events.borrow_mut().remove(&oldest);
            }
        }
        let seconds = self.config.borrow().agent_attention_seconds;
        let active = matches!(
            kind,
            ImportantEventKind::WaitingInput | ImportantEventKind::ConfirmationRequired
        );
        self.agent.replace(Some(AgentRuntime {
            task_id,
            attention: Some((
                event_id,
                deadline_after(Instant::now(), Duration::from_secs(seconds.max(1))),
            )),
            active,
        }));
        self.recompute();
        true
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn clear_agent(&self) {
        self.agent.borrow_mut().take();
        self.recompute();
    }

    fn recompute(&self) {
        let now = Instant::now();
        self.interactions
            .borrow_mut()
            .retain(|_, deadline| *deadline > now);
        if self
            .observation_lease
            .get()
            .is_some_and(|deadline| deadline <= now)
        {
            self.observation_lease.take();
        }
        let interaction_active =
            self.observation_lease.get().is_some() || !self.interactions.borrow().is_empty();
        let cfg = self.config.borrow().clone();
        let idle_elapsed = now.saturating_duration_since(self.last_activity.get());
        let eligible_idle = self.mapped.get()
            && idle_elapsed >= Duration::from_secs(cfg.idle_timeout_seconds)
            && !interaction_active;
        if eligible_idle && self.idle_candidate_since.get().is_none() {
            self.idle_candidate_since.set(Some(now));
        } else if !eligible_idle {
            self.idle_candidate_since.set(None);
        }
        let stable_idle = eligible_idle
            && self.idle_candidate_since.get().is_some_and(|at| {
                now.saturating_duration_since(at) >= Duration::from_secs(cfg.idle_stability_seconds)
            });

        let mut attention_remaining = 0;
        let mut phase = AgentPhase::None;
        if let Some(agent) = self.agent.borrow().as_ref() {
            if let Some((event_id, until)) = &agent.attention {
                if *until > now {
                    attention_remaining = until.saturating_duration_since(now).as_secs();
                    phase = AgentPhase::Attention {
                        task_id: agent.task_id.clone(),
                        event_id: event_id.clone(),
                    };
                }
            }
            if matches!(phase, AgentPhase::None) && agent.active {
                phase = AgentPhase::Active {
                    task_id: agent.task_id.clone(),
                };
            }
        }

        let inactive_grace_elapsed = self.active.get()
            || self.inactive_since.get().is_some_and(|since| {
                now.saturating_duration_since(since)
                    >= Duration::from_secs(cfg.inactive_grace_seconds)
            });
        let local_hour = Local::now().hour() as u8;
        let facts = RuntimeFacts {
            mapped: self.mapped.get(),
            active: self.active.get(),
            inactive_grace_elapsed,
            user_idle: stable_idle,
            interaction_active,
            quiet_hours: quiet_hours_active(
                cfg.quiet_hours_enabled,
                cfg.quiet_hours_start_hour,
                cfg.quiet_hours_end_hour,
                local_hour,
            ),
            power: self.power.get(),
            thermal: self.thermal.get(),
            battery_stage: self.battery_stage.get(),
            agent_phase: phase,
        };
        let mut next = evaluate(&facts, &cfg);
        next.attention_remaining_seconds = attention_remaining;
        next.observation_lease_active =
            self.observation_lease.get().is_some() || !self.interactions.borrow().is_empty();
        if *self.snapshot.borrow() != next {
            self.snapshot.replace(next.clone());
            self.broadcast(next);
        }
        self.schedule_deadline(now, &cfg);
    }

    fn broadcast(&self, snapshot: RuntimeSnapshot) {
        self.subscribers.borrow_mut().retain(|subscriber| {
            match subscriber.sender.try_send(snapshot.clone()) {
                Ok(()) => true,
                Err(async_channel::TrySendError::Full(snapshot)) => {
                    let _ = subscriber.drain.try_recv();
                    subscriber.sender.try_send(snapshot).is_ok() || !subscriber.sender.is_closed()
                }
                Err(async_channel::TrySendError::Closed(_)) => false,
            }
        });
    }

    fn schedule_deadline(&self, now: Instant, cfg: &RuntimeConfig) {
        if let Some(source) = self.deadline_source.borrow_mut().take() {
            source.remove();
        }
        if !self.mapped.get() {
            return;
        }
        let mut deadlines = Vec::new();
        let idle_at = deadline_after(
            self.last_activity.get(),
            Duration::from_secs(cfg.idle_timeout_seconds),
        );
        if idle_at > now {
            deadlines.push(idle_at);
        } else if let Some(candidate) = self.idle_candidate_since.get() {
            deadlines.push(deadline_after(
                candidate,
                Duration::from_secs(cfg.idle_stability_seconds),
            ));
        }
        if let Some(since) = self.inactive_since.get() {
            deadlines.push(deadline_after(
                since,
                Duration::from_secs(cfg.inactive_grace_seconds),
            ));
        }
        deadlines.extend(self.interactions.borrow().values().copied());
        if let Some(deadline) = self.observation_lease.get() {
            deadlines.push(deadline);
        }
        if let Some(seconds) = seconds_until_quiet_boundary(cfg, Local::now()) {
            deadlines.push(deadline_after(now, Duration::from_secs(seconds)));
        }
        if let Some(agent) = self.agent.borrow().as_ref() {
            if let Some((_, until)) = agent.attention {
                deadlines.push(until);
            }
        }
        let Some(deadline) = deadlines.into_iter().filter(|at| *at > now).min() else {
            return;
        };
        let weak = self.self_weak.clone();
        let source = glib::timeout_add_local_once(
            deadline
                .saturating_duration_since(now)
                .max(Duration::from_millis(10)),
            move || {
                if let Some(manager) = weak.upgrade() {
                    manager.deadline_source.borrow_mut().take();
                    manager.recompute();
                }
            },
        );
        self.deadline_source.replace(Some(source));
    }
}

fn battery_stage_for(capacity: f64, current: BatteryStage, cfg: &RuntimeConfig) -> BatteryStage {
    let critical_enter = f64::from(
        cfg.battery_critical_enter_percent
            .min(cfg.battery_critical_exit_percent),
    );
    let critical_exit = f64::from(
        cfg.battery_critical_exit_percent
            .max(cfg.battery_critical_enter_percent),
    );
    let low_enter = f64::from(
        cfg.battery_low_enter_percent
            .max(cfg.battery_critical_exit_percent)
            .min(100),
    );
    let low_exit = f64::from(
        cfg.battery_low_exit_percent
            .max(cfg.battery_low_enter_percent)
            .min(100),
    );

    match current {
        BatteryStage::Critical => {
            if capacity >= critical_exit {
                BatteryStage::Low
            } else {
                BatteryStage::Critical
            }
        }
        BatteryStage::Low => {
            if capacity <= critical_enter {
                BatteryStage::Critical
            } else if capacity >= low_exit {
                BatteryStage::Normal
            } else {
                BatteryStage::Low
            }
        }
        BatteryStage::Normal | BatteryStage::Unknown => {
            if capacity <= critical_enter {
                BatteryStage::Critical
            } else if capacity <= low_enter {
                BatteryStage::Low
            } else {
                BatteryStage::Normal
            }
        }
    }
}

fn seconds_until_quiet_boundary(
    cfg: &RuntimeConfig,
    local: chrono::DateTime<Local>,
) -> Option<u64> {
    if !cfg.quiet_hours_enabled
        || cfg.quiet_hours_start_hour > 23
        || cfg.quiet_hours_end_hour > 23
        || cfg.quiet_hours_start_hour == cfg.quiet_hours_end_hour
    {
        return None;
    }
    let current = local.hour() as i64 * 3600 + local.minute() as i64 * 60 + local.second() as i64;
    [cfg.quiet_hours_start_hour, cfg.quiet_hours_end_hour]
        .into_iter()
        .map(|hour| {
            let target = hour as i64 * 3600;
            let delta = (target - current).rem_euclid(24 * 3600);
            if delta == 0 {
                24 * 3600
            } else {
                delta
            }
        })
        .min()
        .map(|seconds| seconds as u64)
}

impl RuntimeHandle {
    pub fn config(&self) -> RuntimeConfig {
        self.manager.config()
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.manager.snapshot()
    }

    pub fn subscribe(&self) -> RuntimeSubscription {
        self.manager.subscribe()
    }

    pub fn shutdown(&self) {
        self.manager.shutdown();
    }

    pub fn report_user_activity(&self, activity: UserActivity) {
        self.manager.report_activity(activity);
    }

    pub fn set_mapped(&self, mapped: bool) {
        self.manager.set_mapped(mapped);
    }

    pub fn set_active(&self, active: bool) {
        self.manager.set_active(active);
    }

    pub fn update_config(&self, config: RuntimeConfig) {
        self.manager.update_config(config);
    }

    pub fn set_power_source(&self, power: PowerVerdict) {
        self.manager.set_power_source(power);
    }

    pub fn set_thermal(&self, thermal: ThermalVerdict) {
        self.manager.set_thermal(thermal);
    }

    pub fn set_battery_capacity(&self, capacity: Option<f64>) {
        self.manager.set_battery_capacity(capacity);
    }

    pub fn begin_interaction(&self, max_duration: Duration) -> InteractionLease {
        let now = Instant::now();
        let id = self.manager.next_interaction.get().wrapping_add(1);
        self.manager.next_interaction.set(id);
        let configured = self
            .manager
            .config
            .borrow()
            .observation_lease_seconds
            .min(HARD_MAX_OBSERVATION_LEASE_SECONDS);
        let duration = max_duration
            .min(Duration::from_secs(configured))
            .min(Duration::from_secs(HARD_MAX_OBSERVATION_LEASE_SECONDS));
        if duration > Duration::ZERO && self.manager.mapped.get() {
            self.manager
                .interactions
                .borrow_mut()
                .insert(id, deadline_after(now, duration));
        }
        self.manager.recompute();
        InteractionLease {
            manager: Rc::downgrade(&self.manager),
            id,
        }
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn report_agent_started(&self, task_id: impl Into<String>) {
        self.manager.agent_started(task_id);
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn report_agent_event(
        &self,
        task_id: impl Into<String>,
        event_id: impl Into<String>,
        kind: ImportantEventKind,
    ) -> bool {
        self.manager.agent_event(task_id, event_id, kind)
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn clear_agent(&self) {
        self.manager.clear_agent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static GLIB_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn duplicate_agent_start_keeps_one_active_task() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig::default());
        manager.agent_started("task");
        manager.agent_started("task");
        assert_eq!(
            manager.snapshot().agent_phase,
            AgentPhase::Active {
                task_id: "task".into()
            }
        );
    }

    #[test]
    fn important_events_are_deduplicated() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig::default());
        assert!(manager.agent_event("task", "1", ImportantEventKind::Completed));
        assert!(!manager.agent_event("task", "1", ImportantEventKind::Completed));
        assert!(manager.agent_event("task", "2", ImportantEventKind::Failed));
        assert!(!manager.agent_event("task", "1", ImportantEventKind::Completed));
    }

    #[test]
    fn map_and_active_are_independent() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            inactive_grace_seconds: 0,
            ..RuntimeConfig::default()
        });
        manager.set_active(false);
        assert_eq!(manager.snapshot().visibility, Visibility::MappedInactive);
        assert_ne!(manager.snapshot().work_level, WorkLevel::Suspended);
        manager.set_mapped(false);
        assert_eq!(manager.snapshot().visibility, Visibility::Unmapped);
        assert_eq!(manager.snapshot().work_level, WorkLevel::Suspended);
        assert!(manager.deadline_source.borrow().is_none());
    }

    #[test]
    fn shutdown_closes_runtime_consumers_and_deadline_sources() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig::default());
        let rx = manager.subscribe();
        let _ = rx.try_recv();
        assert!(manager.deadline_source.borrow().is_some());
        manager.shutdown();
        assert!(rx.try_recv().is_err());
        assert!(manager.deadline_source.borrow().is_none());
    }

    #[test]
    fn dropping_subscription_removes_the_manager_entry() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig::default());
        let rx = manager.subscribe();
        assert_eq!(manager.subscribers.borrow().len(), 1);
        drop(rx);
        assert!(manager.subscribers.borrow().is_empty());
    }

    #[test]
    fn activity_recovers_from_idle_immediately() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            idle_timeout_seconds: 0,
            idle_stability_seconds: 0,
            ..RuntimeConfig::default()
        });
        manager.recompute();
        assert_eq!(manager.snapshot().activity, Activity::Idle);
        manager.update_config(RuntimeConfig::default());
        manager.report_activity(UserActivity::Click);
        assert_eq!(manager.snapshot().activity, Activity::Engaged);
    }

    #[test]
    fn real_input_renews_configured_observation_lease_but_agent_does_not() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            observation_lease_seconds: 30,
            ..RuntimeConfig::default()
        });
        manager.agent_started("task");
        assert!(!manager.snapshot().observation_lease_active);
        manager.report_activity(UserActivity::Click);
        assert!(manager.snapshot().observation_lease_active);
        let lease = manager.handle().begin_interaction(Duration::from_secs(600));
        assert!(manager.snapshot().observation_lease_active);
        drop(lease);
        // The ordinary input lease remains valid even after a dialog owner is
        // released; it is owned by the user-observation clock.
        assert!(manager.snapshot().observation_lease_active);
    }

    #[test]
    fn dropping_an_interaction_owner_releases_its_lease() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig::default());
        let lease = manager.handle().begin_interaction(Duration::from_secs(30));
        assert!(manager.snapshot().observation_lease_active);
        drop(lease);
        assert!(!manager.snapshot().observation_lease_active);
        manager.set_mapped(false);
        let lease = manager.handle().begin_interaction(Duration::from_secs(30));
        assert!(!manager.snapshot().observation_lease_active);
        drop(lease);
    }

    #[test]
    fn battery_stage_is_hysteretic_and_external_power_does_not_clear_it() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig::default());
        manager.set_battery_capacity(Some(20.0));
        assert_eq!(manager.snapshot().battery_stage, BatteryStage::Low);
        manager.set_battery_capacity(Some(22.0));
        assert_eq!(manager.snapshot().battery_stage, BatteryStage::Low);
        manager.set_power_source(PowerVerdict::External);
        assert_eq!(manager.snapshot().battery_stage, BatteryStage::Low);
        manager.set_battery_capacity(Some(25.0));
        assert_eq!(manager.snapshot().battery_stage, BatteryStage::Normal);
        manager.set_battery_capacity(Some(10.0));
        assert_eq!(manager.snapshot().battery_stage, BatteryStage::Critical);
        manager.set_battery_capacity(Some(14.0));
        assert_eq!(manager.snapshot().battery_stage, BatteryStage::Critical);
        manager.set_battery_capacity(Some(15.0));
        assert_eq!(manager.snapshot().battery_stage, BatteryStage::Low);
    }
}
