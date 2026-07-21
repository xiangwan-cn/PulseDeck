use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::time::Instant;

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
}

pub struct Scheduler {
    heap: BinaryHeap<Reverse<ScheduledTask>>,
    runtimes: HashMap<String, TaskRuntime>,
    paused_when_inactive: bool,
    window_active: bool,
    generation_seed: u64,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            runtimes: HashMap::new(),
            paused_when_inactive: true,
            window_active: true,
            generation_seed: 0,
        }
    }

    pub fn register(&mut self, card_id: &str, _interval_secs: u64, page_id: &str) {
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
            },
        );
        self.heap.push(Reverse(ScheduledTask {
            next_run,
            card_id: card_id.to_string(),
            generation,
        }));
    }

    #[allow(dead_code)]
    pub fn unregister(&mut self, card_id: &str) {
        self.runtimes.remove(card_id);
    }

    #[allow(dead_code)]
    pub fn set_window_active(&mut self, active: bool) {
        self.window_active = active;
    }

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub fn request_now(&mut self, card_id: &str) {
        if let Some(rt) = self.runtimes.get_mut(card_id) {
            if !rt.running {
                rt.next_run = Instant::now();
                rt.generation += 1;
                self.heap.push(Reverse(ScheduledTask {
                    next_run: rt.next_run,
                    card_id: card_id.to_string(),
                    generation: rt.generation,
                }));
            }
        }
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

    #[allow(dead_code)]
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
            });
            if valid {
                return Some(task.next_run);
            }
            self.heap.pop();
        }
    }

    #[allow(dead_code)]
    pub fn poll(&mut self) -> Vec<String> {
        // Keep due entries in the heap while the window is inactive. Popping them
        // here would silently lose the task and it would never run after resume.
        if !self.window_active && self.paused_when_inactive {
            return Vec::new();
        }
        let now = Instant::now();
        let mut ready = Vec::new();

        while let Some(Reverse(task)) = self.heap.peek() {
            if task.next_run > now {
                break;
            }
            let task = self.heap.pop().unwrap().0;

            if let Some(rt) = self.runtimes.get_mut(&task.card_id) {
                if rt.generation != task.generation {
                    continue;
                }
                if !rt.enabled || rt.running || (rt.paused && !rt.run_once) {
                    continue;
                }
                rt.run_once = false;
            }
            ready.push(task.card_id);
        }

        ready
    }

    #[allow(dead_code)]
    pub fn mark_started(&mut self, card_id: &str) {
        if let Some(rt) = self.runtimes.get_mut(card_id) {
            rt.running = true;
            rt.last_started = Some(Instant::now());
        }
    }

    #[allow(dead_code)]
    pub fn mark_done(&mut self, card_id: &str, interval_secs: u64, success: bool) {
        if let Some(rt) = self.runtimes.get_mut(card_id) {
            rt.running = false;
            if success {
                rt.failure_count = 0;
                rt.last_success = Some(Instant::now());
            } else {
                rt.failure_count += 1;
            }
            let backoff = if success || rt.failure_count == 0 {
                interval_secs
            } else {
                interval_secs
                    .saturating_mul(2u64.pow(rt.failure_count.min(6)))
                    .min(120)
            };
            rt.next_run = Instant::now() + std::time::Duration::from_secs(backoff);
            rt.generation += 1;
            self.heap.push(Reverse(ScheduledTask {
                next_run: rt.next_run,
                card_id: card_id.to_string(),
                generation: rt.generation,
            }));
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
}
