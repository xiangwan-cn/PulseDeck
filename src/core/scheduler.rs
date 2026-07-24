use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::time::Instant;

use crate::core::runtime::RefreshMode;

#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub next_run: Instant,
    pub card_id: String,
    pub generation: u64,
}

impl PartialEq for ScheduledTask {
    fn eq(&self, other: &Self) -> bool {
        self.next_run == other.next_run && self.card_id == other.card_id
    }
}

impl Eq for ScheduledTask {}

impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.next_run
            .cmp(&other.next_run)
            .then_with(|| self.card_id.cmp(&other.card_id))
    }
}

#[derive(Debug, Clone)]
pub struct TaskRuntime {
    pub page_id: String,
    pub running: bool,
    pub enabled: bool,
    pub paused: bool,
    pub run_once: bool,
    pub generation: u64,
    pub failure_count: u32,
    pub last_started: Option<Instant>,
    pub last_success: Option<Instant>,
    pub next_run: Instant,
    pub base_interval_secs: u64,
    pub class: TaskClass,
    pub idle_behavior: IdleBehavior,
    pub idle_multiplier: Option<f64>,
    pub external_realtime: Option<bool>,
    pub realtime_multiplier: Option<f64>,
    pub minimum_interval_secs: Option<u64>,
    pub scheduled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskClass {
    SystemRealtime,
    NetworkRate,
    NetworkStatus,
    BatteryThermal,
    Command,
    Http,
    File,
    Static,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleBehavior {
    Throttle,
    Pause,
}

#[derive(Debug, Clone)]
pub struct TaskPolicy {
    pub class: TaskClass,
    pub idle_behavior: IdleBehavior,
    pub idle_multiplier: Option<f64>,
    pub external_realtime: Option<bool>,
    pub realtime_multiplier: Option<f64>,
    pub minimum_interval_secs: Option<u64>,
    pub scheduled: bool,
}

impl Default for TaskPolicy {
    fn default() -> Self {
        Self {
            class: TaskClass::Other,
            idle_behavior: IdleBehavior::Throttle,
            idle_multiplier: None,
            external_realtime: None,
            realtime_multiplier: None,
            minimum_interval_secs: None,
            scheduled: false,
        }
    }
}

pub struct Scheduler {
    heap: BinaryHeap<Reverse<ScheduledTask>>,
    runtimes: HashMap<String, TaskRuntime>,
    paused_when_inactive: bool,
    window_active: bool,
    generation_seed: u64,
    refresh_mode: RefreshMode,
    idle_strength: f64,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            runtimes: HashMap::new(),
            paused_when_inactive: true,
            window_active: true,
            generation_seed: 0,
            refresh_mode: RefreshMode::Normal,
            idle_strength: 1.0,
        }
    }

    #[cfg(test)]
    fn register(&mut self, card_id: &str, interval_secs: u64, page_id: &str) {
        self.register_with_policy(card_id, interval_secs, page_id, TaskPolicy::default());
    }

    pub fn register_with_policy(
        &mut self,
        card_id: &str,
        interval_secs: u64,
        page_id: &str,
        policy: TaskPolicy,
    ) {
        let next_run = Instant::now();
        self.generation_seed = self.generation_seed.wrapping_add(1);
        let generation = self.generation_seed;
        self.runtimes.insert(
            card_id.to_string(),
            TaskRuntime {
                page_id: page_id.to_string(),
                running: false,
                enabled: true,
                paused: false,
                run_once: false,
                generation,
                failure_count: 0,
                last_started: None,
                last_success: None,
                next_run,
                base_interval_secs: interval_secs.max(1),
                class: policy.class,
                idle_behavior: policy.idle_behavior,
                idle_multiplier: policy.idle_multiplier,
                external_realtime: policy.external_realtime,
                realtime_multiplier: policy.realtime_multiplier,
                minimum_interval_secs: policy.minimum_interval_secs,
                scheduled: policy.scheduled,
            },
        );
        self.heap.push(Reverse(ScheduledTask {
            next_run,
            card_id: card_id.to_string(),
            generation,
        }));
    }

    pub fn set_refresh_mode(&mut self, mode: RefreshMode) {
        if self.refresh_mode == mode {
            return;
        }
        self.refresh_mode = mode;
        let now = Instant::now();
        let mode = self.refresh_mode;
        for (card_id, runtime) in &mut self.runtimes {
            if runtime.running || runtime.scheduled || runtime.class == TaskClass::Static {
                continue;
            }
            let interval = effective_interval(mode, runtime, self.idle_strength);
            runtime.generation += 1;
            if mode != RefreshMode::Suspended && runtime.last_started.is_none() {
                runtime.next_run = now;
                self.heap.push(Reverse(ScheduledTask {
                    next_run: now,
                    card_id: card_id.clone(),
                    generation: runtime.generation,
                }));
            } else if let Some(interval) = interval {
                let anchor = runtime.last_success.or(runtime.last_started).unwrap_or(now);
                runtime.next_run = anchor + std::time::Duration::from_secs(interval);
                if runtime.next_run < now {
                    runtime.next_run = now;
                }
                self.heap.push(Reverse(ScheduledTask {
                    next_run: runtime.next_run,
                    card_id: card_id.clone(),
                    generation: runtime.generation,
                }));
            }
        }
    }

    pub fn set_saving_strength(&mut self, value: &str) {
        let strength = match value {
            "mild" => 0.65,
            "aggressive" => 1.75,
            _ => 1.0,
        };
        if (self.idle_strength - strength).abs() < f64::EPSILON {
            return;
        }
        self.idle_strength = strength;
        let current = self.refresh_mode;
        self.refresh_mode = RefreshMode::Normal;
        self.set_refresh_mode(current);
    }

    pub fn unregister(&mut self, card_id: &str) {
        self.runtimes.remove(card_id);
    }

    pub fn set_window_active(&mut self, active: bool) {
        self.window_active = active;
    }

    pub fn set_page_paused(&mut self, page_id: &str, paused: bool) {
        let now = Instant::now();
        for (card_id, runtime) in self
            .runtimes
            .iter_mut()
            .filter(|(_, runtime)| runtime.page_id == page_id)
        {
            if runtime.paused == paused {
                continue;
            }
            runtime.paused = paused;
            runtime.generation += 1;
            if !paused && !runtime.running {
                runtime.next_run = now;
                self.heap.push(Reverse(ScheduledTask {
                    next_run: now,
                    card_id: card_id.clone(),
                    generation: runtime.generation,
                }));
            }
        }
    }

    pub fn set_active_page(&mut self, page_id: &str) {
        let pages: std::collections::HashSet<String> = self
            .runtimes
            .values()
            .map(|runtime| runtime.page_id.clone())
            .collect();
        for page in pages {
            self.set_page_paused(&page, page != page_id);
        }
    }

    pub fn request_now(&mut self, card_id: &str) -> bool {
        if let Some(rt) = self.runtimes.get_mut(card_id) {
            if !rt.running {
                rt.run_once = true;
                rt.next_run = Instant::now();
                rt.generation += 1;
                self.heap.push(Reverse(ScheduledTask {
                    next_run: rt.next_run,
                    card_id: card_id.to_string(),
                    generation: rt.generation,
                }));
                return true;
            }
        }
        false
    }

    pub fn request_all_now(&mut self) {
        let now = Instant::now();
        for (card_id, runtime) in &mut self.runtimes {
            if runtime.running || !runtime.enabled {
                continue;
            }
            runtime.run_once = true;
            runtime.next_run = now;
            runtime.generation += 1;
            self.heap.push(Reverse(ScheduledTask {
                next_run: now,
                card_id: card_id.clone(),
                generation: runtime.generation,
            }));
        }
    }

    pub fn next_task(&mut self) -> Option<Instant> {
        if !self.window_active && self.paused_when_inactive {
            return None;
        }
        loop {
            let task = &self.heap.peek()?.0;
            let valid = self.runtimes.get(&task.card_id).is_some_and(|runtime| {
                runtime.generation == task.generation
                    && runtime.enabled
                    && (!runtime.paused || runtime.run_once)
                    && (runtime.run_once
                        || runtime.last_started.is_none()
                        || runtime.scheduled
                        || effective_interval(self.refresh_mode, runtime, self.idle_strength)
                            .is_some())
            });
            if valid {
                return Some(task.next_run);
            }
            self.heap.pop();
        }
    }

    pub fn poll(&mut self) -> Vec<String> {
        // Keep due entries in the heap while the window is inactive. Popping them
        // here would silently lose the task and it would never run after resume.
        if !self.window_active && self.paused_when_inactive {
            return Vec::new();
        }
        let now = Instant::now();
        let coalescing = match self.refresh_mode {
            RefreshMode::Realtime => std::time::Duration::from_millis(200),
            RefreshMode::Normal => std::time::Duration::from_millis(500),
            RefreshMode::Throttled => std::time::Duration::from_secs(3),
            RefreshMode::Suspended => std::time::Duration::from_secs(10),
        };
        let cutoff = now + coalescing;
        let mut ready = Vec::new();

        while let Some(Reverse(task)) = self.heap.peek() {
            if task.next_run > cutoff {
                break;
            }
            if task.next_run > now
                && self
                    .runtimes
                    .get(&task.card_id)
                    .is_some_and(|runtime| runtime.scheduled)
            {
                break;
            }
            let task = self.heap.pop().unwrap().0;

            if let Some(rt) = self.runtimes.get_mut(&task.card_id) {
                if rt.generation != task.generation {
                    continue;
                }
                if !rt.enabled
                    || rt.running
                    || (rt.paused && !rt.run_once)
                    || (!rt.run_once
                        && rt.last_started.is_some()
                        && !rt.scheduled
                        && effective_interval(self.refresh_mode, rt, self.idle_strength).is_none())
                {
                    continue;
                }
                rt.run_once = false;
            }
            ready.push(task.card_id);
        }

        ready
    }

    pub fn mark_started(&mut self, card_id: &str) {
        if let Some(rt) = self.runtimes.get_mut(card_id) {
            rt.running = true;
            rt.last_started = Some(Instant::now());
        }
    }

    pub fn mark_done(&mut self, card_id: &str, interval_secs: u64, success: bool) {
        self.mark_done_after(card_id, interval_secs, success, None);
    }

    pub fn mark_done_after(
        &mut self,
        card_id: &str,
        interval_secs: u64,
        success: bool,
        next_delay: Option<std::time::Duration>,
    ) {
        if let Some(rt) = self.runtimes.get_mut(card_id) {
            rt.running = false;
            if success {
                rt.failure_count = 0;
                rt.last_success = Some(Instant::now());
            } else {
                rt.failure_count += 1;
            }
            let policy_interval = if rt.scheduled {
                Some(interval_secs)
            } else {
                effective_interval(self.refresh_mode, rt, self.idle_strength)
            };
            let Some(policy_interval) = policy_interval else {
                rt.generation += 1;
                return;
            };
            let backoff = if success || rt.failure_count == 0 {
                policy_interval
            } else {
                policy_interval
                    .saturating_mul(2u64.pow(rt.failure_count.min(6)))
                    .min(policy_interval.saturating_mul(64).max(120))
            };
            rt.next_run = Instant::now()
                + next_delay.unwrap_or_else(|| std::time::Duration::from_secs(backoff));
            rt.generation += 1;
            self.heap.push(Reverse(ScheduledTask {
                next_run: rt.next_run,
                card_id: card_id.to_string(),
                generation: rt.generation,
            }));
        }
    }
}

fn effective_interval(mode: RefreshMode, runtime: &TaskRuntime, idle_strength: f64) -> Option<u64> {
    if runtime.scheduled {
        return Some(runtime.base_interval_secs);
    }
    if matches!(
        runtime.class,
        TaskClass::Static | TaskClass::File | TaskClass::NetworkStatus
    ) {
        return None;
    }
    let base = runtime.base_interval_secs.max(1);
    match mode {
        RefreshMode::Normal => Some(base),
        RefreshMode::Suspended => None,
        RefreshMode::Throttled => {
            if runtime.idle_behavior == IdleBehavior::Pause {
                return None;
            }
            let default = match runtime.class {
                TaskClass::SystemRealtime => 4.0,
                TaskClass::NetworkRate => 8.0,
                TaskClass::NetworkStatus => return None,
                TaskClass::BatteryThermal => 2.0,
                TaskClass::Command | TaskClass::Http => 10.0,
                TaskClass::File => 4.0,
                TaskClass::Static => return None,
                TaskClass::Other => 4.0,
            };
            Some(
                ((base as f64 * runtime.idle_multiplier.unwrap_or(default) * idle_strength).ceil()
                    as u64)
                    .max(1),
            )
        }
        RefreshMode::Realtime => {
            let allowed = runtime.external_realtime.unwrap_or(matches!(
                runtime.class,
                TaskClass::SystemRealtime | TaskClass::NetworkRate
            ));
            if !allowed {
                return Some(base);
            }
            let minimum = runtime.minimum_interval_secs.unwrap_or(2).max(1);
            Some(
                ((base as f64 * runtime.realtime_multiplier.unwrap_or(0.5)).ceil() as u64)
                    .max(minimum),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_active_page_becomes_ready() {
        let mut scheduler = Scheduler::new();
        scheduler.register("monitor-card", 5, "monitor");
        scheduler.register("other-card", 5, "other");
        scheduler.set_active_page("monitor");
        assert_eq!(scheduler.poll(), vec!["monitor-card".to_string()]);
        scheduler.set_active_page("other");
        assert_eq!(scheduler.poll(), vec!["other-card".to_string()]);
    }

    #[test]
    fn inactive_window_has_no_wakeup_deadline() {
        let mut scheduler = Scheduler::new();
        scheduler.register("card", 5, "monitor");
        scheduler.set_window_active(false);
        assert!(scheduler.next_task().is_none());
        scheduler.set_window_active(true);
        assert!(scheduler.next_task().is_some());
    }

    #[test]
    fn failures_back_off_and_success_resets() {
        let mut scheduler = Scheduler::new();
        scheduler.register("card", 5, "monitor");
        scheduler.poll();
        scheduler.mark_started("card");
        scheduler.mark_done("card", 5, false);
        assert_eq!(scheduler.runtimes["card"].failure_count, 1);
        assert!(
            scheduler.runtimes["card"].next_run
                >= Instant::now() + std::time::Duration::from_secs(9)
        );
        scheduler.mark_done("card", 5, true);
        assert_eq!(scheduler.runtimes["card"].failure_count, 0);
    }

    #[test]
    fn static_and_file_tasks_run_once_then_wait_for_events() {
        for class in [TaskClass::Static, TaskClass::File] {
            let mut scheduler = Scheduler::new();
            scheduler.register_with_policy(
                "card",
                5,
                "monitor",
                TaskPolicy {
                    class,
                    ..TaskPolicy::default()
                },
            );
            assert_eq!(scheduler.poll(), vec!["card"]);
            scheduler.mark_started("card");
            scheduler.mark_done("card", 5, true);
            assert!(scheduler.next_task().is_none());
            scheduler.request_now("card");
            assert!(scheduler.next_task().is_some());
        }
    }

    #[test]
    fn background_suspends_non_scheduled_tasks() {
        let mut scheduler = Scheduler::new();
        scheduler.register("card", 5, "monitor");
        scheduler.poll();
        scheduler.mark_started("card");
        scheduler.mark_done("card", 5, true);
        scheduler.set_refresh_mode(RefreshMode::Suspended);
        assert!(scheduler.next_task().is_none());
    }

    #[test]
    fn one_time_follow_up_does_not_change_the_base_interval() {
        let mut scheduler = Scheduler::new();
        scheduler.register("card", 5, "monitor");
        scheduler.poll();
        scheduler.mark_started("card");
        scheduler.mark_done_after("card", 5, true, Some(std::time::Duration::from_millis(250)));
        assert_eq!(scheduler.runtimes["card"].base_interval_secs, 5);
        assert!(
            scheduler.runtimes["card"].next_run
                < Instant::now() + std::time::Duration::from_secs(1)
        );
    }

    #[test]
    fn first_collection_runs_immediately_after_window_resumes() {
        for mode in [RefreshMode::Normal, RefreshMode::Realtime] {
            let mut scheduler = Scheduler::new();
            scheduler.register("card", 60, "monitor");
            scheduler.set_active_page("monitor");
            scheduler.set_window_active(false);
            scheduler.set_refresh_mode(RefreshMode::Suspended);
            assert!(scheduler.next_task().is_none());

            scheduler.set_window_active(true);
            scheduler.set_refresh_mode(mode);
            assert_eq!(scheduler.poll(), vec!["card"]);
        }
    }

    #[test]
    fn event_driven_cards_keep_their_first_collection_after_resume() {
        for class in [TaskClass::File, TaskClass::NetworkStatus] {
            let mut scheduler = Scheduler::new();
            scheduler.register_with_policy(
                "card",
                60,
                "monitor",
                TaskPolicy {
                    class,
                    ..TaskPolicy::default()
                },
            );
            scheduler.set_active_page("monitor");
            scheduler.set_window_active(false);
            scheduler.set_refresh_mode(RefreshMode::Suspended);
            scheduler.set_window_active(true);
            scheduler.set_refresh_mode(RefreshMode::Normal);
            assert_eq!(scheduler.poll(), vec!["card"]);

            scheduler.mark_started("card");
            scheduler.mark_done("card", 60, true);
            assert!(scheduler.next_task().is_none());
        }
    }

    #[test]
    fn completed_collection_keeps_its_interval_after_resume() {
        let mut scheduler = Scheduler::new();
        scheduler.register("card", 60, "monitor");
        assert_eq!(scheduler.poll(), vec!["card"]);
        scheduler.mark_started("card");
        scheduler.mark_done("card", 60, true);

        scheduler.set_window_active(false);
        scheduler.set_refresh_mode(RefreshMode::Suspended);
        scheduler.set_window_active(true);
        scheduler.set_refresh_mode(RefreshMode::Normal);

        assert!(scheduler.poll().is_empty());
        assert!(scheduler.next_task().is_some());
    }
}
