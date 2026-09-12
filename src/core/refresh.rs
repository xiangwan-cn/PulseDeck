//! Generic source/event coordination for the monitor runtime.
//!
//! Sources publish revisions, while scheduler tasks consume those revisions.
//! This module deliberately knows nothing about cards, providers, commands, or
//! plugins; callers bind their opaque task identities to source identities.

use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceKey(String);

impl SourceKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SourceKey {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for SourceKey {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for SourceKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

pub type TaskKey = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct SourceRevision(pub u64);

impl SourceRevision {
    pub const INITIAL: Self = Self(0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceEventKind {
    Changed,
    #[allow(dead_code)]
    Available,
    #[allow(dead_code)]
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEvent {
    pub key: SourceKey,
    pub revision: SourceRevision,
    pub kind: SourceEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshReason {
    #[allow(dead_code)]
    Periodic,
    Manual,
    Source(SourceKey),
    WallClock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshRequest {
    pub task: TaskKey,
    pub reason: RefreshReason,
    pub requested_revision: SourceRevision,
}

#[derive(Debug, Default)]
struct SourceState {
    revision: SourceRevision,
    dependents: HashSet<TaskKey>,
    last_requested: HashMap<TaskKey, SourceRevision>,
}

/// Maps source events to scheduler tasks and keeps source revisions monotonic.
/// A task receives at most one request for a given revision, even when a
/// source has several aliases or an event is routed more than once.
#[derive(Debug, Default)]
pub struct RefreshCoordinator {
    sources: HashMap<SourceKey, SourceState>,
}

impl RefreshCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_source(&mut self, key: impl Into<SourceKey>) -> SourceRevision {
        let key = key.into();
        self.sources.entry(key).or_default().revision
    }

    pub fn bind(&mut self, key: impl Into<SourceKey>, task: impl Into<TaskKey>) {
        let key = key.into();
        let task = task.into();
        self.sources.entry(key).or_default().dependents.insert(task);
    }

    pub fn unbind_task(&mut self, task: &str) {
        for source in self.sources.values_mut() {
            source.dependents.remove(task);
            source.last_requested.remove(task);
        }
        self.sources
            .retain(|_, source| !source.dependents.is_empty());
    }

    pub fn revision(&self, key: &SourceKey) -> SourceRevision {
        self.sources
            .get(key)
            .map(|source| source.revision)
            .unwrap_or(SourceRevision::INITIAL)
    }

    /// Publish one source edge and return the deduplicated task requests for
    /// the resulting revision. Repeated edges while a task is already pending
    /// are still represented by a higher revision, allowing the scheduler to
    /// retain one dirty follow-up rather than losing the latest state.
    pub fn publish(
        &mut self,
        key: impl Into<SourceKey>,
        kind: SourceEventKind,
    ) -> (SourceEvent, Vec<RefreshRequest>) {
        let key = key.into();
        let source = self.sources.entry(key.clone()).or_default();
        source.revision = SourceRevision(source.revision.0.saturating_add(1));
        let event = SourceEvent {
            key: key.clone(),
            revision: source.revision,
            kind,
        };
        let mut requests = Vec::new();
        let mut tasks = source.dependents.iter().cloned().collect::<Vec<_>>();
        tasks.sort();
        for task in tasks {
            let requested = source.last_requested.entry(task.clone()).or_default();
            if *requested >= event.revision {
                continue;
            }
            *requested = event.revision;
            requests.push(RefreshRequest {
                task,
                reason: RefreshReason::Source(key.clone()),
                requested_revision: event.revision,
            });
        }
        (event, requests)
    }

    pub fn clear(&mut self) {
        self.sources.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_events_fan_out_to_bound_tasks() {
        let mut coordinator = RefreshCoordinator::new();
        coordinator.bind("power", "capacity");
        coordinator.bind("power", "temperature");
        let (event, requests) = coordinator.publish("power", SourceEventKind::Changed);
        assert_eq!(event.revision, SourceRevision(1));
        assert_eq!(
            requests
                .iter()
                .map(|request| request.task.as_str())
                .collect::<Vec<_>>(),
            ["capacity", "temperature"]
        );
    }

    #[test]
    fn each_task_receives_one_request_per_revision() {
        let mut coordinator = RefreshCoordinator::new();
        coordinator.bind("source", "task");
        let (_, first) = coordinator.publish("source", SourceEventKind::Changed);
        assert_eq!(first.len(), 1);
        let (_, second) = coordinator.publish("source", SourceEventKind::Changed);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].requested_revision, SourceRevision(2));
        assert_eq!(
            coordinator.revision(&SourceKey::from("source")),
            SourceRevision(2)
        );
    }

    #[test]
    fn unbinding_a_task_does_not_affect_other_dependents() {
        let mut coordinator = RefreshCoordinator::new();
        coordinator.bind("source", "one");
        coordinator.bind("source", "two");
        coordinator.unbind_task("one");
        let (_, requests) = coordinator.publish("source", SourceEventKind::Available);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].task, "two");
    }
}
