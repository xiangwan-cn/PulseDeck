use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use super::config::RuntimeConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    ForegroundNormal,
    ForegroundIdle,
    ExternalPowerRealtime,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshMode {
    Normal,
    Throttled,
    Realtime,
    Suspended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationPolicy {
    Normal,
    Reduced(u32),
    Frozen,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewPolicy {
    Normal,
    Reduced,
    MetadataOnly,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerVerdict {
    Battery,
    ExternalUnstable,
    ExternalSufficient,
    ExternalInsufficient,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalVerdict {
    Normal,
    Warm,
    Hot,
    Throttled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexPhase {
    None,
    Protected { task_id: String },
    LongRunning { task_id: String },
    Attention { task_id: String, event_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub mode: RuntimeMode,
    pub refresh_mode: RefreshMode,
    pub animation_policy: AnimationPolicy,
    pub preview_policy: PreviewPolicy,
    pub codex_phase: CodexPhase,
    pub foreground: bool,
    pub user_idle: bool,
    pub interaction_active: bool,
    pub power_verdict: PowerVerdict,
    pub thermal_verdict: ThermalVerdict,
    pub reason: String,
    pub codex_protection_remaining_seconds: u64,
    pub attention_remaining_seconds: u64,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::ForegroundNormal,
            refresh_mode: RefreshMode::Normal,
            animation_policy: AnimationPolicy::Normal,
            preview_policy: PreviewPolicy::Normal,
            codex_phase: CodexPhase::None,
            foreground: true,
            user_idle: false,
            interaction_active: false,
            power_verdict: PowerVerdict::Unknown,
            thermal_verdict: ThermalVerdict::Unknown,
            reason: "应用启动".into(),
            codex_protection_remaining_seconds: 0,
            attention_remaining_seconds: 0,
        }
    }
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
struct CodexRuntime {
    task_id: String,
    protected_until: Instant,
    attention: Option<(String, Instant)>,
    active: bool,
}

pub struct RuntimeManager {
    self_weak: Weak<RuntimeManager>,
    config: RefCell<RuntimeConfig>,
    snapshot: RefCell<RuntimeSnapshot>,
    subscribers: RefCell<Vec<async_channel::Sender<RuntimeSnapshot>>>,
    foreground: Cell<bool>,
    last_activity: Cell<Instant>,
    idle_candidate_since: Cell<Option<Instant>>,
    interactions: RefCell<HashMap<u64, Instant>>,
    next_interaction: Cell<u64>,
    codex: RefCell<Option<CodexRuntime>>,
    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    seen_events: RefCell<HashMap<String, String>>,
    power: Cell<PowerVerdict>,
    thermal: Cell<ThermalVerdict>,
    cpu_busy_until: Cell<Option<Instant>>,
    deadline_source: RefCell<Option<glib::SourceId>>,
}

#[derive(Clone)]
pub struct RuntimeHandle {
    manager: Rc<RuntimeManager>,
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
            foreground: Cell::new(true),
            last_activity: Cell::new(Instant::now()),
            idle_candidate_since: Cell::new(None),
            interactions: RefCell::new(HashMap::new()),
            next_interaction: Cell::new(0),
            codex: RefCell::new(None),
            seen_events: RefCell::new(HashMap::new()),
            power: Cell::new(PowerVerdict::Unknown),
            thermal: Cell::new(ThermalVerdict::Unknown),
            cpu_busy_until: Cell::new(None),
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
        self.recompute();
        self.broadcast(self.snapshot());
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.snapshot.borrow().clone()
    }

    pub fn subscribe(&self) -> async_channel::Receiver<RuntimeSnapshot> {
        let (tx, rx) = async_channel::unbounded();
        let _ = tx.try_send(self.snapshot());
        self.subscribers.borrow_mut().push(tx);
        rx
    }

    pub fn set_foreground(&self, foreground: bool) {
        self.foreground.set(foreground);
        if foreground {
            self.last_activity.set(Instant::now());
        }
        self.recompute();
    }

    pub fn report_activity(&self, _activity: UserActivity) {
        self.last_activity.set(Instant::now());
        self.idle_candidate_since.set(None);
        self.recompute();
    }

    pub fn set_power(&self, power: PowerVerdict, thermal: ThermalVerdict) {
        self.power.set(power);
        self.thermal.set(thermal);
        self.recompute();
    }

    pub fn report_cpu_activity(&self, percent: f64) {
        if !self.config.borrow().cpu_activity_hint {
            return;
        }
        self.cpu_busy_until
            .set((percent >= 35.0).then(|| Instant::now() + Duration::from_secs(15)));
        self.recompute();
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn codex_started(&self, task_id: impl Into<String>) {
        let task_id = task_id.into();
        let mut codex = self.codex.borrow_mut();
        if let Some(current) = codex.as_mut().filter(|current| current.task_id == task_id) {
            if current.active && current.attention.is_none() {
                return;
            }
            current.active = true;
            current.attention = None;
            drop(codex);
            self.recompute();
            return;
        }
        let minutes = self.config.borrow().codex_protection_minutes;
        *codex = Some(CodexRuntime {
            task_id,
            protected_until: Instant::now() + Duration::from_secs(minutes.saturating_mul(60)),
            attention: None,
            active: true,
        });
        drop(codex);
        self.recompute();
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn codex_finished(
        &self,
        task_id: impl Into<String>,
        event_id: impl Into<String>,
        kind: ImportantEventKind,
    ) -> bool {
        let task_id = task_id.into();
        let event_id = event_id.into();
        if self
            .seen_events
            .borrow()
            .get(&task_id)
            .is_some_and(|seen| seen == &event_id)
        {
            return false;
        }
        self.seen_events
            .borrow_mut()
            .insert(task_id.clone(), event_id.clone());
        let seconds = self.config.borrow().codex_attention_seconds;
        let mut codex = self.codex.borrow_mut();
        let protected_until = codex
            .as_ref()
            .map(|state| state.protected_until)
            .unwrap_or_else(Instant::now);
        let active = matches!(
            kind,
            ImportantEventKind::WaitingInput | ImportantEventKind::ConfirmationRequired
        );
        *codex = Some(CodexRuntime {
            task_id,
            protected_until,
            attention: Some((
                event_id,
                Instant::now() + Duration::from_secs(seconds.max(1)),
            )),
            active,
        });
        drop(codex);
        self.recompute();
        true
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn clear_codex(&self) {
        self.codex.borrow_mut().take();
        self.recompute();
    }

    fn recompute(&self) {
        let now = Instant::now();
        self.interactions
            .borrow_mut()
            .retain(|_, deadline| *deadline > now);
        let interaction_active = !self.interactions.borrow().is_empty();
        let cfg = self.config.borrow().clone();
        let foreground = self.foreground.get();
        let idle_elapsed = now.saturating_duration_since(self.last_activity.get());
        let idle_threshold = Duration::from_secs(cfg.idle_timeout_seconds);
        let eligible_idle =
            cfg.idle_power_saving && idle_elapsed >= idle_threshold && !interaction_active;
        if eligible_idle && self.idle_candidate_since.get().is_none() {
            self.idle_candidate_since.set(Some(now));
        } else if !eligible_idle {
            self.idle_candidate_since.set(None);
        }
        let cpu_busy = cfg.cpu_activity_hint
            && self.cpu_busy_until.get().is_some_and(|until| until > now)
            && self.idle_candidate_since.get().is_some_and(|candidate| {
                now.saturating_duration_since(candidate) < Duration::from_secs(60)
            });
        let stable_idle = eligible_idle
            && !cpu_busy
            && self.idle_candidate_since.get().is_some_and(|at| {
                now.saturating_duration_since(at) >= Duration::from_secs(cfg.idle_stability_seconds)
            });

        let power = self.power.get();
        let thermal = self.thermal.get();
        let external_connected = matches!(
            power,
            PowerVerdict::ExternalUnstable
                | PowerVerdict::ExternalSufficient
                | PowerVerdict::ExternalInsufficient
        );
        let external_realtime = cfg.external_realtime && external_connected;
        let external_prevents_idle = cfg.external_prevents_idle && external_connected;

        let mut protection_remaining = 0;
        let mut attention_remaining = 0;
        let mut phase = CodexPhase::None;
        let mut attention_active = false;
        let mut protected = false;
        if let Some(codex) = self.codex.borrow().as_ref() {
            if let Some((event_id, until)) = &codex.attention {
                if *until > now {
                    attention_active = true;
                    attention_remaining = until.saturating_duration_since(now).as_secs();
                    phase = CodexPhase::Attention {
                        task_id: codex.task_id.clone(),
                        event_id: event_id.clone(),
                    };
                }
            }
            if !attention_active && codex.active {
                if codex.protected_until > now {
                    protected = cfg.codex_keep_bright;
                    protection_remaining = codex
                        .protected_until
                        .saturating_duration_since(now)
                        .as_secs();
                    phase = CodexPhase::Protected {
                        task_id: codex.task_id.clone(),
                    };
                } else {
                    phase = CodexPhase::LongRunning {
                        task_id: codex.task_id.clone(),
                    };
                }
            }
        }

        let (mode, reason) = if !foreground {
            (RuntimeMode::Background, "应用不在前台")
        } else if external_realtime && (external_prevents_idle || !stable_idle) {
            (RuntimeMode::ExternalPowerRealtime, "已连接外接电源")
        } else if attention_active {
            (RuntimeMode::ForegroundNormal, "重要事件短暂唤醒")
        } else if protected {
            (RuntimeMode::ForegroundNormal, "Codex 短时亮度保护")
        } else if external_prevents_idle {
            (RuntimeMode::ForegroundNormal, "已连接外接电源")
        } else if stable_idle {
            (RuntimeMode::ForegroundIdle, "用户空闲并通过稳定等待")
        } else {
            (RuntimeMode::ForegroundNormal, "前台正常")
        };

        let refresh_mode = if !foreground {
            RefreshMode::Suspended
        } else if external_realtime {
            RefreshMode::Realtime
        } else if attention_active {
            RefreshMode::Normal
        } else if external_prevents_idle {
            RefreshMode::Normal
        } else if stable_idle || (eligible_idle && protected) {
            RefreshMode::Throttled
        } else {
            RefreshMode::Normal
        };
        let animation_policy = if !foreground {
            AnimationPolicy::Stopped
        } else if matches!(thermal, ThermalVerdict::Hot | ThermalVerdict::Throttled) {
            AnimationPolicy::Frozen
        } else if mode == RuntimeMode::ForegroundIdle {
            AnimationPolicy::Reduced(1)
        } else {
            AnimationPolicy::Normal
        };
        let preview_policy = if !foreground {
            PreviewPolicy::Stopped
        } else if mode == RuntimeMode::ForegroundIdle {
            PreviewPolicy::MetadataOnly
        } else if matches!(
            thermal,
            ThermalVerdict::Warm | ThermalVerdict::Hot | ThermalVerdict::Throttled
        ) {
            PreviewPolicy::Reduced
        } else {
            PreviewPolicy::Normal
        };
        let next = RuntimeSnapshot {
            mode,
            refresh_mode,
            animation_policy,
            preview_policy,
            codex_phase: phase,
            foreground,
            user_idle: eligible_idle,
            interaction_active,
            power_verdict: power,
            thermal_verdict: thermal,
            reason: reason.into(),
            codex_protection_remaining_seconds: protection_remaining,
            attention_remaining_seconds: attention_remaining,
        };
        let changed = *self.snapshot.borrow() != next;
        if changed {
            self.snapshot.replace(next.clone());
            self.broadcast(next);
        }
        self.schedule_deadline(now, &cfg);
    }

    fn broadcast(&self, snapshot: RuntimeSnapshot) {
        self.subscribers
            .borrow_mut()
            .retain(|sender| sender.try_send(snapshot.clone()).is_ok() || !sender.is_closed());
    }

    fn schedule_deadline(&self, now: Instant, cfg: &RuntimeConfig) {
        if let Some(source) = self.deadline_source.borrow_mut().take() {
            source.remove();
        }
        let mut deadlines = Vec::new();
        if self.foreground.get() && cfg.idle_power_saving {
            let idle_at = self.last_activity.get() + Duration::from_secs(cfg.idle_timeout_seconds);
            if idle_at > now {
                deadlines.push(idle_at);
            } else if let Some(candidate) = self.idle_candidate_since.get() {
                deadlines.push(candidate + Duration::from_secs(cfg.idle_stability_seconds));
            }
        }
        deadlines.extend(self.interactions.borrow().values().copied());
        if let Some(until) = self.cpu_busy_until.get().filter(|until| *until > now) {
            deadlines.push(until);
        }
        if let Some(codex) = self.codex.borrow().as_ref() {
            deadlines.push(codex.protected_until);
            if let Some((_, until)) = codex.attention {
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

impl RuntimeHandle {
    pub fn config(&self) -> RuntimeConfig {
        self.manager.config()
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.manager.snapshot()
    }

    pub fn subscribe(&self) -> async_channel::Receiver<RuntimeSnapshot> {
        self.manager.subscribe()
    }

    pub fn report_user_activity(&self, activity: UserActivity) {
        self.manager.report_activity(activity);
    }

    pub fn set_foreground(&self, foreground: bool) {
        self.manager.set_foreground(foreground);
    }

    pub fn update_config(&self, config: RuntimeConfig) {
        self.manager.update_config(config);
    }

    pub fn set_power(&self, power: PowerVerdict, thermal: ThermalVerdict) {
        self.manager.set_power(power, thermal);
    }

    pub fn report_cpu_activity(&self, percent: f64) {
        self.manager.report_cpu_activity(percent);
    }

    pub fn begin_interaction(&self, max_duration: Duration) -> InteractionLease {
        let id = self.manager.next_interaction.get().wrapping_add(1);
        self.manager.next_interaction.set(id);
        let duration = max_duration.min(Duration::from_secs(300));
        self.manager
            .interactions
            .borrow_mut()
            .insert(id, Instant::now() + duration);
        self.manager.recompute();
        InteractionLease {
            manager: Rc::downgrade(&self.manager),
            id,
        }
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn report_codex_started(&self, task_id: impl Into<String>) {
        self.manager.codex_started(task_id);
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn report_codex_event(
        &self,
        task_id: impl Into<String>,
        event_id: impl Into<String>,
        kind: ImportantEventKind,
    ) -> bool {
        self.manager.codex_finished(task_id, event_id, kind)
    }

    #[cfg_attr(not(feature = "pet-card"), allow(dead_code))]
    pub fn clear_codex(&self) {
        self.manager.clear_codex();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static GLIB_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn duplicate_codex_start_does_not_extend_protection() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig::default());
        manager.codex_started("task");
        let first = manager.codex.borrow().as_ref().unwrap().protected_until;
        manager.codex_started("task");
        assert_eq!(
            manager.codex.borrow().as_ref().unwrap().protected_until,
            first
        );
    }

    #[test]
    fn resumed_task_becomes_active_without_extending_protection() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig::default());
        manager.codex_started("task");
        let protected_until = manager.codex.borrow().as_ref().unwrap().protected_until;
        manager.codex_finished("task", "1", ImportantEventKind::Failed);
        manager.codex_started("task");

        let codex = manager.codex.borrow();
        let codex = codex.as_ref().unwrap();
        assert!(codex.active);
        assert!(codex.attention.is_none());
        assert_eq!(codex.protected_until, protected_until);
    }

    #[test]
    fn important_events_are_deduplicated() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig::default());
        assert!(manager.codex_finished("task", "1", ImportantEventKind::Completed));
        assert!(!manager.codex_finished("task", "1", ImportantEventKind::Completed));
    }

    #[test]
    fn waiting_agent_keeps_idle_overlay_disabled_until_protection_expires() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            idle_timeout_seconds: 0,
            idle_stability_seconds: 0,
            codex_protection_minutes: 60,
            ..RuntimeConfig::default()
        });
        manager.codex_started("task");
        manager.codex_finished("task", "1", ImportantEventKind::WaitingInput);
        manager.codex.borrow_mut().as_mut().unwrap().attention = None;
        manager.recompute();

        assert_eq!(manager.snapshot().mode, RuntimeMode::ForegroundNormal);
        assert!(matches!(
            manager.snapshot().codex_phase,
            CodexPhase::Protected { .. }
        ));
    }

    #[test]
    fn confirmation_agent_keeps_idle_overlay_disabled_until_protection_expires() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            idle_timeout_seconds: 0,
            idle_stability_seconds: 0,
            codex_protection_minutes: 60,
            ..RuntimeConfig::default()
        });
        manager.codex_started("task");
        manager.codex_finished("task", "1", ImportantEventKind::ConfirmationRequired);
        manager.codex.borrow_mut().as_mut().unwrap().attention = None;
        manager.recompute();

        assert_eq!(manager.snapshot().mode, RuntimeMode::ForegroundNormal);
    }

    #[test]
    fn completed_agent_releases_idle_overlay_after_attention() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            idle_timeout_seconds: 0,
            idle_stability_seconds: 0,
            ..RuntimeConfig::default()
        });
        manager.codex_started("task");
        manager.codex_finished("task", "1", ImportantEventKind::Completed);
        manager.codex.borrow_mut().as_mut().unwrap().attention = None;
        manager.recompute();

        assert_eq!(manager.snapshot().mode, RuntimeMode::ForegroundIdle);
    }

    #[test]
    fn background_has_highest_priority() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig::default());
        manager.set_power(PowerVerdict::ExternalSufficient, ThermalVerdict::Normal);
        manager.set_foreground(false);
        assert_eq!(manager.snapshot().mode, RuntimeMode::Background);
        assert_eq!(manager.snapshot().refresh_mode, RefreshMode::Suspended);
    }

    #[test]
    fn idle_enters_after_threshold_and_activity_recovers_immediately() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            idle_timeout_seconds: 0,
            idle_stability_seconds: 0,
            ..RuntimeConfig::default()
        });
        manager.recompute();
        assert_eq!(manager.snapshot().mode, RuntimeMode::ForegroundIdle);
        manager.report_activity(UserActivity::Click);
        // With a zero timeout this becomes eligible again immediately; use a
        // real threshold to verify the recovery edge.
        manager.update_config(RuntimeConfig::default());
        manager.report_activity(UserActivity::Click);
        assert_eq!(manager.snapshot().mode, RuntimeMode::ForegroundNormal);
    }

    #[test]
    fn codex_protection_keeps_brightness_but_allows_refresh_throttling() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            idle_timeout_seconds: 0,
            idle_stability_seconds: 0,
            ..RuntimeConfig::default()
        });
        manager.codex_started("task");
        manager.recompute();
        assert_eq!(manager.snapshot().mode, RuntimeMode::ForegroundNormal);
        assert_eq!(manager.snapshot().refresh_mode, RefreshMode::Throttled);
    }

    #[test]
    fn any_connected_external_power_enables_realtime() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        for verdict in [
            PowerVerdict::ExternalUnstable,
            PowerVerdict::ExternalInsufficient,
        ] {
            let manager = RuntimeManager::new(RuntimeConfig {
                idle_timeout_seconds: 0,
                idle_stability_seconds: 0,
                ..RuntimeConfig::default()
            });
            manager.set_power(verdict, ThermalVerdict::Normal);
            manager.recompute();
            assert_eq!(manager.snapshot().mode, RuntimeMode::ExternalPowerRealtime);
            assert_eq!(manager.snapshot().refresh_mode, RefreshMode::Realtime);
        }
    }

    #[test]
    fn connected_external_power_stays_realtime_when_thermal_is_hot() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            idle_timeout_seconds: 0,
            idle_stability_seconds: 0,
            ..RuntimeConfig::default()
        });
        manager.set_power(PowerVerdict::ExternalInsufficient, ThermalVerdict::Hot);
        manager.recompute();
        assert_eq!(manager.snapshot().mode, RuntimeMode::ExternalPowerRealtime);
        assert_eq!(manager.snapshot().refresh_mode, RefreshMode::Realtime);
        assert_eq!(manager.snapshot().animation_policy, AnimationPolicy::Frozen);
        assert_eq!(manager.snapshot().preview_policy, PreviewPolicy::Reduced);
    }

    #[test]
    fn warm_thermal_state_reduces_preview_without_freezing_animation() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig::default());
        manager.set_power(PowerVerdict::Battery, ThermalVerdict::Warm);

        assert_eq!(manager.snapshot().animation_policy, AnimationPolicy::Normal);
        assert_eq!(manager.snapshot().preview_policy, PreviewPolicy::Reduced);
    }

    #[test]
    fn idle_preview_remains_metadata_only_under_thermal_pressure() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            idle_timeout_seconds: 0,
            idle_stability_seconds: 0,
            ..RuntimeConfig::default()
        });
        manager.set_power(PowerVerdict::Battery, ThermalVerdict::Hot);
        manager.recompute();

        assert_eq!(manager.snapshot().mode, RuntimeMode::ForegroundIdle);
        assert_eq!(
            manager.snapshot().preview_policy,
            PreviewPolicy::MetadataOnly
        );
        assert_eq!(manager.snapshot().animation_policy, AnimationPolicy::Frozen);
    }

    #[test]
    fn preventing_idle_does_not_depend_on_external_realtime() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            idle_timeout_seconds: 0,
            idle_stability_seconds: 0,
            external_realtime: false,
            external_prevents_idle: true,
            ..RuntimeConfig::default()
        });
        manager.set_power(PowerVerdict::ExternalSufficient, ThermalVerdict::Normal);
        manager.recompute();
        assert_eq!(manager.snapshot().mode, RuntimeMode::ForegroundNormal);
        assert_eq!(manager.snapshot().refresh_mode, RefreshMode::Normal);
    }

    #[test]
    fn connected_external_power_can_idle_without_disabling_realtime() {
        let _guard = GLIB_TEST_LOCK.lock().unwrap();
        let manager = RuntimeManager::new(RuntimeConfig {
            idle_timeout_seconds: 0,
            idle_stability_seconds: 0,
            external_prevents_idle: false,
            ..RuntimeConfig::default()
        });
        manager.set_power(PowerVerdict::ExternalUnstable, ThermalVerdict::Normal);
        manager.recompute();
        assert_eq!(manager.snapshot().mode, RuntimeMode::ForegroundIdle);
        assert_eq!(manager.snapshot().refresh_mode, RefreshMode::Realtime);
    }
}
