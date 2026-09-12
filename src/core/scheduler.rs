use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::time::{Duration, Instant};

use crate::core::refresh::{RefreshReason, SourceKey, SourceRevision};
use crate::core::runtime::WorkLevel;

const DEFAULT_EXPENSIVE_INTERVAL_FLOOR_SECS: u64 = 30 * 60;

fn deadline_after(now: Instant, duration: Duration) -> Instant {
    now.checked_add(duration).unwrap_or(now)
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workload {
    Live,
    Normal,
    Expensive,
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBehavior {
    Inherit,
    Keep,
    Throttle,
    Pause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkContext {
    Active,
    Inactive,
    Idle,
}

#[derive(Debug, Clone)]
pub struct TaskRuntime {
    pub page_id: String,
    pub running: bool,
    pub enabled: bool,
    pub paused: bool,
    pub run_once: bool,
    /// An event received while the collector was running. It is consumed as
    /// one immediate follow-up after completion, never dropped.
    pub pending_event: bool,
    /// Legacy scalar diagnostics retain the largest revision observed by this
    /// task. Source freshness itself is tracked per source-key domain below.
    pub requested_revision: SourceRevision,
    pub completed_revision: SourceRevision,
    pub active_revision: SourceRevision,
    pub requested_source_revisions: HashMap<SourceKey, SourceRevision>,
    pub completed_source_revisions: HashMap<SourceKey, SourceRevision>,
    pub active_source_revisions: HashMap<SourceKey, SourceRevision>,
    pub active_revision_key: Option<SourceKey>,
    pub pending_reason: Option<RefreshReason>,
    /// Reason captured when the current run was dequeued. Source revisions
    /// belong to this key, which matters for signal aliases.
    pub active_reason: Option<RefreshReason>,
    pub generation: u64,
    pub failure_count: u32,
    pub last_started: Option<Instant>,
    pub last_success: Option<Instant>,
    pub next_run: Instant,
    pub base_interval_secs: u64,
    pub workload: Workload,
    pub inactive_behavior: WorkBehavior,
    pub idle_behavior: WorkBehavior,
    pub inactive_interval_secs: Option<u64>,
    pub idle_interval_secs: Option<u64>,
    pub minimum_interval_secs: Option<u64>,
    pub scheduled: bool,
}

#[derive(Debug, Clone)]
pub struct TaskPolicy {
    pub workload: Workload,
    pub inactive_behavior: WorkBehavior,
    pub idle_behavior: WorkBehavior,
    pub inactive_interval_secs: Option<u64>,
    pub idle_interval_secs: Option<u64>,
    pub minimum_interval_secs: Option<u64>,
    pub scheduled: bool,
}

impl Default for TaskPolicy {
    fn default() -> Self {
        Self {
            workload: Workload::Normal,
            inactive_behavior: WorkBehavior::Inherit,
            idle_behavior: WorkBehavior::Inherit,
            inactive_interval_secs: None,
            idle_interval_secs: None,
            minimum_interval_secs: None,
            scheduled: false,
        }
    }
}

pub struct Scheduler {
    heap: BinaryHeap<Reverse<ScheduledTask>>,
    runtimes: HashMap<String, TaskRuntime>,
    generation_seed: u64,
    work_level: WorkLevel,
    context: WorkContext,
    periodic_refresh_paused: bool,
    /// Absolute slots used to spread remote/expensive work after a quiet or
    /// configuration boundary. User/manual and source-event requests remain
    /// immediate and are never put behind this cursor.
    resume_cursor: Option<Instant>,
    reload_cursor: Option<Instant>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            runtimes: HashMap::new(),
            generation_seed: 0,
            work_level: WorkLevel::Full,
            context: WorkContext::Active,
            periodic_refresh_paused: false,
            resume_cursor: None,
            reload_cursor: None,
        }
    }

    pub fn register_with_policy(
        &mut self,
        card_id: &str,
        interval_secs: u64,
        page_id: &str,
        policy: TaskPolicy,
    ) {
        self.register_with_policy_at(card_id, interval_secs, page_id, policy, Instant::now());
    }

    fn register_with_policy_at(
        &mut self,
        card_id: &str,
        interval_secs: u64,
        page_id: &str,
        policy: TaskPolicy,
        now: Instant,
    ) {
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
                pending_event: false,
                requested_revision: SourceRevision::INITIAL,
                completed_revision: SourceRevision::INITIAL,
                active_revision: SourceRevision::INITIAL,
                requested_source_revisions: HashMap::new(),
                completed_source_revisions: HashMap::new(),
                active_source_revisions: HashMap::new(),
                active_revision_key: None,
                pending_reason: None,
                active_reason: None,
                generation,
                failure_count: 0,
                last_started: None,
                last_success: None,
                next_run: now,
                base_interval_secs: interval_secs.max(1),
                workload: policy.workload,
                inactive_behavior: policy.inactive_behavior,
                idle_behavior: policy.idle_behavior,
                inactive_interval_secs: policy.inactive_interval_secs,
                idle_interval_secs: policy.idle_interval_secs,
                minimum_interval_secs: policy.minimum_interval_secs,
                scheduled: policy.scheduled,
            },
        );
        self.heap.push(Reverse(ScheduledTask {
            next_run: now,
            card_id: card_id.to_string(),
            generation,
        }));
    }

    pub fn set_work_level(&mut self, level: WorkLevel, inactive: bool, idle: bool) {
        self.set_work_level_at(level, inactive, idle, Instant::now());
    }

    fn set_work_level_at(&mut self, level: WorkLevel, inactive: bool, idle: bool, now: Instant) {
        let context = if idle {
            WorkContext::Idle
        } else if inactive {
            WorkContext::Inactive
        } else {
            WorkContext::Active
        };
        if self.work_level == level && self.context == context {
            return;
        }
        self.work_level = level;
        self.context = context;
        self.reschedule_at(now);
    }

    pub fn set_periodic_refresh_paused(&mut self, paused: bool) {
        self.set_periodic_refresh_paused_at(paused, Instant::now());
    }

    fn set_periodic_refresh_paused_at(&mut self, paused: bool, now: Instant) {
        if self.periodic_refresh_paused == paused {
            return;
        }
        let resumed = self.periodic_refresh_paused && !paused;
        self.periodic_refresh_paused = paused;
        self.resume_cursor = resumed.then_some(now);
        self.reschedule_at(now);
    }

    fn reschedule_at(&mut self, now: Instant) {
        let level = self.work_level;
        let context = self.context;
        let mut resume_cursor = self.resume_cursor.take();
        let mut card_ids = self.runtimes.keys().cloned().collect::<Vec<_>>();
        // Stable ordering makes the bounded resume contract deterministic and
        // gives one-shot/event work priority over expensive periodic work.
        card_ids.sort_by_key(|card_id| {
            let runtime = &self.runtimes[card_id];
            (
                !runtime.run_once,
                workload_rank(runtime.workload),
                card_id.clone(),
            )
        });

        for card_id in card_ids {
            let Some(runtime) = self.runtimes.get_mut(&card_id) else {
                continue;
            };
            if runtime.running {
                continue;
            }
            runtime.generation = runtime.generation.wrapping_add(1);
            if level == WorkLevel::Suspended {
                continue;
            }
            if runtime.run_once {
                // Configuration/wall-clock work may already have an assigned
                // resume slot. If quiet hours lasted beyond that slot, assign
                // a fresh bounded slot on release; manual and source-event
                // work remain immediate.
                let mut next = if matches!(runtime.pending_reason, Some(RefreshReason::WallClock)) {
                    runtime.next_run.max(now)
                } else {
                    now
                };
                if matches!(runtime.pending_reason, Some(RefreshReason::WallClock))
                    && runtime.workload == Workload::Expensive
                    && runtime.next_run <= now
                {
                    if let Some(cursor) = resume_cursor.as_mut() {
                        next = (*cursor).max(now);
                        *cursor = deadline_after(next, Duration::from_millis(500));
                    }
                }
                runtime.next_run = next;
                self.heap.push(Reverse(ScheduledTask {
                    next_run: next,
                    card_id,
                    generation: runtime.generation,
                }));
                continue;
            }
            if self.periodic_refresh_paused || runtime.paused {
                continue;
            }

            let next = if runtime.last_started.is_none() {
                now
            } else if runtime.scheduled {
                // Wall-clock tasks retain their absolute slot and are never
                // moved earlier by a quiet-hours boundary.
                runtime.next_run.max(now)
            } else {
                let Some(interval) = effective_interval(level, context, runtime) else {
                    continue;
                };
                let anchor = runtime.last_success.or(runtime.last_started).unwrap_or(now);
                deadline_after(anchor, Duration::from_secs(interval)).max(now)
            };
            let mut next = next;
            if next <= now && runtime.workload == Workload::Expensive {
                if let Some(cursor) = resume_cursor.as_mut() {
                    next = (*cursor).max(now);
                    *cursor = deadline_after(next, Duration::from_millis(500));
                }
            }
            runtime.next_run = next;
            self.heap.push(Reverse(ScheduledTask {
                next_run: next,
                card_id,
                generation: runtime.generation,
            }));
        }
        self.compact_heap_if_needed();
    }

    pub fn unregister(&mut self, card_id: &str) {
        self.runtimes.remove(card_id);
        self.compact_heap_if_needed();
    }

    /// Remove every live task before a GTK page tree is rebuilt. Keep the
    /// monotonic generation seed: workers from the previous page generation
    /// may still finish later, and their completion must not be mistaken for
    /// a newly registered task with the same card id.
    pub fn clear(&mut self) {
        self.heap.clear();
        self.runtimes.clear();
        self.resume_cursor = None;
        self.reload_cursor = None;
    }

    /// Registration and policy changes invalidate heap entries by generation.
    /// BinaryHeap cannot remove those entries in place, so periodically rebuild
    /// it from the still-current runtime slots to keep repeated hot reloads
    /// from turning stale scheduling metadata into an unbounded allocation.
    fn compact_heap_if_needed(&mut self) {
        let limit = self.runtimes.len().saturating_mul(4).saturating_add(32);
        if self.heap.len() <= limit {
            return;
        }
        let mut compacted = BinaryHeap::with_capacity(self.runtimes.len());
        while let Some(Reverse(task)) = self.heap.pop() {
            if self.runtimes.get(&task.card_id).is_some_and(|runtime| {
                runtime.enabled
                    && runtime.generation == task.generation
                    && runtime.next_run == task.next_run
            }) {
                compacted.push(Reverse(task));
            }
        }
        self.heap = compacted;
    }

    pub fn is_running(&self, card_id: &str) -> bool {
        self.runtimes
            .get(card_id)
            .is_some_and(|runtime| runtime.running)
    }

    /// Rebind a task's current policy without replacing an active collection.
    /// The active generation, wall-clock slot, pending events, and revision
    /// bookkeeping remain owned by the existing runtime until its worker
    /// completes. A replacement registration while `running` would otherwise
    /// allow the old slot and new slot to execute concurrently.
    pub fn rebind_with_policy(
        &mut self,
        card_id: &str,
        interval_secs: u64,
        page_id: &str,
        policy: TaskPolicy,
    ) -> bool {
        let Some(runtime) = self.runtimes.get_mut(card_id) else {
            return false;
        };
        runtime.page_id = page_id.to_owned();
        runtime.base_interval_secs = interval_secs.max(1);
        runtime.workload = policy.workload;
        runtime.inactive_behavior = policy.inactive_behavior;
        runtime.idle_behavior = policy.idle_behavior;
        runtime.inactive_interval_secs = policy.inactive_interval_secs;
        runtime.idle_interval_secs = policy.idle_interval_secs;
        runtime.minimum_interval_secs = policy.minimum_interval_secs;
        runtime.scheduled = policy.scheduled;
        true
    }

    pub fn active_revision(&self, card_id: &str) -> SourceRevision {
        self.runtimes
            .get(card_id)
            .map(|runtime| runtime.active_revision)
            .unwrap_or(SourceRevision::INITIAL)
    }

    pub fn generation(&self, card_id: &str) -> Option<u64> {
        self.runtimes.get(card_id).map(|runtime| runtime.generation)
    }

    pub fn next_run(&self, card_id: &str) -> Option<Instant> {
        self.runtimes.get(card_id).map(|runtime| runtime.next_run)
    }

    /// Restore a fixed schedule deadline after rebinding a card. The new
    /// generation invalidates the registration's initial heap entry while
    /// preserving the wall-clock anchor across configuration edits.
    pub fn restore_deadline(&mut self, card_id: &str, deadline: Instant) {
        let Some(runtime) = self.runtimes.get_mut(card_id) else {
            return;
        };
        if !runtime.scheduled {
            return;
        }
        runtime.next_run = deadline;
        runtime.generation = runtime.generation.wrapping_add(1);
        self.heap.push(Reverse(ScheduledTask {
            next_run: deadline,
            card_id: card_id.to_string(),
            generation: runtime.generation,
        }));
    }

    /// Return the source-event key that caused the currently active run, if
    /// any. Revisions are scoped to their source key; signal aliases must not
    /// be compared numerically with a card's canonical source revision.
    pub fn active_source_key(&self, card_id: &str) -> Option<SourceKey> {
        self.runtimes
            .get(card_id)
            .and_then(|runtime| runtime.active_reason.as_ref())
            .and_then(|reason| match reason {
                RefreshReason::Source(key) => Some(key.clone()),
                RefreshReason::Periodic | RefreshReason::Manual | RefreshReason::WallClock => None,
            })
    }

    fn set_page_paused_at(&mut self, page_id: &str, paused: bool, now: Instant) {
        for (card_id, runtime) in self
            .runtimes
            .iter_mut()
            .filter(|(_, runtime)| runtime.page_id == page_id)
        {
            if runtime.paused == paused {
                continue;
            }
            runtime.paused = paused;
            // A worker has already captured this generation. Do not invalidate
            // its completion while it is in flight: the receiver must clear
            // `running` and consume any pending source event before the page
            // can resume normally.
            if !runtime.running {
                runtime.generation = runtime.generation.wrapping_add(1);
            }
            if !paused
                && !runtime.running
                && self.work_level != WorkLevel::Suspended
                && !self.periodic_refresh_paused
            {
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
        let now = Instant::now();
        for page in pages {
            self.set_page_paused_at(&page, page != page_id, now);
        }
    }

    pub fn request_now(&mut self, card_id: &str) -> bool {
        self.request_now_at(card_id, Instant::now())
    }

    fn request_now_at(&mut self, card_id: &str, now: Instant) -> bool {
        self.request_event_at(card_id, SourceRevision::INITIAL, RefreshReason::Manual, now)
    }

    /// Queue one refresh for a successfully reloaded configuration. This is a
    /// non-user request: it is suppressed by quiet hours and spreads expensive
    /// cards over deterministic 500 ms slots instead of creating a reload
    /// stampede.
    pub fn request_config_reload(&mut self, card_id: &str) -> bool {
        self.request_event_at_with_defer(
            card_id,
            SourceRevision::INITIAL,
            RefreshReason::WallClock,
            Instant::now(),
            true,
        )
    }

    pub fn request_with_reason(
        &mut self,
        card_id: &str,
        reason: RefreshReason,
        revision: SourceRevision,
    ) -> bool {
        self.request_event_at(card_id, revision, reason, Instant::now())
    }

    fn request_event_at(
        &mut self,
        card_id: &str,
        revision: SourceRevision,
        reason: RefreshReason,
        now: Instant,
    ) -> bool {
        self.request_event_at_with_defer(card_id, revision, reason, now, false)
    }

    fn request_event_at_with_defer(
        &mut self,
        card_id: &str,
        revision: SourceRevision,
        reason: RefreshReason,
        now: Instant,
        defer_expensive: bool,
    ) -> bool {
        let Some(existing) = self.runtimes.get(card_id) else {
            return false;
        };
        if !existing.enabled {
            return false;
        }
        let deferred_run = if defer_expensive
            && existing.workload == Workload::Expensive
            && !existing.running
            && !existing.run_once
        {
            let slot = self.reload_cursor.unwrap_or(now).max(now);
            self.reload_cursor = Some(deadline_after(slot, Duration::from_millis(500)));
            slot
        } else {
            now
        };
        let rt = self
            .runtimes
            .get_mut(card_id)
            .expect("task was present and enabled");
        let is_new_revision = match &reason {
            RefreshReason::Source(key) => {
                let requested = rt
                    .requested_source_revisions
                    .entry(key.clone())
                    .or_default();
                let is_new = revision > *requested;
                if is_new {
                    *requested = revision;
                }
                is_new
            }
            RefreshReason::Periodic | RefreshReason::Manual | RefreshReason::WallClock => false,
        };
        // Keep the scalar for diagnostics and compatibility with existing
        // callers, but never use it to compare revisions from another key.
        if revision > rt.requested_revision {
            rt.requested_revision = revision;
        }
        if rt.running {
            // A completion may already be in flight for an older revision;
            // retain one dirty follow-up for the newest source state.
            rt.pending_event = true;
            rt.pending_reason = Some(prefer_reason(rt.pending_reason.take(), reason));
            return true;
        }
        if rt.run_once {
            // Do not grow the heap for duplicate events/manual clicks. The
            // existing entry will observe the latest requested revision.
            if is_new_revision
                || reason_priority(&reason) > reason_priority_of(rt.pending_reason.as_ref())
            {
                rt.pending_reason = Some(prefer_reason(rt.pending_reason.take(), reason));
            }
            return true;
        }
        rt.run_once = true;
        rt.pending_reason = Some(reason);
        rt.next_run = deferred_run;
        rt.generation = rt.generation.wrapping_add(1);
        self.heap.push(Reverse(ScheduledTask {
            next_run: deferred_run,
            card_id: card_id.to_string(),
            generation: rt.generation,
        }));
        true
    }

    pub fn request_all_now(&mut self) {
        let now = Instant::now();
        let card_ids = self.runtimes.keys().cloned().collect::<Vec<_>>();
        for card_id in card_ids {
            let _ = self.request_event_at(
                &card_id,
                SourceRevision::INITIAL,
                RefreshReason::Manual,
                now,
            );
        }
    }

    pub fn next_task(&mut self) -> Option<Instant> {
        if self.work_level == WorkLevel::Suspended {
            self.compact_heap_if_needed();
            return None;
        }
        let mut deferred = Vec::new();
        let result = loop {
            let Some(task) = self.heap.peek().map(|task| task.0.clone()) else {
                break None;
            };
            let Some(runtime) = self.runtimes.get(&task.card_id) else {
                self.heap.pop();
                continue;
            };
            if runtime.generation != task.generation || !runtime.enabled {
                self.heap.pop();
                continue;
            }
            if runtime.run_once {
                // A config-reload/wall-clock request remains in the heap until
                // quiet hours release it. Never discard it as a stale entry;
                // temporarily skip it so a later manual/source event can run.
                if !run_once_permitted(
                    runtime.pending_reason.as_ref(),
                    self.periodic_refresh_paused,
                ) {
                    deferred.push(self.heap.pop().expect("peeked task"));
                    continue;
                }
                break Some(task.next_run);
            }
            if self.periodic_refresh_paused
                || runtime.paused
                || (runtime.last_started.is_some()
                    && effective_interval(self.work_level, self.context, runtime).is_none())
            {
                self.heap.pop();
                continue;
            }
            break Some(task.next_run);
        };
        self.heap.extend(deferred);
        self.compact_heap_if_needed();
        result
    }

    pub fn poll(&mut self) -> Vec<String> {
        self.poll_at(Instant::now())
    }

    fn poll_at(&mut self, now: Instant) -> Vec<String> {
        if self.work_level == WorkLevel::Suspended {
            return Vec::new();
        }
        let coalescing = match self.work_level {
            WorkLevel::Full => Duration::from_millis(500),
            WorkLevel::Reduced => Duration::from_secs(2),
            WorkLevel::Minimal => Duration::from_secs(3),
            WorkLevel::Suspended => Duration::ZERO,
        };
        let cutoff = deadline_after(now, coalescing);
        let mut ready = Vec::new();
        let mut deferred = Vec::new();
        while let Some(Reverse(task)) = self.heap.peek() {
            let task = task.clone();
            let Some(runtime) = self.runtimes.get(&task.card_id) else {
                self.heap.pop();
                continue;
            };
            if task.next_run > cutoff {
                break;
            }
            // Fixed schedules and config-reload slots are absolute deadlines;
            // never let the general coalescing window execute them early.
            if task.next_run > now
                && (runtime.scheduled
                    || runtime.workload == Workload::Expensive
                    || matches!(runtime.pending_reason, Some(RefreshReason::WallClock)))
            {
                break;
            }
            if runtime.run_once
                && !run_once_permitted(
                    runtime.pending_reason.as_ref(),
                    self.periodic_refresh_paused,
                )
            {
                // Keep the queued request for the quiet-hours boundary rather
                // than losing it, while allowing a later manual/source event
                // to proceed during the same quiet interval.
                deferred.push(self.heap.pop().expect("peeked task"));
                continue;
            }
            let task = self.heap.pop().unwrap().0;
            if let Some(rt) = self.runtimes.get_mut(&task.card_id) {
                if rt.generation != task.generation {
                    continue;
                }
                if !rt.enabled
                    || rt.running
                    || (self.periodic_refresh_paused && !rt.run_once)
                    || (rt.paused && !rt.run_once)
                    || (!rt.run_once
                        && rt.last_started.is_some()
                        && effective_interval(self.work_level, self.context, rt).is_none())
                {
                    continue;
                }
                rt.run_once = false;
                rt.active_reason = rt.pending_reason.take();
                rt.active_source_revisions = rt.requested_source_revisions.clone();
                rt.active_revision_key =
                    rt.active_reason.as_ref().and_then(|reason| match reason {
                        RefreshReason::Source(key) => Some(key.clone()),
                        RefreshReason::Periodic
                        | RefreshReason::Manual
                        | RefreshReason::WallClock => None,
                    });
                rt.active_revision = rt
                    .active_revision_key
                    .as_ref()
                    .and_then(|key| rt.active_source_revisions.get(key).copied())
                    .unwrap_or(SourceRevision::INITIAL);
            }
            ready.push(task.card_id);
        }
        self.heap.extend(deferred);
        self.compact_heap_if_needed();
        ready
    }

    pub fn mark_started(&mut self, card_id: &str) {
        self.mark_started_at(card_id, Instant::now());
    }

    fn mark_started_at(&mut self, card_id: &str, now: Instant) {
        if let Some(rt) = self.runtimes.get_mut(card_id) {
            rt.running = true;
            rt.last_started = Some(now);
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
        next_delay: Option<Duration>,
    ) {
        self.mark_done_after_at(card_id, interval_secs, success, next_delay, Instant::now());
    }

    #[cfg(test)]
    pub fn mark_done_after_revision(
        &mut self,
        card_id: &str,
        interval_secs: u64,
        success: bool,
        next_delay: Option<Duration>,
        revision: SourceRevision,
    ) {
        self.mark_done_after_revision_with_deadline(
            card_id,
            interval_secs,
            success,
            next_delay,
            None,
            revision,
        );
    }

    pub fn mark_done_after_revision_with_deadline(
        &mut self,
        card_id: &str,
        interval_secs: u64,
        success: bool,
        next_delay: Option<Duration>,
        next_deadline: Option<Instant>,
        revision: SourceRevision,
    ) {
        if let Some(runtime) = self.runtimes.get_mut(card_id) {
            runtime.active_revision = revision;
        }
        self.mark_done_after_at_with_deadline(
            card_id,
            interval_secs,
            success,
            next_delay,
            next_deadline,
            Instant::now(),
        );
    }

    fn mark_done_after_at(
        &mut self,
        card_id: &str,
        interval_secs: u64,
        success: bool,
        next_delay: Option<Duration>,
        now: Instant,
    ) {
        self.mark_done_after_at_with_deadline(
            card_id,
            interval_secs,
            success,
            next_delay,
            None,
            now,
        );
    }

    fn mark_done_after_at_with_deadline(
        &mut self,
        card_id: &str,
        interval_secs: u64,
        success: bool,
        next_delay: Option<Duration>,
        next_deadline: Option<Instant>,
        now: Instant,
    ) {
        if let Some(rt) = self.runtimes.get_mut(card_id) {
            rt.running = false;
            rt.active_reason = None;
            rt.active_revision_key = None;
            rt.completed_revision = rt.active_revision;
            for (key, revision) in rt.active_source_revisions.drain() {
                let completed = rt.completed_source_revisions.entry(key).or_default();
                *completed = (*completed).max(revision);
            }
            let pending_source = rt.requested_source_revisions.iter().any(|(key, revision)| {
                *revision
                    > rt.completed_source_revisions
                        .get(key)
                        .copied()
                        .unwrap_or(SourceRevision::INITIAL)
            });
            if success {
                rt.failure_count = 0;
                rt.last_success = Some(now);
            } else {
                rt.failure_count = rt.failure_count.saturating_add(1);
            }
            let pending = rt.pending_event || pending_source;
            if pending {
                // Collapse all edges observed during the collection into one
                // follow-up for the latest revision. Keep it queued through
                // quiet/suspended permits instead of dropping it.
                rt.pending_event = false;
                let pending_reason = rt.pending_reason.take();
                rt.pending_reason = pending_reason;
                rt.run_once = true;
                rt.next_run = now;
                rt.generation = rt.generation.wrapping_add(1);
                if self.work_level != WorkLevel::Suspended {
                    self.heap.push(Reverse(ScheduledTask {
                        next_run: now,
                        card_id: card_id.to_string(),
                        generation: rt.generation,
                    }));
                }
                rt.requested_source_revisions.retain(|key, revision| {
                    rt.completed_source_revisions
                        .get(key)
                        .is_none_or(|completed| *revision > *completed)
                });
                return;
            }
            rt.requested_source_revisions.clear();
            rt.completed_source_revisions.clear();
            rt.pending_reason = None;
            let policy_interval = if rt.scheduled {
                Some(interval_secs)
            } else {
                effective_interval(self.work_level, self.context, rt)
            };
            let Some(policy_interval) = policy_interval else {
                rt.generation = rt.generation.wrapping_add(1);
                return;
            };
            let backoff = if success || rt.failure_count == 0 {
                policy_interval
            } else {
                policy_interval
                    .saturating_mul(2u64.pow(rt.failure_count.min(6)))
                    .min(policy_interval.saturating_mul(64).max(120))
            };
            rt.next_run = next_deadline.unwrap_or_else(|| {
                deadline_after(
                    now,
                    next_delay.unwrap_or_else(|| Duration::from_secs(backoff)),
                )
            });
            rt.generation = rt.generation.wrapping_add(1);
            if self.work_level != WorkLevel::Suspended && !self.periodic_refresh_paused {
                self.heap.push(Reverse(ScheduledTask {
                    next_run: rt.next_run,
                    card_id: card_id.to_string(),
                    generation: rt.generation,
                }));
            }
        }
    }
}

fn reason_priority(reason: &RefreshReason) -> u8 {
    match reason {
        RefreshReason::Manual => 3,
        RefreshReason::Source(_) => 2,
        RefreshReason::WallClock => 1,
        RefreshReason::Periodic => 0,
    }
}

fn reason_priority_of(reason: Option<&RefreshReason>) -> u8 {
    reason.map(reason_priority).unwrap_or(0)
}

fn prefer_reason(current: Option<RefreshReason>, next: RefreshReason) -> RefreshReason {
    if current
        .as_ref()
        .is_some_and(|current| reason_priority(current) > reason_priority(&next))
    {
        current.expect("checked above")
    } else {
        next
    }
}

fn run_once_permitted(reason: Option<&RefreshReason>, periodic_refresh_paused: bool) -> bool {
    if !periodic_refresh_paused {
        return true;
    }
    // Manual and source-event requests are explicit observations/signals and
    // remain permitted in quiet hours. Wall-clock/config requests are
    // unattended work and wait for the quiet permit to reopen.
    !matches!(
        reason,
        Some(RefreshReason::WallClock | RefreshReason::Periodic)
    )
}

fn workload_rank(workload: Workload) -> u8 {
    match workload {
        Workload::Event => 0,
        Workload::Live => 1,
        Workload::Normal => 2,
        Workload::Expensive => 3,
    }
}

fn effective_interval(
    level: WorkLevel,
    context: WorkContext,
    runtime: &TaskRuntime,
) -> Option<u64> {
    if level == WorkLevel::Suspended {
        return None;
    }
    if runtime.scheduled {
        return Some(clamp_min(runtime.base_interval_secs, runtime));
    }
    if runtime.workload == Workload::Event {
        return None;
    }
    let (behavior, explicit) = match context {
        WorkContext::Inactive => (runtime.inactive_behavior, runtime.inactive_interval_secs),
        WorkContext::Idle => (runtime.idle_behavior, runtime.idle_interval_secs),
        WorkContext::Active => (WorkBehavior::Inherit, None),
    };
    if behavior == WorkBehavior::Pause {
        return None;
    }
    if behavior == WorkBehavior::Keep {
        return Some(clamp_min(runtime.base_interval_secs, runtime));
    }
    if behavior == WorkBehavior::Throttle {
        if let Some(interval) = explicit {
            return Some(clamp_min(interval.max(runtime.base_interval_secs), runtime));
        }
    } else if level == WorkLevel::Full {
        let interval = if runtime.workload == Workload::Expensive {
            runtime
                .base_interval_secs
                .max(DEFAULT_EXPENSIVE_INTERVAL_FLOOR_SECS)
        } else {
            runtime.base_interval_secs
        };
        return Some(clamp_min(interval, runtime));
    }
    let throttle_level = if behavior == WorkBehavior::Throttle && level == WorkLevel::Full {
        match context {
            WorkContext::Inactive => WorkLevel::Reduced,
            WorkContext::Idle => WorkLevel::Minimal,
            WorkContext::Active => WorkLevel::Full,
        }
    } else {
        level
    };
    let multiplier = match (throttle_level, runtime.workload) {
        (WorkLevel::Reduced, Workload::Live) => 2,
        (WorkLevel::Reduced, Workload::Normal) => 3,
        (WorkLevel::Reduced, Workload::Expensive) => return None,
        (WorkLevel::Minimal, Workload::Live) => 4,
        (WorkLevel::Minimal, Workload::Normal) => 6,
        (WorkLevel::Minimal, Workload::Expensive) => return None,
        (_, Workload::Event) => return None,
        _ => 1,
    };
    Some(clamp_min(
        runtime.base_interval_secs.saturating_mul(multiplier),
        runtime,
    ))
}

fn clamp_min(interval: u64, runtime: &TaskRuntime) -> u64 {
    interval
        .max(runtime.minimum_interval_secs.unwrap_or(1))
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn transition_reschedules_from_last_success_with_fake_clock() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at("card", 10, "monitor", TaskPolicy::default(), start);
        assert_eq!(scheduler.poll_at(start), vec!["card"]);
        {
            let runtime = scheduler.runtimes.get_mut("card").unwrap();
            runtime.running = true;
            runtime.last_started = Some(start);
        }
        scheduler.mark_done_after_at("card", 10, true, None, start);
        scheduler.set_work_level_at(
            WorkLevel::Reduced,
            true,
            false,
            start + Duration::from_secs(2),
        );
        assert_eq!(
            scheduler.runtimes["card"].next_run,
            start + Duration::from_secs(30)
        );
        scheduler.set_work_level_at(
            WorkLevel::Full,
            false,
            false,
            start + Duration::from_secs(3),
        );
        assert_eq!(
            scheduler.runtimes["card"].next_run,
            start + Duration::from_secs(10)
        );
    }

    #[test]
    fn automatic_expensive_work_uses_a_monitor_floor_and_pauses_under_safety_caps() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at(
            "expensive",
            5,
            "monitor",
            TaskPolicy {
                workload: Workload::Expensive,
                ..TaskPolicy::default()
            },
            start,
        );
        scheduler.poll_at(start);
        let runtime = scheduler.runtimes.get_mut("expensive").unwrap();
        runtime.last_started = Some(start);
        runtime.last_success = Some(start);
        scheduler.reschedule_at(start);
        assert_eq!(
            scheduler.runtimes["expensive"].next_run,
            start + Duration::from_secs(DEFAULT_EXPENSIVE_INTERVAL_FLOOR_SECS)
        );

        scheduler.set_work_level_at(WorkLevel::Reduced, true, false, start);
        assert!(scheduler.next_task().is_none());
        scheduler.set_work_level_at(WorkLevel::Minimal, false, true, start);
        assert!(scheduler.next_task().is_none());
    }

    #[test]
    fn quiet_hours_pause_periodic_work_but_allow_manual_refresh() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at("card", 5, "monitor", TaskPolicy::default(), start);
        scheduler.set_periodic_refresh_paused_at(true, start);
        assert!(scheduler.next_task().is_none());
        assert!(scheduler.poll_at(start).is_empty());

        assert!(scheduler.request_now_at("card", start + Duration::from_secs(1)));
        assert_eq!(
            scheduler.poll_at(start + Duration::from_secs(1)),
            vec!["card"]
        );

        scheduler.set_periodic_refresh_paused_at(false, start + Duration::from_secs(2));
        assert_eq!(
            scheduler.poll_at(start + Duration::from_secs(2)),
            vec!["card"]
        );
    }

    #[test]
    fn explicit_context_behavior_applies_even_when_work_level_is_full() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at("card", 10, "monitor", TaskPolicy::default(), start);
        let runtime = scheduler.runtimes.get_mut("card").unwrap();

        runtime.inactive_behavior = WorkBehavior::Pause;
        assert_eq!(
            effective_interval(WorkLevel::Full, WorkContext::Inactive, runtime),
            None
        );

        runtime.inactive_behavior = WorkBehavior::Throttle;
        runtime.inactive_interval_secs = Some(25);
        assert_eq!(
            effective_interval(WorkLevel::Full, WorkContext::Inactive, runtime),
            Some(25)
        );

        runtime.inactive_interval_secs = None;
        assert_eq!(
            effective_interval(WorkLevel::Full, WorkContext::Inactive, runtime),
            Some(30)
        );

        runtime.inactive_behavior = WorkBehavior::Keep;
        assert_eq!(
            effective_interval(WorkLevel::Full, WorkContext::Inactive, runtime),
            Some(10)
        );
    }

    #[test]
    fn explicit_behavior_intervals_and_minimum_are_honored() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at(
            "card",
            5,
            "monitor",
            TaskPolicy {
                inactive_behavior: WorkBehavior::Throttle,
                idle_behavior: WorkBehavior::Keep,
                inactive_interval_secs: Some(2),
                minimum_interval_secs: Some(4),
                ..TaskPolicy::default()
            },
            start,
        );
        let rt = &scheduler.runtimes["card"];
        assert_eq!(
            effective_interval(WorkLevel::Reduced, WorkContext::Inactive, rt),
            Some(5)
        );
        assert_eq!(
            effective_interval(WorkLevel::Minimal, WorkContext::Idle, rt),
            Some(5)
        );
    }

    #[test]
    fn active_thermal_reduction_throttles_instead_of_being_ignored() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at("card", 10, "monitor", TaskPolicy::default(), start);
        let rt = &scheduler.runtimes["card"];
        assert_eq!(
            effective_interval(WorkLevel::Reduced, WorkContext::Active, rt),
            Some(30)
        );
    }

    #[test]
    fn active_source_key_preserves_revision_domain_for_signal_aliases() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at("card", 30, "monitor", TaskPolicy::default(), start);
        let source = SourceKey::new("signal:power-supply");
        assert!(scheduler.request_event_at(
            "card",
            SourceRevision(7),
            RefreshReason::Source(source.clone()),
            start,
        ));
        assert_eq!(scheduler.poll_at(start), vec!["card"]);
        assert_eq!(scheduler.active_source_key("card"), Some(source));
    }

    #[test]
    fn source_revision_domains_remain_independent_across_coalesced_events() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at("card", 30, "monitor", TaskPolicy::default(), start);
        assert_eq!(scheduler.poll_at(start), vec!["card"]);
        scheduler.mark_started_at("card", start);

        let signal = SourceKey::new("signal:power-supply");
        let canonical = SourceKey::new("builtin:power");
        assert!(scheduler.request_event_at(
            "card",
            SourceRevision(2),
            RefreshReason::Source(signal.clone()),
            start,
        ));
        assert!(scheduler.request_event_at(
            "card",
            SourceRevision(1),
            RefreshReason::Source(canonical.clone()),
            start,
        ));
        scheduler.mark_done_after_revision("card", 30, true, None, SourceRevision::INITIAL);
        assert_eq!(scheduler.poll(), vec!["card"]);
        assert_eq!(scheduler.active_source_key("card"), Some(canonical.clone()));
        assert_eq!(scheduler.active_revision("card"), SourceRevision(1));

        scheduler.mark_started_at("card", Instant::now());
        assert!(scheduler.request_event_at(
            "card",
            SourceRevision(2),
            RefreshReason::Source(canonical.clone()),
            Instant::now(),
        ));
        scheduler.mark_done_after_revision("card", 30, true, None, SourceRevision(1));
        assert_eq!(scheduler.poll(), vec!["card"]);
        assert_eq!(scheduler.active_source_key("card"), Some(canonical));
        assert_eq!(scheduler.active_revision("card"), SourceRevision(2));
    }

    #[test]
    fn event_tasks_run_once_then_wait_for_events() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at(
            "card",
            5,
            "monitor",
            TaskPolicy {
                workload: Workload::Event,
                ..TaskPolicy::default()
            },
            start,
        );
        assert_eq!(scheduler.poll_at(start), vec!["card"]);
        scheduler.runtimes.get_mut("card").unwrap().running = true;
        scheduler.mark_done_after_at("card", 5, true, None, start);
        assert!(scheduler.next_task().is_none());
        assert!(scheduler.request_now_at("card", start + Duration::from_secs(1)));
        assert!(scheduler.next_task().is_some());
    }

    #[test]
    fn queued_event_refresh_survives_suspended_transition() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at(
            "event",
            5,
            "monitor",
            TaskPolicy {
                workload: Workload::Event,
                ..TaskPolicy::default()
            },
            start,
        );
        assert_eq!(scheduler.poll_at(start), vec!["event"]);
        scheduler.runtimes.get_mut("event").unwrap().running = true;
        scheduler.mark_done_after_at("event", 5, true, None, start);
        scheduler.set_work_level_at(WorkLevel::Suspended, false, false, start);
        assert!(scheduler.request_now_at("event", start + Duration::from_secs(1)));
        scheduler.set_work_level_at(
            WorkLevel::Full,
            false,
            false,
            start + Duration::from_secs(2),
        );
        assert_eq!(
            scheduler.poll_at(start + Duration::from_secs(2)),
            vec!["event"]
        );
    }

    #[test]
    fn suspended_is_a_hard_stop_and_queued_manual_refresh_resumes() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at("card", 5, "monitor", TaskPolicy::default(), start);
        scheduler.set_work_level_at(WorkLevel::Suspended, false, false, start);
        assert!(scheduler.request_now_at("card", start));
        assert!(scheduler.next_task().is_none());
        assert!(scheduler.poll_at(start).is_empty());
        scheduler.set_work_level_at(WorkLevel::Full, false, false, start);
        assert_eq!(scheduler.poll_at(start), vec!["card"]);
    }

    #[test]
    fn expensive_scheduled_cards_remain_due_when_inactive_or_idle() {
        let start = now();
        for (level, inactive, idle) in [
            (WorkLevel::Reduced, true, false),
            (WorkLevel::Minimal, false, true),
        ] {
            let mut scheduler = Scheduler::new();
            scheduler.register_with_policy_at(
                "scheduled",
                60,
                "monitor",
                TaskPolicy {
                    workload: Workload::Expensive,
                    scheduled: true,
                    ..TaskPolicy::default()
                },
                start,
            );
            assert_eq!(scheduler.poll_at(start), vec!["scheduled"]);
            {
                let runtime = scheduler.runtimes.get_mut("scheduled").unwrap();
                runtime.running = true;
                runtime.last_started = Some(start);
            }
            scheduler.mark_done_after_at(
                "scheduled",
                60,
                true,
                Some(Duration::from_secs(100)),
                start,
            );
            scheduler.set_work_level_at(level, inactive, idle, start + Duration::from_secs(1));
            assert_eq!(
                scheduler.poll_at(start + Duration::from_secs(100)),
                vec!["scheduled"]
            );
        }
    }

    #[test]
    fn scheduled_completion_while_suspended_preserves_its_next_deadline() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at(
            "scheduled",
            60,
            "monitor",
            TaskPolicy {
                scheduled: true,
                ..TaskPolicy::default()
            },
            start,
        );
        assert_eq!(scheduler.poll_at(start), vec!["scheduled"]);
        {
            let runtime = scheduler.runtimes.get_mut("scheduled").unwrap();
            runtime.running = true;
            runtime.last_started = Some(start);
        }
        scheduler.set_work_level_at(WorkLevel::Suspended, false, false, start);
        scheduler.mark_done_after_at("scheduled", 60, true, Some(Duration::from_secs(300)), start);
        assert!(scheduler.next_task().is_none());
        scheduler.set_work_level_at(
            WorkLevel::Full,
            false,
            false,
            start + Duration::from_secs(10),
        );
        assert_eq!(
            scheduler.runtimes["scheduled"].next_run,
            start + Duration::from_secs(300)
        );
    }

    #[test]
    fn event_while_running_is_coalesced_into_one_follow_up() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at(
            "event",
            30,
            "monitor",
            TaskPolicy {
                workload: Workload::Event,
                ..TaskPolicy::default()
            },
            start,
        );
        assert_eq!(scheduler.poll_at(start), vec!["event"]);
        scheduler.mark_started_at("event", start);
        assert!(scheduler.request_with_reason(
            "event",
            RefreshReason::Source(crate::core::refresh::SourceKey::new("state")),
            SourceRevision(1),
        ));
        assert!(scheduler.request_with_reason(
            "event",
            RefreshReason::Source(crate::core::refresh::SourceKey::new("state")),
            SourceRevision(2),
        ));
        scheduler.mark_done_after_revision("event", 30, true, None, SourceRevision::INITIAL);
        assert_eq!(scheduler.poll(), vec!["event"]);
        assert_eq!(
            scheduler.runtimes["event"].requested_revision,
            SourceRevision(2)
        );
    }

    #[test]
    fn duplicate_manual_requests_do_not_add_duplicate_ready_tasks() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at("card", 30, "monitor", TaskPolicy::default(), start);
        assert_eq!(scheduler.poll_at(start), vec!["card"]);
        scheduler.mark_started_at("card", start);
        scheduler.mark_done_after_at("card", 30, true, None, start);
        assert!(scheduler.request_now_at("card", start + Duration::from_secs(1)));
        assert!(scheduler.request_now_at("card", start + Duration::from_secs(1)));
        assert_eq!(
            scheduler.poll_at(start + Duration::from_secs(1)),
            vec!["card"]
        );
    }

    #[test]
    fn config_reload_requests_are_bounded_and_quiet_suppressed() {
        let start = now();
        let mut scheduler = Scheduler::new();
        for card in ["one", "two", "three"] {
            scheduler.register_with_policy_at(
                card,
                30,
                "monitor",
                TaskPolicy {
                    workload: Workload::Expensive,
                    ..TaskPolicy::default()
                },
                start,
            );
        }
        scheduler.set_periodic_refresh_paused_at(true, start);
        assert!(scheduler.request_config_reload("one"));
        assert!(scheduler.request_config_reload("two"));
        assert!(scheduler.request_config_reload("three"));
        // Wall-clock/config work waits in quiet hours while each task keeps its
        // assigned bounded resume slot.
        assert!(scheduler.poll_at(start).is_empty());
        scheduler.set_periodic_refresh_paused_at(false, start + Duration::from_secs(1));
        assert_eq!(
            scheduler.poll_at(start + Duration::from_secs(1)),
            vec!["one"]
        );
        assert!(scheduler.runtimes["two"].next_run >= start + Duration::from_millis(500));
        assert!(scheduler.runtimes["three"].next_run >= start + Duration::from_secs(1));
    }

    #[test]
    fn quiet_wallclock_request_does_not_starve_manual_event() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at(
            "reload",
            30,
            "monitor",
            TaskPolicy {
                workload: Workload::Expensive,
                ..TaskPolicy::default()
            },
            start,
        );
        scheduler.register_with_policy_at("manual", 30, "monitor", TaskPolicy::default(), start);
        scheduler.set_periodic_refresh_paused_at(true, start);
        assert!(scheduler.request_config_reload("reload"));
        assert!(scheduler.request_now_at("manual", start));
        assert_eq!(scheduler.poll_at(start), vec!["manual"]);
    }

    #[test]
    fn periodic_resume_spreads_expensive_cards_one_slot_at_a_time() {
        let start = now();
        let mut scheduler = Scheduler::new();
        for card in ["one", "two", "three"] {
            scheduler.register_with_policy_at(
                card,
                30,
                "monitor",
                TaskPolicy {
                    workload: Workload::Expensive,
                    ..TaskPolicy::default()
                },
                start,
            );
        }
        scheduler.set_periodic_refresh_paused_at(true, start);
        scheduler.set_periodic_refresh_paused_at(false, start);
        assert_eq!(scheduler.poll_at(start), vec!["one"]);
        assert!(scheduler.runtimes["two"].next_run >= start + Duration::from_millis(500));
        assert!(scheduler.runtimes["three"].next_run >= start + Duration::from_millis(500));
        assert!(
            scheduler.runtimes["two"].next_run >= start + Duration::from_secs(1)
                || scheduler.runtimes["three"].next_run >= start + Duration::from_secs(1)
        );
    }

    #[test]
    fn absolute_wallclock_deadline_is_not_completion_anchored() {
        let start = now();
        let deadline = start + Duration::from_secs(60);
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at(
            "scheduled",
            60,
            "monitor",
            TaskPolicy {
                scheduled: true,
                ..TaskPolicy::default()
            },
            start,
        );
        assert_eq!(scheduler.poll_at(start), vec!["scheduled"]);
        scheduler.mark_started_at("scheduled", start);
        scheduler.mark_done_after_at_with_deadline(
            "scheduled",
            60,
            true,
            None,
            Some(deadline),
            start + Duration::from_secs(10),
        );
        assert_eq!(scheduler.runtimes["scheduled"].next_run, deadline);
    }

    #[test]
    fn page_pause_non_reentrancy_backoff_and_coalescing_remain() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at("card", 5, "monitor", TaskPolicy::default(), start);
        scheduler.set_page_paused_at("monitor", true, start);
        assert!(scheduler.poll_at(start).is_empty());
        scheduler.set_page_paused_at("monitor", false, start);
        assert_eq!(scheduler.poll_at(start), vec!["card"]);
        scheduler.runtimes.get_mut("card").unwrap().running = true;
        assert!(scheduler.poll_at(start).is_empty());
        scheduler.mark_done_after_at("card", 5, false, None, start);
        assert_eq!(scheduler.runtimes["card"].failure_count, 1);
        assert_eq!(
            scheduler.runtimes["card"].next_run,
            start + Duration::from_secs(10)
        );
    }

    #[test]
    fn page_pause_during_running_collection_preserves_generation_and_pending_event() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at("card", 30, "monitor", TaskPolicy::default(), start);
        assert_eq!(scheduler.poll_at(start), vec!["card"]);
        scheduler.mark_started_at("card", start);
        let generation = scheduler.generation("card").unwrap();

        scheduler.set_page_paused_at("monitor", true, start + Duration::from_secs(1));
        assert!(scheduler.is_running("card"));
        assert_eq!(scheduler.generation("card"), Some(generation));
        assert!(scheduler.request_event_at(
            "card",
            SourceRevision(1),
            RefreshReason::Source(SourceKey::new("source")),
            start + Duration::from_secs(2),
        ));

        scheduler.set_page_paused_at("monitor", false, start + Duration::from_secs(3));
        assert!(scheduler.is_running("card"));
        assert_eq!(scheduler.generation("card"), Some(generation));

        scheduler.mark_done_after_revision_with_deadline(
            "card",
            30,
            true,
            None,
            None,
            SourceRevision::INITIAL,
        );
        assert!(!scheduler.is_running("card"));
        assert_eq!(
            scheduler.poll_at(start + Duration::from_secs(4)),
            vec!["card"]
        );
    }

    #[test]
    fn healthy_completion_resets_failure_backoff_after_a_failed_round() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at("card", 5, "monitor", TaskPolicy::default(), start);
        assert_eq!(scheduler.poll_at(start), vec!["card"]);
        scheduler.mark_started_at("card", start);
        scheduler.mark_done_after_at("card", 5, false, None, start);
        assert_eq!(scheduler.runtimes["card"].failure_count, 1);
        assert_eq!(
            scheduler.runtimes["card"].next_run,
            start + Duration::from_secs(10)
        );

        assert_eq!(
            scheduler.poll_at(start + Duration::from_secs(10)),
            vec!["card"]
        );
        scheduler.mark_started_at("card", start + Duration::from_secs(10));
        scheduler.mark_done_after_at("card", 5, true, None, start + Duration::from_secs(10));
        assert_eq!(scheduler.runtimes["card"].failure_count, 0);
        assert_eq!(
            scheduler.runtimes["card"].next_run,
            start + Duration::from_secs(15)
        );
    }

    #[test]
    fn rebinding_a_running_scheduled_task_defers_replacement_until_completion() {
        let start = now();
        let mut scheduler = Scheduler::new();
        scheduler.register_with_policy_at(
            "scheduled",
            60,
            "monitor",
            TaskPolicy {
                scheduled: true,
                ..TaskPolicy::default()
            },
            start,
        );
        assert_eq!(scheduler.poll_at(start), vec!["scheduled"]);
        scheduler.mark_started_at("scheduled", start);
        let generation = scheduler.generation("scheduled").unwrap();
        let deadline = start + Duration::from_secs(60);

        assert!(scheduler.rebind_with_policy(
            "scheduled",
            90,
            "monitor",
            TaskPolicy {
                scheduled: true,
                ..TaskPolicy::default()
            },
        ));
        assert!(scheduler.is_running("scheduled"));
        assert_eq!(scheduler.generation("scheduled"), Some(generation));
        assert!(scheduler.poll_at(start + Duration::from_secs(1)).is_empty());

        scheduler.mark_done_after_at_with_deadline(
            "scheduled",
            90,
            true,
            None,
            Some(deadline),
            start + Duration::from_secs(10),
        );
        assert!(!scheduler.is_running("scheduled"));
        assert_eq!(scheduler.next_run("scheduled"), Some(deadline));
        assert_eq!(scheduler.poll_at(deadline), vec!["scheduled"]);
    }
}
