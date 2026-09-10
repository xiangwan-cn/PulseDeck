use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use adw::prelude::AdwApplicationWindowExt;
use gtk::prelude::*;
use gtk::{Align, Box as GtkBox, Button, Label, Orientation};

use crate::core::cache;
use crate::core::config::{
    config_modules_dir, config_path, AppConfig, CardConfig, CardWorkBehavior, CardWorkload,
    ConfigManager, DisplayConfig, IdleViewMode, ScreenInhibitMode, SourceConfig, SourceKind,
};
use crate::core::refresh::{
    RefreshCoordinator, RefreshReason, SourceEventKind, SourceKey, SourceRevision,
};
use crate::core::runtime::{
    Activity, IdleViewDecision, RuntimeHandle, RuntimeManager, UserActivity, Visibility,
};
use crate::core::scheduler::{Scheduler, TaskPolicy, WorkBehavior, Workload};
use crate::metrics::builtin::{
    builtin_is_event_driven, builtin_uses_stateful_sampling, create_builtin_metric,
};
use crate::metrics::command::CommandMetric;
use crate::metrics::file::FileMetric;
use crate::metrics::http::HttpMetric;
use crate::metrics::traits::{BuiltinMetric, MetricContext};
use crate::model::action_result::ActionResult;
use crate::model::card_model::{CardModel, CardState, CardValue};
use crate::model::metric_result::{MetricResult, MetricState};
use crate::tokio_handle;
use crate::ui::page::Page;

const DEFAULT_CONFIG: &str = include_str!("../config/config.example.toml");
const NETWORK_SIGNAL_FALLBACK_SECONDS: u64 = 600;

const APP_CSS: &str = r#"
.tab-bar-area { padding: 5px 6px; }
.tab-bar-area tab { border-radius: 10px; min-height: 32px; font-size: 13px; }
.compact-grid-button { min-width: 32px; min-height: 32px; padding: 0; }
.pulsedeck-flow > flowboxchild { padding: 0; }
.pulsedeck-card { padding: 10px 8px 8px; border-radius: 14px; border: 1px solid alpha(currentColor, 0.12); background: alpha(currentColor, 0.035); box-shadow: 0 2px 8px alpha(black, 0.08); }
.metric-card { }
.accent-blue   { border-left: 2px solid #3584e4; }
.accent-purple { border-left: 2px solid #9141ac; }
.accent-green  { border-left: 2px solid #33d17a; }
.accent-orange { border-left: 2px solid #e5a50a; }
.accent-teal   { border-left: 2px solid #2190a0; }
.metric-header-icon { opacity: 0.65; }
.metric-header-name { font-weight: 700; font-size: 16px; }
.metric-header-sub { font-size: 12px; opacity: 0.62; margin-top: 0px; }
.metric-value-box { margin: 5px 0 2px 0; }
.metric-value { font-size: 24px; font-weight: 800; font-feature-settings: "tnum"; }
.content-medium .metric-value { font-size: 20px; }
.content-dense .metric-value { font-size: 15px; font-weight: 650; }
.metric-value-placeholder { font-size: 14px; font-weight: 400; opacity: 0.3; }
.metric-value-warning  { color: #e5a50a; }
.metric-value-critical { color: #e01b24; }
.metric-value-good     { color: #33d17a; }
.metric-card.click-action-card { transition: background-color 120ms ease; }
.metric-card.click-action-card:hover { background-color: alpha(@accent_bg_color, 0.12); }
.metric-footer { font-size: 12px; opacity: 0.7; margin-top: 1px; }
.content-medium .metric-footer, .content-medium .metric-header-sub { font-size: 11px; }
.content-dense .metric-footer, .content-dense .metric-header-sub { font-size: 10px; }
.compact-card { padding: 6px 4px; border-radius: 10px; }
.compact-card .metric-header-name { font-size: 15px; }
.compact-card .metric-header-icon { opacity: 0; min-width: 0; min-height: 0; }
.compact-card .metric-header-sub, .compact-card .metric-footer,
.compact-card.content-medium .metric-header-sub, .compact-card.content-medium .metric-footer,
.compact-card.content-dense .metric-header-sub, .compact-card.content-dense .metric-footer { font-size: 12px; }
.compact-card .metric-value-box { margin: 2px 0 0 0; }
.compact-card .metric-value { font-size: 23px; }
.compact-card.content-medium .metric-value { font-size: 21px; }
.compact-card.content-dense .metric-value { font-size: 19px; }
.compact-card .action-icon { opacity: 0; min-width: 0; min-height: 0; }
.compact-card .action-desc { font-size: 12px; }
.compact-card .action-confirm-badge { font-size: 11px; }
.compact-card .action-name { font-size: 15px; }
.compact-card .action-run-btn { padding: 2px 4px; min-width: 0; }
.action-card { }
.action-icon { opacity: 0.55; }
.action-name { font-weight: 700; font-size: 13px; }
.action-desc { font-size: 10px; opacity: 0.55; margin-top: 1px; }
.action-confirm-badge { font-size: 9px; color: #e5a50a; }
.action-run-btn { min-width: 36px; min-height: 36px; }
.settings-card, .status-card { }
.settings-icon { opacity: 0.55; }
.settings-name { font-weight: 700; font-size: 14px; }
.settings-desc { font-size: 11px; opacity: 0.55; margin-top: 1px; }
.status-icon { opacity: 0.55; }
.status-text { font-size: 11px; opacity: 0.6; }
.settings-card-row { min-height: 48px; }
.card-fullscreen-layer { background: @window_bg_color; }
.card-fullscreen-close { margin: 4px; }
.runtime-dim-layer { background: #000000; transition: opacity 350ms ease; }
.runtime-idle-status { color: #777777; font-size: 16px; font-weight: 600; }
.runtime-idle-time { color: #666666; font-size: 28px; font-feature-settings: "tnum"; }
"#;

struct MetricUpdate {
    card_id: String,
    page_id: String,
    source_key: String,
    source_revision: SourceRevision,
    /// Source key for the revision domain that triggered this run. It is
    /// separate from the canonical card source because power/network signal
    /// aliases share one scheduler task.
    revision_key: Option<String>,
    /// Cache/source invalidation generations cover periodic runs as well as
    /// event-triggered runs. A collector that overlaps an edge is rejected by
    /// the GTK receiver before it can render stale data.
    invalidation_generation: u64,
    config_epoch: u64,
    task_generation: u64,
    result: MetricResult,
    /// Health of the underlying collection, independent of the result chosen
    /// for display. A stale last-good projection may still be rendered after
    /// failure, but it must not reset scheduler backoff as if the collection
    /// were healthy.
    collection_health: CollectionHealth,
    /// Budget used by this replay/collection. The receiver rejects an update
    /// if hot reload lowered the current budget while it was queued.
    max_output_bytes: usize,
    interval_secs: u64,
    next_delay: Option<Duration>,
    /// Fixed wall-clock schedules carry their absolute next slot through the
    /// worker so completion latency cannot shift the following execution.
    next_deadline: Option<Instant>,
}

struct ActionUpdate {
    action_id: String,
    invocation_id: u64,
    result_card_id: Option<String>,
    result: ActionResult,
}

// Worker completions are state updates, not an append-only event log. Keeping
// only the newest update for each card/action prevents a slow GTK turn from
// retaining every intermediate result when collectors finish in a burst.
const MAX_UI_UPDATES_PER_TURN: usize = 24;
const MAX_PENDING_UI_UPDATES: usize = 1024;

struct LatestInbox<T> {
    pending: Mutex<HashMap<String, T>>,
    wake_tx: async_channel::Sender<()>,
}

impl<T> LatestInbox<T> {
    fn new(wake_tx: async_channel::Sender<()>) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            wake_tx,
        }
    }

    fn publish(&self, key: String, update: T) {
        self.publish_if(key, update, |_| true);
    }

    fn publish_if<F>(&self, key: String, update: T, should_replace: F)
    where
        F: FnOnce(Option<&T>) -> bool,
    {
        if self.wake_tx.is_closed() {
            return;
        }
        if let Ok(mut pending) = self.pending.lock() {
            if !should_replace(pending.get(&key)) {
                return;
            }
            pending.insert(key, update);
            if pending.len() > MAX_PENDING_UI_UPDATES {
                if let Some(evicted) = pending.keys().next().cloned() {
                    pending.remove(&evicted);
                }
            }
        }
        let _ = self.wake_tx.try_send(());
    }

    fn take_batch(&self, limit: usize) -> Vec<T> {
        let Ok(mut pending) = self.pending.lock() else {
            return Vec::new();
        };
        let keys: Vec<String> = pending.keys().take(limit).cloned().collect();
        keys.into_iter()
            .filter_map(|key| pending.remove(&key))
            .collect()
    }

    fn has_pending(&self) -> bool {
        self.pending
            .lock()
            .map(|pending| !pending.is_empty())
            .unwrap_or(false)
    }

    fn len(&self) -> usize {
        self.pending
            .lock()
            .map(|pending| pending.len())
            .unwrap_or_default()
    }

    fn wake(&self) {
        let _ = self.wake_tx.try_send(());
    }

    fn close(&self) {
        self.wake_tx.close();
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
    }
}

fn publish_metric(inbox: &Arc<LatestInbox<MetricUpdate>>, update: MetricUpdate) {
    let generation = update.task_generation;
    inbox.publish_if(update.card_id.clone(), update, |pending| {
        pending.is_none_or(|pending| pending.task_generation <= generation)
    });
}

fn publish_action(inbox: &Arc<LatestInbox<ActionUpdate>>, update: ActionUpdate) {
    let invocation = update.invocation_id;
    inbox.publish_if(update.action_id.clone(), update, |pending| {
        pending.is_none_or(|pending| pending.invocation_id <= invocation)
    });
}

const MAX_COMPLETED_ACTION_INVOCATIONS: usize = 4096;
static NEXT_SAMPLING_CONSUMER_KEY: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollectionHealth {
    Healthy,
    Failed,
}

impl CollectionHealth {
    fn from_collector(result: &MetricResult) -> Self {
        if result.state == MetricState::Normal && !result.cached {
            Self::Healthy
        } else {
            Self::Failed
        }
    }

    fn cache_hit() -> Self {
        Self::Healthy
    }

    fn is_healthy(self) -> bool {
        matches!(self, Self::Healthy)
    }
}

struct InvocationDedup {
    seen: std::collections::HashSet<u64>,
    order: std::collections::VecDeque<u64>,
}

impl InvocationDedup {
    fn insert(&mut self, invocation_id: u64) -> bool {
        if !self.seen.insert(invocation_id) {
            return false;
        }
        self.order.push_back(invocation_id);
        while self.order.len() > MAX_COMPLETED_ACTION_INVOCATIONS {
            if let Some(oldest) = self.order.pop_front() {
                self.seen.remove(&oldest);
            }
        }
        true
    }
}

struct CardMeta {
    page_id: String,
    interval_secs: u64,
    source_key: String,
    source_node_key: String,
    config_epoch: u64,
    /// Opaque lifetime identity for this card's mutable sampling consumer.
    /// It is deliberately separate from the canonical source descriptor.
    sampling_consumer_key: u64,
}

enum PersistentSource {
    Command(CommandMetric),
    File(FileMetric),
    Http(HttpMetric),
    Static(MetricResult),
}

enum SourceCollector {
    Builtin(BuiltinMetric),
    Persistent(PersistentSource),
}

struct SourceNode {
    state: Mutex<SourceNodeState>,
    /// Canonical descriptor key used to invalidate all sampling-policy nodes
    /// that project the same source.
    descriptor_key: String,
    /// Invalidation is atomic so a GTK event never waits for a blocking
    /// command/HTTP collector holding the state lock.
    invalidation_generation: AtomicU64,
}

struct SourceNodeState {
    collector: SourceCollector,
    /// A short source-level coalescing window lets cards bound to the same
    /// descriptor share one in-flight sample/result without mixing the
    /// cadence/state of stateful samplers.
    last_result: Option<MetricResult>,
    last_collected_at: Option<Instant>,
    last_generation: u64,
    last_max_output: usize,
}

impl SourceNode {
    const COALESCE_WINDOW: Duration = Duration::from_millis(100);

    fn new(collector: SourceCollector, descriptor_key: String) -> Self {
        Self {
            state: Mutex::new(SourceNodeState {
                collector,
                last_result: None,
                last_collected_at: None,
                last_generation: 0,
                last_max_output: 0,
            }),
            descriptor_key,
            invalidation_generation: AtomicU64::new(0),
        }
    }

    fn collect(&self, ctx: &MetricContext, max_output: usize) -> MetricResult {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        let generation = self.invalidation_generation.load(Ordering::Acquire);
        if state.last_generation == generation
            && state.last_max_output == max_output
            && state
                .last_collected_at
                .is_some_and(|at| now.duration_since(at) < Self::COALESCE_WINDOW)
        {
            if let Some(result) = &state.last_result {
                return result.clone();
            }
        }
        let result = match &mut state.collector {
            SourceCollector::Builtin(metric) => metric.collect(ctx),
            SourceCollector::Persistent(source) => source.collect(ctx, max_output),
        };
        state.last_collected_at = Some(Instant::now());
        state.last_max_output = max_output;
        state.last_result = Some(result.clone());
        // Keep the generation observed before collection. If an edge arrived
        // during the blocking sample, the next caller sees a mismatch and
        // cannot replay this potentially stale result from the coalescing
        // window.
        state.last_generation = generation;
        result
    }

    fn invalidate(&self) {
        self.invalidation_generation.fetch_add(1, Ordering::AcqRel);
    }
}

impl PersistentSource {
    fn collect(&mut self, ctx: &MetricContext, max_output: usize) -> MetricResult {
        match self {
            Self::Command(source) => source.collect_no_ctx(max_output, ctx.shutdown.clone()),
            Self::File(source) => source.collect(ctx, max_output),
            Self::Http(source) => source.collect(ctx, max_output),
            Self::Static(result) => result.clone(),
        }
    }
}

struct ConfigReloadGuard {
    last_reload: RefCell<Instant>,
    debounce_ms: u64,
    pending: RefCell<bool>,
    source_id: Rc<RefCell<Option<glib::SourceId>>>,
}

impl Drop for ConfigReloadGuard {
    fn drop(&mut self) {
        if let Some(source) = self.source_id.borrow_mut().take() {
            source.remove();
        }
    }
}

impl ConfigReloadGuard {
    fn new(debounce_ms: u64) -> Rc<Self> {
        Rc::new(Self {
            last_reload: RefCell::new(Instant::now()),
            debounce_ms,
            pending: RefCell::new(false),
            source_id: Rc::new(RefCell::new(None)),
        })
    }
}

pub struct MonitorWindow {
    window: adw::ApplicationWindow,
    app_style_provider: gtk::CssProvider,
    app_style_display: gtk::gdk::Display,
    view_stack: adw::ViewStack,
    pages: Rc<RefCell<HashMap<String, Page>>>,
    config: Rc<RefCell<ConfigManager>>,
    scheduler: Rc<RefCell<Scheduler>>,
    handle: tokio::runtime::Handle,
    metric_ctx: Arc<MetricContext>,
    previous_results: Rc<RefCell<HashMap<String, MetricResult>>>,
    source_nodes: Arc<Mutex<HashMap<String, Arc<SourceNode>>>>,
    max_output_budget: Arc<AtomicUsize>,
    card_metas: Rc<RefCell<HashMap<String, CardMeta>>>,
    refresh: Rc<RefCell<RefreshCoordinator>>,
    config_epoch: Rc<Cell<u64>>,
    current_page_id: Rc<RefCell<String>>,
    metric_inbox: Arc<LatestInbox<MetricUpdate>>,
    action_inbox: Arc<LatestInbox<ActionUpdate>>,
    reload_guard: Rc<ConfigReloadGuard>,
    config_monitors: Vec<gio::FileMonitor>,
    scheduler_wake: async_channel::Sender<()>,
    shutdown: Arc<AtomicBool>,
    heartbeat_source: Option<glib::SourceId>,
    compact_grid: Rc<Cell<bool>>,
    runtime: RuntimeHandle,
    dashboard_content: gtk::Box,
    dim_layer: gtk::Box,
    idle_status: gtk::Label,
    idle_time: gtk::Label,
    _power_monitor: Rc<crate::core::power_supply::PowerSupplyMonitor>,
    file_monitors: Rc<RefCell<Vec<gio::FileMonitor>>>,
    file_fallback: Rc<RefCell<Option<glib::SourceId>>>,
    _network_monitor: gio::NetworkMonitor,
    network_dbus: Option<(gio::DBusConnection, gio::SignalSubscriptionId)>,
    network_fallback: Option<glib::SourceId>,
    network_debounce: Rc<RefCell<Option<glib::SourceId>>>,
}

impl MonitorWindow {
    pub fn new(app: &adw::Application, config: ConfigManager) -> Self {
        let window = adw::ApplicationWindow::new(app);
        window.set_default_size(420, 720);
        window.set_title(Some(&config.config().app.title));
        let runtime_manager = RuntimeManager::new(config.config().runtime.clone());
        let runtime = runtime_manager.handle();

        let app_style_display = gtk::gdk::Display::default().unwrap();
        let app_style_provider = gtk::CssProvider::new();
        app_style_provider.load_from_data(APP_CSS);
        gtk::style_context_add_provider_for_display(
            &app_style_display,
            &app_style_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let view_stack = adw::ViewStack::new();
        view_stack.set_vexpand(true);

        let switcher = adw::ViewSwitcher::new();
        switcher.set_stack(Some(&view_stack));
        switcher.set_policy(adw::ViewSwitcherPolicy::Wide);

        let sw_area = gtk::CenterBox::new();
        sw_area.set_hexpand(true);
        sw_area.set_margin_top(4);
        sw_area.set_center_widget(Some(&switcher));
        let compact_grid = gtk::ToggleButton::new();
        compact_grid.set_icon_name("view-grid-symbolic");
        compact_grid.add_css_class("flat");
        compact_grid.add_css_class("compact-grid-button");
        compact_grid.set_halign(Align::End);
        let initial_compact = load_compact_grid_preference();
        compact_grid.set_active(initial_compact);
        compact_grid.set_tooltip_text(Some(if initial_compact {
            "恢复默认卡片布局"
        } else {
            "切换全部卡片为 6×3 紧凑布局"
        }));
        sw_area.set_end_widget(Some(&compact_grid));
        sw_area.add_css_class("tab-bar-area");

        let content = GtkBox::new(Orientation::Vertical, 0);
        content.append(&sw_area);
        content.append(&gtk::Separator::new(Orientation::Horizontal));
        content.append(&view_stack);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&content));
        let dim_layer = gtk::Box::new(Orientation::Vertical, 0);
        dim_layer.add_css_class("runtime-dim-layer");
        dim_layer.set_hexpand(true);
        dim_layer.set_vexpand(true);
        dim_layer.set_can_target(false);
        dim_layer.set_opacity(0.0);
        dim_layer.set_visible(false);
        dim_layer.set_valign(Align::Fill);
        let idle_spacer_top = gtk::Box::new(Orientation::Vertical, 0);
        idle_spacer_top.set_vexpand(true);
        dim_layer.append(&idle_spacer_top);
        let idle_time = Label::new(None);
        idle_time.add_css_class("runtime-idle-time");
        idle_time.set_visible(false);
        dim_layer.append(&idle_time);
        let idle_status = Label::new(None);
        idle_status.add_css_class("runtime-idle-status");
        idle_status.set_wrap(true);
        idle_status.set_justify(gtk::Justification::Center);
        idle_status.set_visible(false);
        dim_layer.append(&idle_status);
        let idle_spacer_bottom = gtk::Box::new(Orientation::Vertical, 0);
        idle_spacer_bottom.set_vexpand(true);
        dim_layer.append(&idle_spacer_bottom);
        overlay.add_overlay(&dim_layer);
        window.set_content(Some(&overlay));

        let pages: Rc<RefCell<HashMap<String, Page>>> = Rc::new(RefCell::new(HashMap::new()));
        let compact_preference = Rc::new(Cell::new(initial_compact));
        let compact_pages = pages.clone();
        let saved_compact_preference = compact_preference.clone();
        compact_grid.connect_toggled(move |button| {
            let compact = button.is_active();
            saved_compact_preference.set(compact);
            if let Err(error) = save_compact_grid_preference(compact) {
                tracing::warn!(%error, "failed to save compact-grid preference");
            }
            button.set_tooltip_text(Some(if compact {
                "恢复默认卡片布局"
            } else {
                "切换全部卡片为 6×3 紧凑布局"
            }));
            for page in compact_pages.borrow_mut().values_mut() {
                page.set_compact_grid(compact);
            }
        });
        let config_ref = Rc::new(RefCell::new(config));
        let scheduler = Rc::new(RefCell::new(Scheduler::new()));

        let handle = tokio_handle();
        std::mem::drop(handle.spawn_blocking(cache::cleanup));
        let shutdown = Arc::new(AtomicBool::new(false));
        let heartbeat_last = Rc::new(Cell::new(Instant::now()));
        let heartbeat_source = {
            let heartbeat_last = heartbeat_last.clone();
            glib::timeout_add_local(Duration::from_millis(250), move || {
                let now = Instant::now();
                let gap = now.saturating_duration_since(heartbeat_last.replace(now));
                if gap >= Duration::from_secs(1) {
                    tracing::warn!(
                        heartbeat_gap_ms = gap.as_millis() as u64,
                        "GTK main-loop heartbeat delayed"
                    );
                }
                glib::ControlFlow::Continue
            })
        };
        let http_client = reqwest::Client::new();

        let battery_root = PathBuf::from("/sys/class/power_supply");
        let procfs_root = PathBuf::from("/proc");
        let metric_ctx = Arc::new(MetricContext::new(
            handle.clone(),
            shutdown.clone(),
            http_client.clone(),
            battery_root,
            procfs_root,
            PathBuf::from("/sys/class/thermal"),
        ));

        let (metric_wake, metric_wake_rx) = async_channel::bounded::<()>(1);
        let (action_wake, action_wake_rx) = async_channel::bounded::<()>(1);
        let metric_inbox = Arc::new(LatestInbox::new(metric_wake));
        let action_inbox = Arc::new(LatestInbox::new(action_wake));
        let (scheduler_wake, scheduler_wake_rx) = async_channel::bounded::<()>(1);
        let source_nodes: Arc<Mutex<HashMap<String, Arc<SourceNode>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let max_output_budget = Arc::new(AtomicUsize::new(
            config_ref.borrow().config().app.max_output_bytes.max(1),
        ));
        let refresh = Rc::new(RefCell::new(RefreshCoordinator::new()));
        let config_epoch = Rc::new(Cell::new(0));
        let previous_results: Rc<RefCell<HashMap<String, MetricResult>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let card_metas: Rc<RefCell<HashMap<String, CardMeta>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let current_page_id = Rc::new(RefCell::new(String::new()));
        let power_ctx = metric_ctx.clone();
        let power_refresh = refresh.clone();
        let power_scheduler = scheduler.clone();
        let power_wake = scheduler_wake.clone();
        let power_card_metas = card_metas.clone();
        let power_nodes = source_nodes.clone();
        let thermal_refresh = refresh.clone();
        let thermal_scheduler = scheduler.clone();
        let thermal_wake = scheduler_wake.clone();
        let thermal_card_metas = card_metas.clone();
        let thermal_nodes = source_nodes.clone();
        let power_monitor =
            crate::core::power_supply::PowerSupplyMonitor::start_with_shared_battery(
                runtime.clone(),
                PathBuf::from("/sys/class/power_supply"),
                PathBuf::from("/sys/class/thermal"),
                power_ctx.battery.clone(),
                move || {
                    if let Ok(mut battery) = power_ctx.battery.lock() {
                        battery.invalidate();
                    }
                    publish_source_event(
                        &power_refresh,
                        &power_scheduler,
                        &power_wake,
                        &power_card_metas,
                        &power_nodes,
                        "signal:power-supply",
                        SourceEventKind::Changed,
                    );
                },
                move || {
                    publish_source_event(
                        &thermal_refresh,
                        &thermal_scheduler,
                        &thermal_wake,
                        &thermal_card_metas,
                        &thermal_nodes,
                        "signal:thermal",
                        SourceEventKind::Changed,
                    );
                },
            );

        let reload_guard = ConfigReloadGuard::new(500);
        let network_monitor = gio::NetworkMonitor::default();

        let mut win = Self {
            window,
            app_style_provider,
            app_style_display,
            view_stack,
            pages: pages.clone(),
            config: config_ref.clone(),
            scheduler: scheduler.clone(),
            handle,
            metric_ctx,
            previous_results,
            source_nodes,
            max_output_budget,
            card_metas,
            refresh,
            config_epoch,
            current_page_id: current_page_id.clone(),
            metric_inbox,
            action_inbox,
            reload_guard: reload_guard.clone(),
            config_monitors: Vec::new(),
            scheduler_wake,
            shutdown,
            heartbeat_source: Some(heartbeat_source),
            compact_grid: compact_preference,
            runtime,
            dashboard_content: content,
            dim_layer,
            idle_status,
            idle_time,
            _power_monitor: power_monitor,
            file_monitors: Rc::new(RefCell::new(Vec::new())),
            file_fallback: Rc::new(RefCell::new(None)),
            _network_monitor: network_monitor.clone(),
            network_dbus: None,
            network_fallback: None,
            network_debounce: Rc::new(RefCell::new(None)),
        };

        win.setup_pages();
        win.setup_adaptive_layout();
        win.setup_runtime_consumers(app);
        win.setup_lifecycle();
        win.setup_config_monitor();
        win.setup_network_monitor(&network_monitor);
        win.start_scheduler_polling(scheduler_wake_rx);
        win.start_metric_receiver(metric_wake_rx);
        win.start_action_receiver(action_wake_rx);

        win
    }

    fn setup_runtime_consumers(&mut self, app: &adw::Application) {
        let application = app.clone();
        let inhibit_cookie: Rc<RefCell<Option<u32>>> = Rc::new(RefCell::new(None));
        let runtime = self.runtime.clone();
        runtime.set_mapped(self.window.is_mapped());
        runtime.set_active(self.window.is_active());
        self.window.connect_map({
            let runtime = runtime.clone();
            move |_| runtime.set_mapped(true)
        });
        self.window.connect_unmap({
            let runtime = runtime.clone();
            move |_| runtime.set_mapped(false)
        });
        self.window.connect_is_active_notify({
            let runtime = runtime.clone();
            move |window| runtime.set_active(window.is_active())
        });

        let rx = runtime.subscribe();
        let window = self.window.clone();
        let dashboard_content = self.dashboard_content.clone();
        let dim = self.dim_layer.clone();
        let idle_status = self.idle_status.clone();
        let idle_time = self.idle_time.clone();
        let clock_source: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        let presented_attention: Rc<RefCell<Option<(String, String)>>> =
            Rc::new(RefCell::new(None));
        glib::MainContext::default().spawn_local(async move {
            while let Ok(snapshot) = rx.recv().await {
                let runtime_config = runtime.config();
                if let crate::core::runtime::AgentPhase::Attention { task_id, event_id } =
                    &snapshot.agent_phase
                {
                    let key = (task_id.clone(), event_id.clone());
                    if runtime_config.bring_to_foreground_on_attention
                        && presented_attention.borrow().as_ref() != Some(&key)
                    {
                        presented_attention.replace(Some(key));
                        window.present();
                    }
                } else {
                    presented_attention.borrow_mut().take();
                }
                if snapshot.inhibit_screen {
                    if inhibit_cookie.borrow().is_none() {
                        let flags = gtk::ApplicationInhibitFlags::IDLE;
                        let id = application.inhibit(
                            Some(&window),
                            flags,
                            Some("PulseDeck 正在前台显示实时监控信息"),
                        );
                        inhibit_cookie.replace(Some(id));
                    }
                } else if let Some(id) = inhibit_cookie.borrow_mut().take() {
                    application.uninhibit(id);
                }

                let idle_view = snapshot.idle_view;
                if idle_view != IdleViewDecision::None {
                    let minimal = idle_view == IdleViewDecision::Minimal;
                    let brightness = match idle_view {
                        IdleViewDecision::Dim(percent) => percent as f64,
                        IdleViewDecision::Minimal => 0.0,
                        IdleViewDecision::None => 100.0,
                    };
                    dashboard_content.set_child_visible(!minimal);
                    idle_status.set_visible(minimal);
                    idle_time.set_visible(minimal);
                    if minimal {
                        idle_status.set_text(&format!(
                            "{:?}\n供电 {:?} · 温度 {:?}\n{}",
                            snapshot.agent_phase,
                            snapshot.power_verdict,
                            snapshot.thermal_verdict,
                            snapshot.reasons.join(", ")
                        ));
                        update_idle_clock(&idle_time, &idle_status);
                        if clock_source.borrow().is_none() {
                            let time = idle_time.clone();
                            let status = idle_status.clone();
                            let mode = runtime.clone();
                            let holder = clock_source.clone();
                            let source =
                                glib::timeout_add_local(Duration::from_secs(60), move || {
                                    if mode.snapshot().idle_view != IdleViewDecision::Minimal {
                                        holder.borrow_mut().take();
                                        return glib::ControlFlow::Break;
                                    }
                                    update_idle_clock(&time, &status);
                                    glib::ControlFlow::Continue
                                });
                            clock_source.replace(Some(source));
                        }
                    } else if let Some(source) = clock_source.borrow_mut().take() {
                        source.remove();
                    }
                    dim.set_visible(true);
                    dim.set_opacity(if minimal {
                        1.0
                    } else {
                        (1.0 - brightness / 100.0).clamp(0.0, 0.95)
                    });
                } else {
                    dashboard_content.set_child_visible(true);
                    idle_status.set_visible(false);
                    idle_time.set_visible(false);
                    if let Some(source) = clock_source.borrow_mut().take() {
                        source.remove();
                    }
                    dim.set_opacity(0.0);
                    let dim = dim.clone();
                    glib::timeout_add_local_once(Duration::from_millis(400), move || {
                        if dim.opacity() <= 0.001 {
                            dim.set_visible(false);
                        }
                    });
                }
            }
            if let Some(source) = clock_source.borrow_mut().take() {
                source.remove();
            }
            if let Some(id) = inhibit_cookie.borrow_mut().take() {
                application.uninhibit(id);
            }
        });
    }

    fn drain_cards(&self) -> (Vec<CardConfig>, Vec<crate::core::config::ActionConfig>) {
        let cfg = self.config.borrow();
        let app_config = cfg.config();

        let mut cards = effective_cards(app_config, cfg.uses_default_card_registry());

        cards.sort_by_key(|c| c.order);

        let actions = app_config.actions.clone();

        (cards, actions)
    }

    fn sorted_pages(&self) -> Vec<crate::core::config::PageConfig> {
        let cfg = self.config.borrow();
        let app_config = cfg.config();

        let mut pages_list = effective_pages(app_config, cfg.uses_default_page_registry());

        pages_list.sort_by_key(|p| p.order);
        pages_list
    }

    fn setup_pages(&mut self) {
        let pages_list = self.sorted_pages();
        let (cards, actions) = self.drain_cards();

        let mut page_ids = Vec::new();

        self.pages.borrow_mut().clear();
        self.card_metas.borrow_mut().clear();
        self.previous_results.borrow_mut().clear();
        self.source_nodes.lock().unwrap().clear();
        self.refresh.borrow_mut().clear();
        for card in &cards {
            let source_key = source_key_for(card.source.as_ref());
            self.refresh
                .borrow_mut()
                .register_source(source_key.clone());
            self.refresh.borrow_mut().bind(source_key, card.id.clone());
            if card
                .source
                .as_ref()
                .and_then(SourceConfig::builtin_metric)
                .is_some_and(|metric| {
                    matches!(metric, "battery_capacity" | "battery_temperature" | "power")
                })
            {
                self.refresh
                    .borrow_mut()
                    .bind("signal:power-supply", card.id.clone());
            }
            if card.source.as_ref().and_then(SourceConfig::builtin_metric) == Some("network") {
                self.refresh
                    .borrow_mut()
                    .bind("signal:network", card.id.clone());
            }
            if card.source.as_ref().and_then(SourceConfig::builtin_metric)
                == Some("cpu_temperature")
            {
                self.refresh
                    .borrow_mut()
                    .bind("signal:thermal", card.id.clone());
            }
        }
        self.setup_file_monitors(&cards);

        let plugin_context = crate::plugins::PluginContext {
            handle: self.handle.clone(),
            presentation: None,
            runtime: self.runtime.clone(),
            shutdown: self.shutdown.clone(),
        };
        for page_cfg in &pages_list {
            match crate::plugins::build_page(&plugin_context, page_cfg) {
                Ok(Some(container)) => {
                    self.view_stack
                        .add_titled(&container, Some(&page_cfg.id), &page_cfg.title);
                    page_ids.push(page_cfg.id.clone());
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(page = %page_cfg.id, %error, "plugin page skipped");
                    continue;
                }
            }
            let ui = self.config.borrow().config().ui.clone();
            let mut page = Page::new(&page_cfg.id, &ui);
            page.set_available_height(self.view_stack.height());
            page.set_compact_grid(self.compact_grid.get());
            self.populate_page(&mut page, &page_cfg.id, &cards, &actions);

            self.view_stack
                .add_titled(&page.container, Some(&page_cfg.id), &page_cfg.title);

            self.pages.borrow_mut().insert(page_cfg.id.clone(), page);
            page_ids.push(page_cfg.id.clone());
        }

        let preferred = self.config.borrow().config().ui.default_page.clone();
        if let Some(initial) = page_ids
            .iter()
            .find(|id| **id == preferred)
            .or_else(|| page_ids.first())
        {
            self.view_stack.set_visible_child_name(initial);
            *self.current_page_id.borrow_mut() = initial.clone();
        }

        self.scheduler
            .borrow_mut()
            .set_active_page(&self.current_page_id.borrow());
        prune_source_nodes(&self.source_nodes, &self.card_metas);
    }

    fn setup_adaptive_layout(&self) {
        let pages = self.pages.clone();
        let stack = self.view_stack.clone();
        let update_pending = Rc::new(Cell::new(false));
        let schedule_update: Rc<dyn Fn()> = Rc::new(move || {
            if update_pending.replace(true) {
                return;
            }
            let pages = pages.clone();
            let stack = stack.clone();
            let update_pending = update_pending.clone();
            glib::idle_add_local_once(move || {
                update_pending.set(false);
                let height = stack.height();
                for page in pages.borrow_mut().values_mut() {
                    page.set_available_height(height);
                }
            });
        });
        self.window.connect_map({
            let schedule_update = schedule_update.clone();
            move |_| schedule_update()
        });
        self.window.connect_realize(move |window| {
            let Some(surface) = window.surface() else {
                return;
            };
            let schedule_update = schedule_update.clone();
            surface.connect_height_notify(move |_| schedule_update());
        });
    }

    fn setup_file_monitors(&self, cards: &[CardConfig]) {
        install_file_monitors(
            &self.file_monitors,
            &self.file_fallback,
            cards,
            self.refresh.clone(),
            self.scheduler.clone(),
            self.scheduler_wake.clone(),
            self.card_metas.clone(),
            self.source_nodes.clone(),
        );
    }

    fn setup_network_monitor(&mut self, monitor: &gio::NetworkMonitor) {
        let pending = self.network_debounce.clone();
        let queue_network_event: Rc<dyn Fn()> = {
            let pending = pending.clone();
            let refresh = self.refresh.clone();
            let scheduler = self.scheduler.clone();
            let wake = self.scheduler_wake.clone();
            let card_metas = self.card_metas.clone();
            let source_nodes = self.source_nodes.clone();
            let network = self.metric_ctx.clone();
            Rc::new(move || {
                if pending.borrow().is_some() {
                    return;
                }
                let pending_for_timer = pending.clone();
                let refresh = refresh.clone();
                let scheduler = scheduler.clone();
                let wake = wake.clone();
                let card_metas = card_metas.clone();
                let source_nodes = source_nodes.clone();
                let network = network.clone();
                let source = glib::timeout_add_local_once(Duration::from_millis(100), move || {
                    pending_for_timer.borrow_mut().take();
                    if let Ok(mut source) = network.network.lock() {
                        source.invalidate();
                    }
                    publish_source_event(
                        &refresh,
                        &scheduler,
                        &wake,
                        &card_metas,
                        &source_nodes,
                        "signal:network",
                        SourceEventKind::Changed,
                    );
                });
                pending.replace(Some(source));
            })
        };
        monitor.connect_network_changed({
            let queue_network_event = queue_network_event.clone();
            move |_, _| queue_network_event()
        });

        // A signal loss must not turn an event-driven network source into a
        // permanently stale value. The watchdog is a source hint, not a
        // second per-card NetworkManager query, and remains harmless when no
        // card is bound to the signal key.
        let watchdog = {
            let queue_network_event = queue_network_event.clone();
            glib::timeout_add_local(
                Duration::from_secs(NETWORK_SIGNAL_FALLBACK_SECONDS),
                move || {
                    queue_network_event();
                    glib::ControlFlow::Continue
                },
            )
        };
        self.network_fallback = Some(watchdog);

        // NetworkMonitor is a useful generic hint, but NetworkManager's own
        // PropertiesChanged/StateChanged signals are the authoritative edge.
        let Ok(connection) = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE) else {
            tracing::debug!(
                "NetworkManager system bus unavailable; bounded source fallback remains active"
            );
            return;
        };
        let subscription = connection.signal_subscribe(
            Some("org.freedesktop.NetworkManager"),
            None,
            None,
            None,
            None,
            gio::DBusSignalFlags::NONE,
            {
                let queue_network_event = queue_network_event.clone();
                move |_, _, _, _, _, _| queue_network_event()
            },
        );
        self.network_dbus = Some((connection, subscription));
    }

    fn populate_page(
        &self,
        page: &mut Page,
        page_id: &str,
        all_cards: &[CardConfig],
        all_actions: &[crate::core::config::ActionConfig],
    ) {
        let mut page_cards: Vec<&CardConfig> =
            all_cards.iter().filter(|c| c.page == page_id).collect();
        page_cards.sort_by_key(|c| c.order);

        let mut page_actions: Vec<&crate::core::config::ActionConfig> = all_actions
            .iter()
            .filter(|action| action.page == page_id && action.visible)
            .collect();
        page_actions.sort_by_key(|_| 0);

        for card_cfg in &page_cards {
            if card_cfg.kind.is_some() {
                let (presentation, presentation_rx) =
                    crate::plugins::CardPresentationHandle::channel();
                let context = crate::plugins::PluginContext {
                    handle: self.handle.clone(),
                    presentation: Some(presentation.clone()),
                    runtime: self.runtime.clone(),
                    shutdown: self.shutdown.clone(),
                };
                match crate::plugins::build_card(&context, card_cfg) {
                    Ok(Some(widget)) => {
                        page.add_plugin_card(
                            &card_cfg.id,
                            &widget,
                            card_cfg.display.as_ref(),
                            presentation,
                        );
                        let pages = Rc::downgrade(&self.pages);
                        let page_id = page_id.to_string();
                        let card_id = card_cfg.id.clone();
                        glib::MainContext::default().spawn_local(async move {
                            while let Ok(request) = presentation_rx.recv().await {
                                let Some(pages) = pages.upgrade() else {
                                    break;
                                };
                                {
                                    let mut pages = pages.borrow_mut();
                                    if let Some(page) = pages.get_mut(&page_id) {
                                        page.set_plugin_card_presentation(&card_id, request);
                                    }
                                }
                            }
                        });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(card = %card_cfg.id, %error, "plugin card skipped");
                    }
                }
                continue;
            }
            let model = CardModel {
                id: card_cfg.id.clone(),
                title: card_cfg.title.clone(),
                subtitle: card_cfg.description.clone(),
                icon: card_cfg.icon.clone(),
                renderer: card_cfg.renderer,
                state: CardState::Loading,
                value: CardValue::Text("加载中...".into()),
                tooltip: None,
                cached: false,
                columns_after: card_cfg.display.as_ref().and_then(|d| d.columns_after),
                columns: card_cfg.display.as_ref().and_then(|d| d.columns),
            };
            page.add_metric_card(&model, card_cfg.display.as_ref());
            if let Some(metric_card) = page.get_metric_card(&card_cfg.id) {
                let scheduler = self.scheduler.clone();
                let wake = self.scheduler_wake.clone();
                let card_id = card_cfg.id.clone();
                let refresh_runtime = self.runtime.clone();
                metric_card.refresh_btn.connect_clicked(move |button| {
                    refresh_runtime.report_user_activity(UserActivity::ManualRefresh);
                    if scheduler.borrow_mut().request_now(&card_id) {
                        button.set_sensitive(false);
                        button.set_tooltip_text(Some("正在刷新"));
                        let _ = wake.try_send(());
                    }
                });

                bind_metric_action(
                    metric_card,
                    &card_cfg.id,
                    self.config.clone(),
                    self.action_inbox.clone(),
                    self.handle.clone(),
                    self.runtime.clone(),
                    self.shutdown.clone(),
                );
            }

            let source_key = source_key_for(card_cfg.source.as_ref());
            let sampling_consumer_key = next_sampling_consumer_key();
            let source_node_key = source_node_key(&source_key, card_cfg, sampling_consumer_key);
            self.card_metas.borrow_mut().insert(
                card_cfg.id.clone(),
                CardMeta {
                    page_id: page_id.to_string(),
                    interval_secs: card_cfg.refresh_interval,
                    source_key,
                    source_node_key,
                    config_epoch: self.config_epoch.get(),
                    sampling_consumer_key,
                },
            );

            self.scheduler.borrow_mut().register_with_policy(
                &card_cfg.id,
                card_cfg.refresh_interval,
                page_id,
                task_policy(card_cfg),
            );
        }

        for action_cfg in &page_actions {
            let icon = action_cfg.icon.as_deref().unwrap_or("system-run-symbolic");
            let action_id = action_cfg.id.clone();
            let action_inbox = self.action_inbox.clone();
            let handle = self.handle.clone();
            let shutdown = self.shutdown.clone();
            let config = self.config.clone();
            let resolve_config = config.clone();
            let dialog_runtime = self.runtime.clone();
            let response_runtime = self.runtime.clone();
            let dialog_lease = Rc::new(RefCell::new(None));
            let open_lease = dialog_lease.clone();
            let close_lease = dialog_lease.clone();

            page.add_action_card_with_resolver(
                &action_id,
                &action_cfg.name,
                action_cfg.description.as_deref().unwrap_or(""),
                icon,
                move |id| {
                    current_action_config(&resolve_config, id).map(|action| {
                        let (title, detail) = action_confirmation_text(&action);
                        (action.confirm, title, detail)
                    })
                },
                move |id| {
                    let Some(cfg) = current_action_config(&config, id) else {
                        tracing::warn!(action = %id, "action was removed by config reload");
                        return;
                    };
                    execute_action_async(
                        cfg,
                        action_inbox.clone(),
                        handle.clone(),
                        config.clone(),
                        shutdown.clone(),
                        None,
                    );
                },
                move || {
                    open_lease.replace(Some(
                        dialog_runtime.begin_interaction(Duration::from_secs(300)),
                    ));
                },
                move || {
                    close_lease.borrow_mut().take();
                    response_runtime.report_user_activity(UserActivity::Dialog);
                },
            );
        }

        if page_id == "settings" {
            self.add_settings_content(page);
        }
    }

    fn add_settings_content(&self, page: &mut Page) {
        let status_card = GtkBox::new(Orientation::Horizontal, 10);
        status_card.set_hexpand(true);
        status_card.set_overflow(gtk::Overflow::Hidden);
        status_card.add_css_class("card");
        status_card.add_css_class("pulsedeck-card");
        status_card.add_css_class("status-card");

        let status_icon = gtk::Image::from_icon_name("emblem-ok-symbolic");
        status_icon.set_pixel_size(22);
        status_icon.set_valign(Align::Start);
        status_icon.add_css_class("status-icon");
        status_card.append(&status_icon);

        let status_label = Label::new(Some("卡片开关会立即生效，并自动保存到所属配置文件"));
        status_label.set_wrap(true);
        status_label.set_xalign(0.0);
        status_label.set_hexpand(true);
        status_label.set_size_request(1, -1);
        status_label.add_css_class("status-text");
        status_card.append(&status_label);

        page.flow_insert(&status_card);

        let runtime_cfg = self.config.borrow().config().runtime.clone();
        let (inhibit_row, inhibit_dropdown) = setting_dropdown_row(
            "屏幕常亮策略",
            "常亮与刷新、空闲视觉独立；推荐仅窗口活动时抑制熄屏",
            &["从不", "窗口活动时", "窗口已映射时"],
            match runtime_cfg.screen_inhibit {
                ScreenInhibitMode::Never => 0,
                ScreenInhibitMode::WhileActive => 1,
                ScreenInhibitMode::WhileMapped => 2,
            },
        );
        {
            let config = self.config.clone();
            let runtime = self.runtime.clone();
            inhibit_dropdown.connect_selected_notify(move |dropdown| {
                let mut config = config.borrow_mut();
                config.config_mut().runtime.screen_inhibit = match dropdown.selected() {
                    0 => ScreenInhibitMode::Never,
                    2 => ScreenInhibitMode::WhileMapped,
                    _ => ScreenInhibitMode::WhileActive,
                };
                let next = config.config().runtime.clone();
                let _ = config.save();
                runtime.update_config(next);
            });
        }
        page.flow_insert(&inhibit_row);

        let runtime_section = Label::new(Some("运行与省电"));
        runtime_section.set_halign(Align::Start);
        runtime_section.add_css_class("settings-name");
        runtime_section.set_margin_top(8);
        page.flow_insert(&runtime_section);

        for (title, description, active, field) in [
            (
                "启用夜间静默",
                "按本地时钟暂停普通周期工作；电源、网络、Agent 信号和手动/事件请求仍可用",
                runtime_cfg.quiet_hours_enabled,
                "quiet_hours_enabled",
            ),
            (
                "Agent 完成提示音",
                "重要任务边沿只提示一次，后台仍保留",
                runtime_cfg.agent_completion_sound,
                "agent_completion_sound",
            ),
            (
                "重要事件带回前台",
                "Agent 完成或需要处理时主动显示窗口；默认关闭",
                runtime_cfg.bring_to_foreground_on_attention,
                "bring_to_foreground_on_attention",
            ),
        ] {
            let row = setting_switch_row(title, description, active);
            let switch = row
                .last_child()
                .and_then(|widget| widget.downcast::<gtk::Switch>().ok())
                .expect("setting switch row");
            let config = self.config.clone();
            let runtime = self.runtime.clone();
            switch.connect_active_notify(move |switch| {
                let mut config = config.borrow_mut();
                let value = switch.is_active();
                match field {
                    "quiet_hours_enabled" => {
                        config.config_mut().runtime.quiet_hours_enabled = value
                    }
                    "agent_completion_sound" => {
                        config.config_mut().runtime.agent_completion_sound = value
                    }
                    "bring_to_foreground_on_attention" => {
                        config.config_mut().runtime.bring_to_foreground_on_attention = value
                    }
                    _ => {}
                }
                let next = config.config().runtime.clone();
                if let Err(error) = config.save() {
                    tracing::warn!(%error, "failed to save runtime setting");
                }
                runtime.update_config(next);
            });
            page.flow_insert(&row);
        }

        for (title, description, value, min, max, field) in [
            (
                "非活动宽限时间",
                "仅记录窗口焦点变化；映射白天的默认监控不会因失焦而降速",
                runtime_cfg.inactive_grace_seconds,
                0,
                300,
                "inactive_grace_seconds",
            ),
            (
                "空闲等待时间",
                "最后一次真实操作后等待的秒数",
                runtime_cfg.idle_timeout_seconds,
                10,
                3600,
                "idle_timeout_seconds",
            ),
            (
                "稳定等待时间",
                "达到空闲条件后防抖的秒数",
                runtime_cfg.idle_stability_seconds,
                0,
                120,
                "idle_stability_seconds",
            ),
            (
                "空闲视觉亮度",
                "仅改变显式空闲视图，不影响映射窗口的监控新鲜度",
                runtime_cfg.idle_visual_brightness_percent as u64,
                5,
                100,
                "idle_visual_brightness_percent",
            ),
            (
                "夜间静默开始小时",
                "本地时间 0–23；默认 0 表示午夜",
                runtime_cfg.quiet_hours_start_hour as u64,
                0,
                23,
                "quiet_hours_start_hour",
            ),
            (
                "夜间静默结束小时",
                "本地时间 0–23；默认 8 表示上午八点",
                runtime_cfg.quiet_hours_end_hour as u64,
                0,
                23,
                "quiet_hours_end_hour",
            ),
            (
                "观察租约时长",
                "真实点击、滚动、切页或手动刷新后暂时允许观察工作；最长 300 秒，不会被后台事件续期",
                runtime_cfg.observation_lease_seconds,
                1,
                300,
                "observation_lease_seconds",
            ),
            (
                "Agent 提醒保留时间",
                "去重的重要事件在运行状态中保留的秒数",
                runtime_cfg.agent_attention_seconds,
                1,
                300,
                "agent_attention_seconds",
            ),
        ] {
            let (row, spin) = setting_spin_row(title, description, value, min, max);
            let config = self.config.clone();
            let runtime = self.runtime.clone();
            spin.connect_value_changed(move |spin| {
                let value = spin.value().round().max(0.0) as u64;
                let mut config = config.borrow_mut();
                match field {
                    "inactive_grace_seconds" => {
                        config.config_mut().runtime.inactive_grace_seconds = value
                    }
                    "idle_timeout_seconds" => {
                        config.config_mut().runtime.idle_timeout_seconds = value
                    }
                    "idle_stability_seconds" => {
                        config.config_mut().runtime.idle_stability_seconds = value
                    }
                    "idle_visual_brightness_percent" => {
                        config.config_mut().runtime.idle_visual_brightness_percent =
                            value.min(100) as u8
                    }
                    "quiet_hours_start_hour" => {
                        config.config_mut().runtime.quiet_hours_start_hour = value.min(23) as u8
                    }
                    "quiet_hours_end_hour" => {
                        config.config_mut().runtime.quiet_hours_end_hour = value.min(23) as u8
                    }
                    "agent_attention_seconds" => {
                        config.config_mut().runtime.agent_attention_seconds = value
                    }
                    "observation_lease_seconds" => {
                        config.config_mut().runtime.observation_lease_seconds = value.min(300)
                    }
                    _ => {}
                }
                let next = config.config().runtime.clone();
                if let Err(error) = config.save() {
                    tracing::warn!(%error, "failed to save runtime duration");
                }
                runtime.update_config(next);
            });
            page.flow_insert(&row);
        }

        let (display_row, display_dropdown) = setting_dropdown_row(
            "空闲显示方式",
            "默认不改变视觉；可选遮罩或 OLED 纯黑极简视图",
            &["不改变", "深色遮罩", "纯黑极简"],
            match runtime_cfg.idle_view {
                IdleViewMode::None => 0,
                IdleViewMode::Dim => 1,
                IdleViewMode::Minimal => 2,
            },
        );
        {
            let config = self.config.clone();
            let runtime = self.runtime.clone();
            display_dropdown.connect_selected_notify(move |dropdown| {
                let mut config = config.borrow_mut();
                config.config_mut().runtime.idle_view = match dropdown.selected() {
                    1 => IdleViewMode::Dim,
                    2 => IdleViewMode::Minimal,
                    _ => IdleViewMode::None,
                };
                let next = config.config().runtime.clone();
                let _ = config.save();
                runtime.update_config(next);
            });
        }
        page.flow_insert(&display_row);

        let runtime_status = setting_status_row("当前运行状态", "正在初始化…");
        let status_value = runtime_status
            .last_child()
            .and_then(|widget| widget.downcast::<gtk::Label>().ok())
            .expect("runtime status label");
        let runtime_rx = self.runtime.subscribe();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(snapshot) = runtime_rx.recv().await {
                status_value.set_text(&format!(
                    "{:?} · {:?} · {:?} · 周期刷新 {}\n供电 {:?} · 电池阶段 {:?} · 温度 {:?}\n租约 {}\n{}\nAgent {:?} · 提醒 {}s",
                    snapshot.visibility,
                    snapshot.activity,
                    snapshot.work_level,
                    if snapshot.periodic_refresh_paused {
                        "按静默许可暂停"
                    } else {
                        "运行中"
                    },
                    snapshot.power_verdict,
                    snapshot.battery_stage,
                    snapshot.thermal_verdict,
                    if snapshot.observation_lease_active { "有效" } else { "无" },
                    snapshot.reasons.join(", "),
                    snapshot.agent_phase,
                    snapshot.attention_remaining_seconds
                ));
            }
        });
        page.flow_insert(&runtime_status);

        #[cfg(feature = "power-debug")]
        {
            let debug = setting_status_row("功耗调试计数", "点击更新，不启用周期刷新");
            let value = debug
                .last_child()
                .and_then(|widget| widget.downcast::<gtk::Label>().ok())
                .expect("power debug label");
            let button = Button::with_label("读取功耗计数");
            button.connect_clicked(move |_| {
                let counters = crate::core::power_debug::snapshot();
                let cache = crate::core::cache::stats();
                value.set_text(&format!(
                    "调度唤醒 {} · 卡片采集 {} · 外部进程 {}\nHTTP {} · 图片解码 {} · 动画帧 {}\nGTK 更新 {} · 磁盘读 {} · 磁盘写 {}\n缓存 {} 项 / {} KiB · 无效化 {} 项",
                    counters[0],
                    counters[1],
                    counters[2],
                    counters[3],
                    counters[4],
                    counters[5],
                    counters[6],
                    counters[7],
                    counters[8],
                    cache.memory_entries,
                    cache.memory_bytes / 1024,
                    cache.invalidation_entries,
                ));
            });
            debug.append(&button);
            page.flow_insert(&debug);
        }

        let section = Label::new(Some("系统指标卡片"));
        section.set_halign(Align::Start);
        section.add_css_class("settings-name");
        section.set_margin_top(8);
        page.flow_insert(&section);

        let builtin_cards: Vec<CardConfig> = self
            .config
            .borrow()
            .config()
            .cards
            .iter()
            .filter(|card| {
                card.source
                    .as_ref()
                    .map(|source| source.kind() == SourceKind::Builtin)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        for card in builtin_cards {
            let row = GtkBox::new(Orientation::Horizontal, 12);
            row.set_hexpand(true);
            row.set_overflow(gtk::Overflow::Hidden);
            row.add_css_class("card");
            row.add_css_class("pulsedeck-card");
            row.add_css_class("settings-card-row");

            let icon = gtk::Image::from_icon_name(
                card.icon
                    .as_deref()
                    .unwrap_or("utilities-system-monitor-symbolic"),
            );
            icon.set_pixel_size(22);
            icon.add_css_class("settings-icon");
            row.append(&icon);

            let labels = GtkBox::new(Orientation::Vertical, 1);
            labels.set_hexpand(true);
            labels.set_size_request(1, -1);
            let title = Label::new(Some(&card.title));
            title.set_halign(Align::Start);
            title.add_css_class("settings-name");
            labels.append(&title);
            let metric_name = card
                .source
                .as_ref()
                .and_then(SourceConfig::builtin_metric)
                .unwrap_or("builtin");
            let description = Label::new(Some(metric_name));
            description.set_halign(Align::Start);
            description.add_css_class("settings-desc");
            labels.append(&description);
            row.append(&labels);

            let toggle = gtk::Switch::new();
            toggle.set_valign(Align::Center);
            toggle.set_active(card.enabled);
            let card_id = card.id.clone();
            let config = self.config.clone();
            let action_inbox = self.action_inbox.clone();
            let action_handle = self.handle.clone();
            let shutdown = self.shutdown.clone();
            let pages = self.pages.clone();
            let scheduler = self.scheduler.clone();
            let card_metas = self.card_metas.clone();
            let previous_results = self.previous_results.clone();
            let scheduler_wake = self.scheduler_wake.clone();
            let current_page_id = self.current_page_id.clone();
            let runtime = self.runtime.clone();
            let refresh = self.refresh.clone();
            let file_monitors = self.file_monitors.clone();
            let file_fallback = self.file_fallback.clone();
            let source_nodes = self.source_nodes.clone();
            let config_epoch = self.config_epoch.clone();
            toggle.connect_active_notify(move |switch| {
                let mut config_manager = config.borrow_mut();
                let changed_card = config_manager
                    .config_mut()
                    .cards
                    .iter_mut()
                    .find(|card| card.id == card_id)
                    .map(|card| {
                        card.enabled = switch.is_active();
                        card.clone()
                    });
                if let Some(card) = changed_card {
                    if let Err(error) = config_manager.save() {
                        tracing::warn!("failed to save card setting: {}", error);
                    }
                    let cards_for_watchers = config_manager.config().cards.clone();
                    drop(config_manager);

                    if switch.is_active() {
                        previous_results.borrow_mut().remove(&card.id);
                        if let Some(page) = pages.borrow_mut().get_mut(&card.page) {
                            if !page.metric_cards.contains_key(&card.id) {
                                let model = CardModel {
                                    id: card.id.clone(),
                                    title: card.title.clone(),
                                    subtitle: card.description.clone(),
                                    icon: card.icon.clone(),
                                    renderer: card.renderer,
                                    state: CardState::Loading,
                                    value: CardValue::Text("加载中...".into()),
                                    tooltip: None,
                                    cached: false,
                                    columns_after: None,
                                    columns: None,
                                };
                                page.add_metric_card(&model, card.display.as_ref());
                                if let Some(metric_card) = page.get_metric_card(&card.id) {
                                    bind_metric_action(
                                        metric_card,
                                        &card.id,
                                        config.clone(),
                                        action_inbox.clone(),
                                        action_handle.clone(),
                                        runtime.clone(),
                                        shutdown.clone(),
                                    );
                                    let scheduler = scheduler.clone();
                                    let wake = scheduler_wake.clone();
                                    let card_id = card.id.clone();
                                    let runtime = runtime.clone();
                                    metric_card.refresh_btn.connect_clicked(move |button| {
                                        runtime.report_user_activity(UserActivity::ManualRefresh);
                                        if scheduler.borrow_mut().request_now(&card_id) {
                                            button.set_sensitive(false);
                                            button.set_tooltip_text(Some("正在刷新"));
                                            let _ = wake.try_send(());
                                        }
                                    });
                                }
                                let source_key = source_key_for(card.source.as_ref());
                                let sampling_consumer_key = next_sampling_consumer_key();
                                let source_node_key =
                                    source_node_key(&source_key, &card, sampling_consumer_key);
                                card_metas.borrow_mut().insert(
                                    card.id.clone(),
                                    CardMeta {
                                        page_id: card.page.clone(),
                                        interval_secs: card.refresh_interval,
                                        source_key,
                                        source_node_key,
                                        config_epoch: config_epoch.get(),
                                        sampling_consumer_key,
                                    },
                                );
                                scheduler.borrow_mut().register_with_policy(
                                    &card.id,
                                    card.refresh_interval,
                                    &card.page,
                                    task_policy(&card),
                                );
                                bind_card_refresh_sources(&mut refresh.borrow_mut(), &card);
                                scheduler
                                    .borrow_mut()
                                    .set_active_page(&current_page_id.borrow());
                                let _ = scheduler_wake.try_send(());
                            }
                        }
                    } else if let Some(page) = pages.borrow_mut().get_mut(&card.page) {
                        previous_results.borrow_mut().remove(&card.id);
                        if let Some(metric_card) = page.metric_cards.remove(&card.id) {
                            page.metric_flow.remove(&metric_card.card);
                        }
                        card_metas.borrow_mut().remove(&card.id);
                        scheduler.borrow_mut().unregister(&card.id);
                        refresh.borrow_mut().unbind_task(&card.id);
                    }
                    prune_source_nodes(&source_nodes, &card_metas);
                    install_file_monitors(
                        &file_monitors,
                        &file_fallback,
                        &cards_for_watchers,
                        refresh.clone(),
                        scheduler.clone(),
                        scheduler_wake.clone(),
                        card_metas.clone(),
                        source_nodes.clone(),
                    );
                }
            });
            row.append(&toggle);
            page.flow_insert(&row);
        }

        let refresh_btn = Button::with_label("刷新全部指标");
        refresh_btn.add_css_class("pill");
        refresh_btn.set_halign(Align::Center);
        let scheduler = self.scheduler.clone();
        let wake = self.scheduler_wake.clone();
        let runtime = self.runtime.clone();
        refresh_btn.connect_clicked(move |_| {
            runtime.report_user_activity(UserActivity::ManualRefresh);
            scheduler.borrow_mut().request_all_now();
            let _ = wake.try_send(());
        });
        page.flow_insert(&refresh_btn);
    }

    fn setup_lifecycle(&self) {
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed({
            let runtime = self.runtime.clone();
            move |_, _, _, _| runtime.report_user_activity(UserActivity::Click)
        });
        self.window.add_controller(click);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        scroll.connect_scroll({
            let runtime = self.runtime.clone();
            move |_, _, _| {
                runtime.report_user_activity(UserActivity::Scroll);
                glib::Propagation::Proceed
            }
        });
        self.window.add_controller(scroll);

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let runtime = self.runtime.clone();
            move |_, _, _, _| {
                runtime.report_user_activity(UserActivity::Keyboard);
                glib::Propagation::Proceed
            }
        });
        self.window.add_controller(keys);

        let drag = gtk::GestureDrag::new();
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        drag.connect_drag_begin({
            let runtime = self.runtime.clone();
            move |_, _, _| runtime.report_user_activity(UserActivity::Drag)
        });
        self.window.add_controller(drag);

        let runtime_rx = self.runtime.subscribe();
        let runtime_scheduler = self.scheduler.clone();
        let runtime_wake = self.scheduler_wake.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(snapshot) = runtime_rx.recv().await {
                let mut scheduler = runtime_scheduler.borrow_mut();
                scheduler.set_work_level(
                    snapshot.work_level,
                    snapshot.visibility == Visibility::MappedInactive,
                    snapshot.activity == Activity::Idle,
                );
                scheduler.set_periodic_refresh_paused(snapshot.periodic_refresh_paused);
                drop(scheduler);
                let _ = runtime_wake.try_send(());
            }
        });

        let current_page = self.current_page_id.clone();
        let scheduler = self.scheduler.clone();
        let scheduler_wake = self.scheduler_wake.clone();
        let view_stack = self.view_stack.clone();
        let runtime = self.runtime.clone();

        view_stack.connect_visible_child_name_notify(move |stack| {
            if let Some(name) = stack.visible_child_name() {
                runtime.report_user_activity(UserActivity::PageSwitch);
                let name_str = name.to_string();
                *current_page.borrow_mut() = name_str.clone();

                scheduler
                    .borrow_mut()
                    .set_active_page(&current_page.borrow());
                let _ = scheduler_wake.try_send(());
            }
        });
    }

    fn setup_config_monitor(&mut self) {
        let config_path_buf = config_path();
        if !config_path_buf.exists() || !self.config.borrow().config().app.reload_on_change {
            return;
        }

        let modules_path_buf = config_modules_dir();
        let _ = std::fs::create_dir_all(&modules_path_buf);
        let mut monitors = Vec::new();
        if let Ok(monitor) = gio::File::for_path(&config_path_buf)
            .monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        {
            monitors.push(monitor);
        }
        if let Ok(monitor) = gio::File::for_path(&modules_path_buf)
            .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        {
            monitors.push(monitor);
        }
        if monitors.is_empty() {
            return;
        }

        let config_ref = self.config.clone();
        let reload_guard = self.reload_guard.clone();
        let runtime = self.runtime.clone();
        let previous_results = self.previous_results.clone();
        let scheduler = self.scheduler.clone();
        let scheduler_wake = self.scheduler_wake.clone();
        let card_metas = self.card_metas.clone();
        let source_nodes = self.source_nodes.clone();
        let file_monitors = self.file_monitors.clone();
        let file_fallback = self.file_fallback.clone();
        let refresh = self.refresh.clone();
        let max_output_budget = self.max_output_budget.clone();
        let config_epoch = self.config_epoch.clone();

        let reload: Rc<dyn Fn(gio::FileMonitorEvent)> = Rc::new(move |event_type| {
            // Keep monitors installed after a true -> false transition so the
            // transition itself is applied, but ignore later file edges.
            if !config_ref.borrow().config().app.reload_on_change {
                return;
            }
            if matches!(
                event_type,
                gio::FileMonitorEvent::Changed
                    | gio::FileMonitorEvent::ChangesDoneHint
                    | gio::FileMonitorEvent::Created
                    | gio::FileMonitorEvent::Deleted
                    | gio::FileMonitorEvent::MovedIn
                    | gio::FileMonitorEvent::MovedOut
                    | gio::FileMonitorEvent::Renamed
            ) {
                let guard = reload_guard.clone();
                let cfg = config_ref.clone();
                let runtime = runtime.clone();
                let previous_results = previous_results.clone();
                let scheduler = scheduler.clone();
                let scheduler_wake = scheduler_wake.clone();
                let card_metas = card_metas.clone();
                let source_nodes = source_nodes.clone();
                let file_monitors = file_monitors.clone();
                let file_fallback = file_fallback.clone();
                let refresh = refresh.clone();
                let max_output_budget = max_output_budget.clone();
                let config_epoch = config_epoch.clone();

                let should_schedule;
                {
                    let mut last = guard.last_reload.borrow_mut();
                    let elapsed = last.elapsed();
                    if elapsed < Duration::from_millis(guard.debounce_ms) {
                        if !*guard.pending.borrow() {
                            *guard.pending.borrow_mut() = true;
                            should_schedule =
                                Some(guard.debounce_ms.saturating_sub(elapsed.as_millis() as u64));
                        } else {
                            should_schedule = None;
                        }
                    } else {
                        *last = Instant::now();
                        should_schedule = None;
                        drop(last);
                        let (previous, previous_effective) = {
                            let manager = cfg.borrow();
                            let raw = manager.config().clone();
                            let mut effective = raw.clone();
                            effective.cards =
                                effective_cards(&raw, manager.uses_default_card_registry());
                            (raw, effective)
                        };
                        if do_reload_config(&cfg) {
                            let (next, next_effective) = {
                                let manager = cfg.borrow();
                                let raw = manager.config().clone();
                                let mut effective = raw.clone();
                                effective.cards =
                                    effective_cards(&raw, manager.uses_default_card_registry());
                                (raw, effective)
                            };
                            apply_output_budget_change(
                                &previous,
                                &next,
                                &max_output_budget,
                                &scheduler,
                                &scheduler_wake,
                                &card_metas,
                                &source_nodes,
                            );
                            let changed = changed_card_ids(&previous_effective, &next_effective);
                            for card_id in &changed {
                                previous_results.borrow_mut().remove(card_id);
                                invalidate_changed_source_keys(
                                    &previous_effective,
                                    &next_effective,
                                    card_id,
                                    &source_nodes,
                                );
                            }
                            if !changed.is_empty() {
                                let next_epoch = config_epoch.get().wrapping_add(1);
                                reconcile_scheduler_cards(
                                    &scheduler,
                                    &card_metas,
                                    &source_nodes,
                                    &previous_effective,
                                    &next_effective,
                                    &changed,
                                    next_epoch,
                                );
                                rebind_refresh_sources(
                                    &refresh,
                                    &previous_effective,
                                    &next_effective,
                                    &changed,
                                );
                                install_file_monitors(
                                    &file_monitors,
                                    &file_fallback,
                                    &next_effective.cards,
                                    refresh.clone(),
                                    scheduler.clone(),
                                    scheduler_wake.clone(),
                                    card_metas.clone(),
                                    source_nodes.clone(),
                                );
                                config_epoch.set(next_epoch);
                            }
                            for card_id in changed {
                                if next_effective.cards.iter().any(|card| {
                                    card.id == card_id && card.enabled && card.schedule.is_none()
                                }) {
                                    let _ = scheduler.borrow_mut().request_config_reload(&card_id);
                                }
                            }
                            let _ = scheduler_wake.try_send(());
                            runtime.update_config(next.runtime);
                        }
                    }
                }

                if let Some(remaining_ms) = should_schedule {
                    let cfg2 = cfg.clone();
                    let runtime2 = runtime.clone();
                    let previous_results2 = previous_results.clone();
                    let scheduler2 = scheduler.clone();
                    let scheduler_wake2 = scheduler_wake.clone();
                    let card_metas2 = card_metas.clone();
                    let source_nodes2 = source_nodes.clone();
                    let file_monitors2 = file_monitors.clone();
                    let file_fallback2 = file_fallback.clone();
                    let refresh2 = refresh.clone();
                    let max_output_budget2 = max_output_budget.clone();
                    let config_epoch2 = config_epoch.clone();
                    let sid_cell = guard.source_id.clone();

                    let sid_cell_for_timer = sid_cell.clone();
                    let sid = glib::timeout_add_local(
                        Duration::from_millis(remaining_ms + 50),
                        move || {
                            sid_cell_for_timer.borrow_mut().take();
                            guard.pending.replace(false);
                            let (previous, previous_effective) = {
                                let manager = cfg2.borrow();
                                let raw = manager.config().clone();
                                let mut effective = raw.clone();
                                effective.cards =
                                    effective_cards(&raw, manager.uses_default_card_registry());
                                (raw, effective)
                            };
                            if do_reload_config(&cfg2) {
                                let (next, next_effective) = {
                                    let manager = cfg2.borrow();
                                    let raw = manager.config().clone();
                                    let mut effective = raw.clone();
                                    effective.cards =
                                        effective_cards(&raw, manager.uses_default_card_registry());
                                    (raw, effective)
                                };
                                apply_output_budget_change(
                                    &previous,
                                    &next,
                                    &max_output_budget2,
                                    &scheduler2,
                                    &scheduler_wake2,
                                    &card_metas2,
                                    &source_nodes2,
                                );
                                let changed =
                                    changed_card_ids(&previous_effective, &next_effective);
                                for card_id in &changed {
                                    previous_results2.borrow_mut().remove(card_id);
                                    invalidate_changed_source_keys(
                                        &previous_effective,
                                        &next_effective,
                                        card_id,
                                        &source_nodes2,
                                    );
                                }
                                if !changed.is_empty() {
                                    let next_epoch = config_epoch2.get().wrapping_add(1);
                                    reconcile_scheduler_cards(
                                        &scheduler2,
                                        &card_metas2,
                                        &source_nodes2,
                                        &previous_effective,
                                        &next_effective,
                                        &changed,
                                        next_epoch,
                                    );
                                    rebind_refresh_sources(
                                        &refresh2,
                                        &previous_effective,
                                        &next_effective,
                                        &changed,
                                    );
                                    install_file_monitors(
                                        &file_monitors2,
                                        &file_fallback2,
                                        &next_effective.cards,
                                        refresh2.clone(),
                                        scheduler2.clone(),
                                        scheduler_wake2.clone(),
                                        card_metas2.clone(),
                                        source_nodes2.clone(),
                                    );
                                    config_epoch2.set(next_epoch);
                                }
                                for card_id in changed {
                                    if next_effective.cards.iter().any(|card| {
                                        card.id == card_id
                                            && card.enabled
                                            && card.schedule.is_none()
                                    }) {
                                        let _ =
                                            scheduler2.borrow_mut().request_config_reload(&card_id);
                                    }
                                }
                                let _ = scheduler_wake2.try_send(());
                                runtime2.update_config(next.runtime);
                            }
                            glib::ControlFlow::Break
                        },
                    );
                    sid_cell.replace(Some(sid));
                }
            }
        });
        for monitor in &monitors {
            let reload = reload.clone();
            let config_path = config_path_buf.clone();
            let modules_path = modules_path_buf.clone();
            monitor.connect_changed(move |_monitor, file, other_file, event_type| {
                let relevant = [Some(file), other_file]
                    .into_iter()
                    .flatten()
                    .filter_map(|file| file.path())
                    .any(|path| {
                        path == config_path
                            || (path.parent() == Some(modules_path.as_path())
                                && matches!(
                                    path.extension().and_then(|value| value.to_str()),
                                    Some("toml" | "json")
                                ))
                    });
                if relevant {
                    reload(event_type);
                }
            });
        }
        self.config_monitors = monitors;
    }

    fn start_scheduler_polling(&self, wake_rx: async_channel::Receiver<()>) {
        let scheduler = self.scheduler.clone();
        let card_metas = self.card_metas.clone();
        let config = self.config.clone();
        let handle = self.handle.clone();
        let metric_ctx = self.metric_ctx.clone();
        let source_nodes = self.source_nodes.clone();
        let max_output_budget = self.max_output_budget.clone();
        let metric_inbox = self.metric_inbox.clone();
        let shutdown = self.shutdown.clone();
        glib::MainContext::default().spawn_local(async move {
            loop {
                if shutdown.load(Ordering::Acquire) {
                    break;
                }
                let delay = scheduler
                    .borrow_mut()
                    .next_task()
                    .map(|next| next.saturating_duration_since(Instant::now()))
                    .unwrap_or(Duration::from_secs(3600));
                let timer = Box::pin(glib::timeout_future(delay.max(Duration::from_millis(10))));
                let wake = Box::pin(wake_rx.recv());
                let wake_closed = match futures_util::future::select(timer, wake).await {
                    futures_util::future::Either::Left((_timer, _wake)) => false,
                    futures_util::future::Either::Right((result, _timer)) => result.is_err(),
                };
                if wake_closed {
                    break;
                }
                crate::core::power_debug::increment(
                    crate::core::power_debug::Counter::SchedulerWake,
                );
                let ready = scheduler.borrow_mut().poll();

                for card_id in ready {
                    if shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    let meta = match card_metas.borrow().get(&card_id) {
                        Some(m) => CardMeta {
                            page_id: m.page_id.clone(),
                            interval_secs: m.interval_secs,
                            source_key: m.source_key.clone(),
                            source_node_key: m.source_node_key.clone(),
                            config_epoch: m.config_epoch,
                            sampling_consumer_key: m.sampling_consumer_key,
                        },
                        None => continue,
                    };

                    scheduler.borrow_mut().mark_started(&card_id);

                    let cfg = config.borrow();
                    let use_default_cards = cfg.uses_default_card_registry();
                    let card_cfg = effective_card_config(cfg.config(), &card_id, use_default_cards);
                    drop(cfg);

                    let card_cfg = match card_cfg {
                        Some(c) => c,
                        None => {
                            scheduler
                                .borrow_mut()
                                .mark_done(&card_id, meta.interval_secs, false);
                            continue;
                        }
                    };

                    if !card_cfg.enabled {
                        scheduler
                            .borrow_mut()
                            .mark_done(&card_id, meta.interval_secs, false);
                        continue;
                    }

                    let source_key = meta.source_key.clone();
                    let source_revision = scheduler.borrow().active_revision(&card_id);
                    let revision_key = scheduler
                        .borrow()
                        .active_source_key(&card_id)
                        .map(|key| key.as_str().to_owned());
                    let task_generation =
                        scheduler.borrow().generation(&card_id).unwrap_or_default();
                    // Epochs are owned by each card. An unrelated card reload
                    // must not reject this card's otherwise current result.
                    let epoch = meta.config_epoch;
                    let invalidation_generation = cache::invalidation_token(&source_key);
                    let inbox = metric_inbox.clone();
                    let h = handle.clone();
                    let ctx = metric_ctx.clone();
                    let nodes = source_nodes.clone();
                    // Read the current budget for cache replay and again inside
                    // the queued worker immediately before collection.
                    let max_output = max_output_budget.load(Ordering::Acquire).max(1);

                    let source = card_cfg.source.clone();
                    let node_key = meta.source_node_key.clone();
                    let needs_initial_follow_up = source
                        .as_ref()
                        .is_some_and(|source| source.builtin_metric() == Some("cpu"));
                    // Counter/window metrics own mutable sampling state and
                    // must not be satisfied by a card-level disk cache. Their
                    // source node is scoped to this opaque consumer.
                    let cache_allowed = source.as_ref().is_none_or(|source| {
                        !matches!(
                            source,
                            SourceConfig::Builtin(metric)
                                if builtin_uses_stateful_sampling(metric)
                        )
                    });
                    let cache_ttl = card_cfg.cache_ttl_seconds.filter(|_| cache_allowed);
                    let cacheable_source = cache_allowed
                        && source.as_ref().is_some_and(|source| {
                            matches!(
                                source,
                                SourceConfig::Command(_)
                                    | SourceConfig::File(_)
                                    | SourceConfig::Http(_)
                            )
                        });
                    let schedule = card_cfg
                        .schedule
                        .as_deref()
                        .map(crate::core::schedule::evaluate);
                    let schedule_state = match schedule {
                        Some(Ok(state)) => Some(state),
                        Some(Err(error)) => {
                            publish_metric(
                                &inbox,
                                MetricUpdate {
                                    card_id,
                                    page_id: meta.page_id,
                                    source_key: source_key.clone(),
                                    source_revision,
                                    revision_key: revision_key.clone(),
                                    config_epoch: epoch,
                                    task_generation,
                                    result: MetricResult::error(error),
                                    collection_health: CollectionHealth::Failed,
                                    max_output_bytes: max_output,
                                    invalidation_generation,
                                    interval_secs: meta.interval_secs,
                                    next_delay: None,
                                    next_deadline: None,
                                },
                            );
                            continue;
                        }
                        None => None,
                    };
                    let schedule_deadline = schedule_state.as_ref().map(|schedule| {
                        Instant::now() + Duration::from_secs(schedule.next_delay_seconds.max(1))
                    });

                    // Scheduled data is immutable within one configured time slot. Load it
                    // from disk instead of repeating external requests after every launch.
                    if let Some(schedule) = &schedule_state {
                        if schedule.period.is_none() {
                            publish_metric(
                                &inbox,
                                MetricUpdate {
                                    card_id,
                                    page_id: meta.page_id,
                                    source_key: source_key.clone(),
                                    source_revision,
                                    revision_key: revision_key.clone(),
                                    invalidation_generation,
                                    config_epoch: epoch,
                                    task_generation,
                                    result: MetricResult::unavailable("等待第一个计划更新时间"),
                                    collection_health: CollectionHealth::Failed,
                                    max_output_bytes: max_output,
                                    interval_secs: schedule.next_delay_seconds,
                                    next_delay: None,
                                    next_deadline: schedule_deadline,
                                },
                            );
                            continue;
                        }
                    }

                    let cache_source_key = source_key.clone();
                    let collect_source_key = source_key.clone();
                    let cache_token = cache::invalidation_token(&cache_source_key);
                    let budget_source = max_output_budget.clone();
                    let cache_lookup_schedule = schedule_state.clone();
                    let cache_lookup_ttl = cache_ttl;
                    let cache_lookup_enabled =
                        cacheable_source || schedule_state.is_some() || cache_ttl.is_some();
                    let task_shutdown = shutdown.clone();
                    h.spawn(async move {
                        if task_shutdown.load(Ordering::Acquire) {
                            return;
                        }
                        let collection_budget = budget_source.load(Ordering::Acquire).max(1);
                        let cache_lookup_key = cache_source_key.clone();
                        let (mut result, cache_hit) = tokio::task::spawn_blocking(move || {
                            if cache_lookup_enabled {
                                let cached = if let Some(schedule) = &cache_lookup_schedule {
                                    schedule.period.as_deref().and_then(|period| {
                                        cache::load_with_budget(
                                            &cache_lookup_key,
                                            None,
                                            Some(period),
                                            collection_budget,
                                        )
                                    })
                                } else {
                                    cache_lookup_ttl.and_then(|ttl| {
                                        cache::load_with_budget(
                                            &cache_lookup_key,
                                            Some(ttl),
                                            None,
                                            collection_budget,
                                        )
                                    })
                                };
                                if let Some(result) = cached {
                                    return (result, true);
                                }
                            }
                            crate::core::power_debug::increment(
                                crate::core::power_debug::Counter::CardCollect,
                            );
                            (
                                collect_card_metric(
                                    &collect_source_key,
                                    &node_key,
                                    &source,
                                    &ctx,
                                    &nodes,
                                    collection_budget,
                                ),
                                false,
                            )
                        })
                        .await
                        .unwrap_or_else(|e| {
                            (
                                MetricResult::error(format!("metric task panicked: {}", e)),
                                false,
                            )
                        });
                        if task_shutdown.load(Ordering::Acquire) {
                            return;
                        }
                        if cache_hit {
                            let interval = schedule_state
                                .as_ref()
                                .map(|schedule| schedule.next_delay_seconds)
                                .unwrap_or(meta.interval_secs);
                            publish_metric(
                                &inbox,
                                MetricUpdate {
                                    card_id,
                                    page_id: meta.page_id.clone(),
                                    source_key,
                                    source_revision,
                                    revision_key,
                                    config_epoch: epoch,
                                    task_generation,
                                    invalidation_generation,
                                    collection_health: CollectionHealth::cache_hit(),
                                    max_output_bytes: collection_budget,
                                    result,
                                    interval_secs: interval,
                                    next_delay: None,
                                    next_deadline: schedule_deadline,
                                },
                            );
                            return;
                        }
                        let collection_health = metric_result_collection_health(&result);

                        if cache_allowed
                            && (cacheable_source || schedule_state.is_some() || cache_ttl.is_some())
                        {
                            let cache_key = format!("cache:{cache_source_key}");
                            let period = schedule_state
                                .as_ref()
                                .and_then(|state| state.period.as_deref());
                            if collection_health.is_healthy()
                                && cache::result_within_output_budget(&result, collection_budget)
                            {
                                match cache::store_if_current(
                                    &cache_source_key,
                                    period,
                                    &result,
                                    cache_token,
                                ) {
                                    Ok(true) => crate::core::error_limiter::recovered(&cache_key),
                                    Ok(false) => {
                                        // A source edge won the race with this
                                        // worker. Keep the cache dirty so the
                                        // follow-up cannot replay this result.
                                    }
                                    Err(error) => crate::core::error_limiter::warn(
                                        cache_key.clone(),
                                        format!("failed to persist source cache: {error}"),
                                    ),
                                }
                            } else if let Some(last_good) =
                                cache::load_last_good_with_max_age_and_budget(
                                    &cache_source_key,
                                    Some(cache::DEFAULT_LAST_GOOD_MAX_STALENESS_SECONDS),
                                    period,
                                    collection_budget,
                                )
                            {
                                // Preserve the last-good visible value after a
                                // transient command/HTTP/file failure. The
                                // stale state remains explicit in the model,
                                // while collection_health keeps backoff.
                                result = last_good;
                            }
                        }

                        let page_id = meta.page_id.clone();
                        let interval = if let Some(schedule) = &schedule_state {
                            schedule.next_delay_seconds
                        } else {
                            meta.interval_secs
                        };

                        publish_metric(
                            &inbox,
                            MetricUpdate {
                                card_id,
                                page_id,
                                source_key,
                                source_revision,
                                revision_key,
                                config_epoch: epoch,
                                task_generation,
                                invalidation_generation,
                                collection_health,
                                max_output_bytes: collection_budget,
                                next_delay: if needs_initial_follow_up
                                    && result.state == MetricState::Loading
                                {
                                    Some(Duration::from_millis(250))
                                } else {
                                    None
                                },
                                result,
                                interval_secs: interval,
                                next_deadline: schedule_deadline,
                            },
                        );
                    });
                }
            }
        });
    }

    fn start_metric_receiver(&self, wake_rx: async_channel::Receiver<()>) {
        let pages = self.pages.clone();
        let previous_results = self.previous_results.clone();
        let scheduler = self.scheduler.clone();
        let card_metas = self.card_metas.clone();
        let config = self.config.clone();
        let max_output_budget = self.max_output_budget.clone();
        let scheduler_wake = self.scheduler_wake.clone();
        let refresh = self.refresh.clone();
        let inbox = self.metric_inbox.clone();

        glib::MainContext::default().spawn_local(async move {
            while wake_rx.recv().await.is_ok() {
                let updates = inbox.take_batch(MAX_UI_UPDATES_PER_TURN);
                if updates.is_empty() {
                    continue;
                }
                let batch_size = updates.len();
                let batch_started = Instant::now();
                for update in updates {
                    // A reload may replace a running task with a new
                    // generation under the same card id. Its old worker must
                    // not complete or reschedule the replacement task.
                    if scheduler.borrow().generation(&update.card_id)
                        != Some(update.task_generation)
                    {
                        continue;
                    }
                    let meta = match card_metas.borrow().get(&update.card_id) {
                        Some(m) => CardMeta {
                            page_id: m.page_id.clone(),
                            interval_secs: m.interval_secs,
                            source_key: m.source_key.clone(),
                            source_node_key: m.source_node_key.clone(),
                            config_epoch: m.config_epoch,
                            sampling_consumer_key: m.sampling_consumer_key,
                        },
                        None => continue,
                    };
                    let current_revision = update
                        .revision_key
                        .as_deref()
                        .map(|key| refresh.borrow().revision(&SourceKey::new(key.to_owned())));
                    if update.config_epoch != meta.config_epoch
                        || update.source_key != meta.source_key
                        || update.max_output_bytes
                            != max_output_budget.load(Ordering::Acquire).max(1)
                        || !cache::matches_invalidation_generation(
                            &update.source_key,
                            update.invalidation_generation,
                        )
                        || current_revision
                            .is_some_and(|revision| update.source_revision < revision)
                    {
                        // A stale worker still has to complete its scheduler
                        // slot; pending source revisions then produce one
                        // follow-up rather than being lost.
                        scheduler
                            .borrow_mut()
                            .mark_done_after_revision_with_deadline(
                                &update.card_id,
                                update.interval_secs,
                                update.collection_health.is_healthy(),
                                None,
                                update.next_deadline,
                                update.source_revision,
                            );
                        let _ = scheduler_wake.try_send(());
                        continue;
                    }

                    let card_cfg = {
                        let cfg = config.borrow();
                        effective_card_config(
                            cfg.config(),
                            &update.card_id,
                            cfg.uses_default_card_registry(),
                        )
                    };
                    let display = card_cfg.as_ref().and_then(|card| card.display.clone());

                    let should_skip = {
                        let prev = previous_results.borrow();
                        if let Some(last) = prev.get(&update.card_id) {
                            results_equivalent(last, &update.result, display.as_ref())
                        } else {
                            false
                        }
                    };

                    if let Some(page) = pages.borrow_mut().get_mut(&update.page_id) {
                        if let Some(card) = page.get_metric_card(&update.card_id) {
                            card.set_refresh_pending(false);
                        }
                    }

                    if !should_skip {
                        crate::core::power_debug::increment(
                            crate::core::power_debug::Counter::GtkUpdate,
                        );
                        let apply_started = Instant::now();
                        if let Some(page) = pages.borrow_mut().get_mut(&update.page_id) {
                            if let Some(card) = page.get_metric_card(&update.card_id) {
                                apply_metric_result(card, &update.result, display.as_ref());
                            }
                        }
                        let apply_elapsed = apply_started.elapsed();
                        if apply_elapsed >= Duration::from_millis(100) {
                            tracing::warn!(
                                card = %update.card_id,
                                elapsed_ms = apply_elapsed.as_millis() as u64,
                                "metric GTK projection exceeded budget"
                            );
                        }
                        previous_results
                            .borrow_mut()
                            .insert(update.card_id.clone(), update.result.clone());
                    }

                    scheduler
                        .borrow_mut()
                        .mark_done_after_revision_with_deadline(
                            &update.card_id,
                            update.interval_secs,
                            update.collection_health.is_healthy(),
                            update.next_delay,
                            update.next_deadline,
                            update.source_revision,
                        );
                    let _ = scheduler_wake.try_send(());
                }
                let batch_elapsed = batch_started.elapsed();
                if batch_elapsed >= Duration::from_millis(100) {
                    tracing::warn!(
                        batch = batch_size,
                        elapsed_ms = batch_elapsed.as_millis() as u64,
                        pending = inbox.len(),
                        "metric GTK update batch exceeded budget"
                    );
                }
                if inbox.has_pending() {
                    inbox.wake();
                    // Bound the amount of GTK work performed in one main-loop
                    // turn so a burst of completions cannot starve input or
                    // frame-clock processing.
                    glib::timeout_future(Duration::from_millis(1)).await;
                }
            }
        });
    }

    fn start_action_receiver(&self, wake_rx: async_channel::Receiver<()>) {
        let pages = self.pages.clone();
        let runtime = self.runtime.clone();
        let config = self.config.clone();
        let card_metas = self.card_metas.clone();
        let scheduler = self.scheduler.clone();
        let scheduler_wake = self.scheduler_wake.clone();
        let refresh = self.refresh.clone();
        let source_nodes = self.source_nodes.clone();
        let inbox = self.action_inbox.clone();
        let completed = Rc::new(RefCell::new(InvocationDedup {
            seen: std::collections::HashSet::new(),
            order: std::collections::VecDeque::new(),
        }));

        glib::MainContext::default().spawn_local(async move {
            while wake_rx.recv().await.is_ok() {
                let updates = inbox.take_batch(MAX_UI_UPDATES_PER_TURN);
                if updates.is_empty() {
                    continue;
                }
                let batch_size = updates.len();
                let batch_started = Instant::now();
                for update in updates {
                    if !completed.borrow_mut().insert(update.invocation_id) {
                        continue;
                    }

                    // Action completion is a generic relationship lookup: any
                    // enabled card linked through click_action is invalidated,
                    // whether visible, hidden, or on another page. Failure is
                    // intentionally included because a partial command may
                    // still have changed observable state.
                    let linked_cards = linked_card_ids(config.borrow().config(), &update.action_id);
                    let mut linked_sources = Vec::new();
                    let mut cards_without_source = Vec::new();
                    for card_id in linked_cards {
                        if let Some(meta) = card_metas.borrow().get(&card_id) {
                            if !linked_sources.iter().any(|key| key == &meta.source_key) {
                                linked_sources.push(meta.source_key.clone());
                            }
                        } else {
                            cards_without_source.push(card_id);
                        }
                    }
                    linked_sources.sort();
                    for source_key in linked_sources {
                        publish_source_event(
                            &refresh,
                            &scheduler,
                            &scheduler_wake,
                            &card_metas,
                            &source_nodes,
                            &source_key,
                            SourceEventKind::Changed,
                        );
                    }
                    // Keep the generic one-shot behavior for an enabled linked
                    // card that has no standard source/task (for example a
                    // plugin-owned control), without inventing source logic.
                    for card_id in cards_without_source {
                        let _ = scheduler.borrow_mut().request_with_reason(
                            &card_id,
                            RefreshReason::Manual,
                            SourceRevision::INITIAL,
                        );
                    }
                    let _ = scheduler_wake.try_send(());

                    if let Some(card_id) = update.result_card_id.as_deref() {
                        for page in pages.borrow_mut().values_mut() {
                            if let Some(card) = page.get_metric_card(card_id) {
                                card.set_action_running(false);
                                show_action_result_dialog(
                                    &card.card,
                                    &update.action_id,
                                    &update.result,
                                    runtime.clone(),
                                );
                                break;
                            }
                        }
                    }
                    for (_page_id, page) in pages.borrow_mut().iter_mut() {
                        if let Some(card) = page.get_action_card(&update.action_id) {
                            card.set_running(false);
                            show_action_result_dialog(
                                &card.card,
                                &update.action_id,
                                &update.result,
                                runtime.clone(),
                            );
                            break;
                        }
                    }
                }
                let batch_elapsed = batch_started.elapsed();
                if batch_elapsed >= Duration::from_millis(100) {
                    tracing::warn!(
                        batch = batch_size,
                        elapsed_ms = batch_elapsed.as_millis() as u64,
                        pending = inbox.len(),
                        "action GTK update batch exceeded budget"
                    );
                }
                if inbox.has_pending() {
                    inbox.wake();
                    glib::timeout_future(Duration::from_millis(1)).await;
                }
            }
        });
    }

    pub fn present(&self) {
        self.window.present();
    }
}

impl Drop for MonitorWindow {
    fn drop(&mut self) {
        // Closing the channels wakes the controller-owned local consumers even
        // when a task still retains a sender clone. Runtime subscriptions need
        // the same explicit boundary because RuntimeManager is also retained
        // by those consumers.
        self.metric_inbox.close();
        self.action_inbox.close();
        self.scheduler_wake.close();
        self.shutdown.store(true, Ordering::Release);
        self.runtime.shutdown();

        if let Some(source) = self.heartbeat_source.take() {
            source.remove();
        }

        if let Some(source) = self.file_fallback.borrow_mut().take() {
            source.remove();
        }
        if let Some(source) = self.network_fallback.take() {
            source.remove();
        }
        if let Some(source) = self.network_debounce.borrow_mut().take() {
            source.remove();
        }
        if let Some(source) = self.reload_guard.source_id.borrow_mut().take() {
            source.remove();
        }
        if let Some((connection, subscription)) = self.network_dbus.take() {
            connection.signal_unsubscribe(subscription);
        }
        gtk::style_context_remove_provider_for_display(
            &self.app_style_display,
            &self.app_style_provider,
        );
    }
}

fn effective_cards(config: &AppConfig, use_default_cards: bool) -> Vec<CardConfig> {
    if !use_default_cards {
        return config
            .cards
            .iter()
            .filter(|card| card.enabled)
            .cloned()
            .collect();
    }
    // The root owns the default-registry decision. Generated module entries
    // layer over the shipped registry, and removing an override reveals the
    // shipped card again rather than deleting its runtime metadata.
    let mut cards = default_builtin_cards();
    for configured in config.cards.iter().filter(|card| card.enabled) {
        if let Some(existing) = cards.iter_mut().find(|card| card.id == configured.id) {
            *existing = configured.clone();
        } else {
            cards.push(configured.clone());
        }
    }
    cards
}

fn effective_card_config(
    config: &AppConfig,
    card_id: &str,
    use_default_cards: bool,
) -> Option<CardConfig> {
    effective_cards(config, use_default_cards)
        .into_iter()
        .find(|card| card.id == card_id)
}

fn changed_card_ids(
    previous: &crate::core::config::AppConfig,
    next: &crate::core::config::AppConfig,
) -> Vec<String> {
    let mut ids = std::collections::HashSet::new();
    ids.extend(previous.cards.iter().map(|card| card.id.clone()));
    ids.extend(next.cards.iter().map(|card| card.id.clone()));
    let mut changed = ids
        .into_iter()
        .filter(|id| {
            let before = previous.cards.iter().find(|card| &card.id == id);
            let after = next.cards.iter().find(|card| &card.id == id);
            match (before, after) {
                (Some(before), Some(after)) => {
                    serde_json::to_string(before).ok() != serde_json::to_string(after).ok()
                }
                _ => true,
            }
        })
        .collect::<Vec<_>>();
    changed.sort();
    changed
}

fn apply_output_budget_change(
    previous: &AppConfig,
    next: &AppConfig,
    budget: &Arc<AtomicUsize>,
    scheduler: &Rc<RefCell<Scheduler>>,
    wake: &async_channel::Sender<()>,
    card_metas: &Rc<RefCell<HashMap<String, CardMeta>>>,
    source_nodes: &Arc<Mutex<HashMap<String, Arc<SourceNode>>>>,
) {
    let previous_budget = previous.app.max_output_bytes.max(1);
    let next_budget = next.app.max_output_bytes.max(1);
    if previous_budget == next_budget {
        return;
    }
    budget.store(next_budget, Ordering::Release);

    let active_cards = card_metas
        .borrow()
        .iter()
        .map(|(card_id, meta)| (card_id.clone(), meta.source_key.clone()))
        .collect::<Vec<_>>();
    for (_, source_key) in &active_cards {
        invalidate_source(source_nodes, source_key);
        cache::invalidate(source_key);
    }
    for (card_id, _) in active_cards {
        let scheduled = next
            .cards
            .iter()
            .find(|card| card.id == card_id)
            .is_some_and(|card| card.schedule.is_some());
        if !scheduled {
            let _ = scheduler.borrow_mut().request_config_reload(&card_id);
        }
    }
    let _ = wake.try_send(());
}

fn rebind_refresh_sources(
    refresh: &Rc<RefCell<RefreshCoordinator>>,
    previous: &crate::core::config::AppConfig,
    next: &crate::core::config::AppConfig,
    changed: &[String],
) {
    let mut coordinator = refresh.borrow_mut();
    for card_id in changed {
        coordinator.unbind_task(card_id);
    }
    for card in next.cards.iter().filter(|card| card.enabled) {
        // Unchanged cards retain their source revision. Changed/new cards were
        // unbound above (or were absent) and are installed with fresh edges.
        if previous.cards.iter().any(|current| {
            current.id == card.id
                && !changed.iter().any(|changed_id| changed_id == &card.id)
                && current.enabled
        }) {
            continue;
        }
        bind_card_refresh_sources(&mut coordinator, card);
    }
}

fn bind_card_refresh_sources(coordinator: &mut RefreshCoordinator, card: &CardConfig) {
    let source_key = source_key_for(card.source.as_ref());
    coordinator.register_source(source_key.clone());
    coordinator.bind(source_key, card.id.clone());
    if card
        .source
        .as_ref()
        .and_then(SourceConfig::builtin_metric)
        .is_some_and(|metric| {
            matches!(metric, "battery_capacity" | "battery_temperature" | "power")
        })
    {
        coordinator.bind("signal:power-supply", card.id.clone());
    }
    if card.source.as_ref().and_then(SourceConfig::builtin_metric) == Some("network") {
        coordinator.bind("signal:network", card.id.clone());
    }
    if card.source.as_ref().and_then(SourceConfig::builtin_metric) == Some("cpu_temperature") {
        coordinator.bind("signal:thermal", card.id.clone());
    }
}

fn reconcile_scheduler_cards(
    scheduler: &Rc<RefCell<Scheduler>>,
    card_metas: &Rc<RefCell<HashMap<String, CardMeta>>>,
    source_nodes: &Arc<Mutex<HashMap<String, Arc<SourceNode>>>>,
    previous: &crate::core::config::AppConfig,
    next: &crate::core::config::AppConfig,
    changed: &[String],
    config_epoch: u64,
) {
    let mut scheduler = scheduler.borrow_mut();
    let mut metas = card_metas.borrow_mut();
    for card_id in changed {
        let Some(card) = next
            .cards
            .iter()
            .find(|card| card.id == *card_id && card.enabled)
        else {
            scheduler.unregister(card_id);
            metas.remove(card_id);
            continue;
        };

        // Keep an active runtime in place across a reload. Re-registering it
        // would create a fresh due heap entry while the old wall-clock slot is
        // still collecting. The old worker keeps its generation and completes
        // the active slot before the rebound policy schedules another one.
        if scheduler.is_running(card_id) {
            if let Some(meta) = metas.get_mut(card_id) {
                scheduler.rebind_with_policy(
                    card_id,
                    card.refresh_interval,
                    &card.page,
                    task_policy(card),
                );
                meta.page_id = card.page.clone();
                meta.interval_secs = card.refresh_interval;
                meta.source_key = source_key_for(card.source.as_ref());
                meta.source_node_key =
                    source_node_key(&meta.source_key, card, meta.sampling_consumer_key);
                meta.config_epoch = config_epoch;
                continue;
            }
        }

        let preserved_deadline = previous
            .cards
            .iter()
            .find(|before| before.id == *card_id && before.schedule.is_some())
            .filter(|before| card.schedule == before.schedule)
            .and_then(|_| scheduler.next_run(card_id));
        scheduler.unregister(card_id);

        // Existing UI cards can be rebound in place. A newly added card still
        // requires the normal page rebuild before it can own a GTK widget.
        if let Some(meta) = metas.get_mut(card_id) {
            meta.page_id = card.page.clone();
            meta.interval_secs = card.refresh_interval;
            meta.source_key = source_key_for(card.source.as_ref());
            meta.source_node_key =
                source_node_key(&meta.source_key, card, meta.sampling_consumer_key);
            meta.config_epoch = config_epoch;
            scheduler.register_with_policy(
                card_id,
                card.refresh_interval,
                &card.page,
                task_policy(card),
            );
            if let Some(deadline) = preserved_deadline {
                scheduler.restore_deadline(card_id, deadline);
            }
        }
    }
    drop(metas);
    drop(scheduler);
    prune_source_nodes(source_nodes, card_metas);
}

fn invalidate_changed_source_keys(
    previous: &crate::core::config::AppConfig,
    next: &crate::core::config::AppConfig,
    card_id: &str,
    source_nodes: &Arc<Mutex<HashMap<String, Arc<SourceNode>>>>,
) {
    for config in [
        previous.cards.iter().find(|card| card.id == card_id),
        next.cards.iter().find(|card| card.id == card_id),
    ]
    .into_iter()
    .flatten()
    {
        let key = source_key_for(config.source.as_ref());
        invalidate_source(source_nodes, &key);
        cache::invalidate(&key);
    }
}

fn linked_card_ids(config: &crate::core::config::AppConfig, action_id: &str) -> Vec<String> {
    let mut cards = config
        .cards
        .iter()
        .filter(|card| card.enabled && card.click_action.as_deref() == Some(action_id))
        .map(|card| card.id.clone())
        .collect::<Vec<_>>();
    cards.sort();
    cards
}

fn source_key_for(source: Option<&SourceConfig>) -> String {
    match source {
        None => "source:none".into(),
        Some(SourceConfig::Builtin(metric)) => format!("builtin:{metric}"),
        Some(SourceConfig::File(file)) => {
            format!("file:path={:?};first_line={}", file.path, file.first_line)
        }
        Some(SourceConfig::Command(command)) => format!(
            "command:run={:?};timeout={};max_output={};reverse_lines={};subtitle_lines={}",
            command.run,
            command.timeout_seconds,
            command.max_output_bytes,
            command.reverse_lines,
            command.subtitle_lines
        ),
        Some(SourceConfig::Http(http)) => {
            let mut headers = http
                .headers
                .as_ref()
                .map(|headers| headers.iter().collect::<Vec<_>>())
                .unwrap_or_default();
            headers.sort_by(|left, right| left.0.cmp(right.0));
            format!(
                "http:url={:?};method={:?};headers={:?};body={:?};timeout={};max_output={};parser={:?}",
                http.url,
                http.method,
                headers,
                http.body,
                http.timeout_seconds,
                http.max_output_bytes,
                http.parser
            )
        }
        Some(SourceConfig::Text(value)) => format!("text:{value:?}"),
    }
}

fn install_file_monitors(
    file_monitors: &Rc<RefCell<Vec<gio::FileMonitor>>>,
    file_fallback: &Rc<RefCell<Option<glib::SourceId>>>,
    cards: &[CardConfig],
    refresh: Rc<RefCell<RefreshCoordinator>>,
    scheduler: Rc<RefCell<Scheduler>>,
    wake: async_channel::Sender<()>,
    card_metas: Rc<RefCell<HashMap<String, CardMeta>>>,
    source_nodes: Arc<Mutex<HashMap<String, Arc<SourceNode>>>>,
) {
    file_monitors.borrow_mut().clear();
    if let Some(source) = file_fallback.borrow_mut().take() {
        source.remove();
    }
    let mut watched = std::collections::HashSet::new();
    for card in cards.iter().filter(|card| card.enabled) {
        let Some(SourceConfig::File(file_source)) = card.source.as_ref() else {
            continue;
        };
        let path = PathBuf::from(&file_source.path);
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let Some(watch_key) = nearest_existing_directory(parent) else {
            continue;
        };
        if !watched.insert(watch_key.clone()) {
            continue;
        }
        let Ok(monitor) = gio::File::for_path(&watch_key)
            .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        else {
            continue;
        };
        let refresh = refresh.clone();
        let scheduler = scheduler.clone();
        let wake = wake.clone();
        let card_metas = card_metas.clone();
        let source_nodes = source_nodes.clone();
        let cards = cards
            .iter()
            .filter(|card| card.enabled)
            .cloned()
            .collect::<Vec<_>>();
        monitor.connect_changed(move |_, file, other_file, event| {
            if !matches!(
                event,
                gio::FileMonitorEvent::Changed
                    | gio::FileMonitorEvent::ChangesDoneHint
                    | gio::FileMonitorEvent::Created
                    | gio::FileMonitorEvent::Deleted
                    | gio::FileMonitorEvent::Moved
                    | gio::FileMonitorEvent::MovedIn
                    | gio::FileMonitorEvent::MovedOut
                    | gio::FileMonitorEvent::Renamed
            ) {
                return;
            }
            let file_path = file.path();
            let other_path = other_file.and_then(|other| other.path());
            for card in &cards {
                let Some(SourceConfig::File(file_source)) = card.source.as_ref() else {
                    continue;
                };
                let target = PathBuf::from(&file_source.path);
                if file_event_matches_target(
                    file_path.as_deref(),
                    other_path.as_deref(),
                    target.as_path(),
                ) {
                    publish_source_event(
                        &refresh,
                        &scheduler,
                        &wake,
                        &card_metas,
                        &source_nodes,
                        &source_key_for(card.source.as_ref()),
                        SourceEventKind::Changed,
                    );
                }
            }
        });
        file_monitors.borrow_mut().push(monitor);
    }

    let mut source_keys = cards
        .iter()
        .filter(|card| card.enabled)
        .filter_map(|card| {
            card.source
                .as_ref()
                .is_some_and(|source| matches!(source, SourceConfig::File(_)))
                .then(|| source_key_for(card.source.as_ref()))
        })
        .collect::<Vec<_>>();
    source_keys.sort();
    source_keys.dedup();
    if !source_keys.is_empty() {
        let source = glib::timeout_add_local(Duration::from_secs(60), move || {
            for key in &source_keys {
                publish_source_event(
                    &refresh,
                    &scheduler,
                    &wake,
                    &card_metas,
                    &source_nodes,
                    key,
                    SourceEventKind::Changed,
                );
            }
            glib::ControlFlow::Continue
        });
        file_fallback.replace(Some(source));
    }
}

fn nearest_existing_directory(path: &std::path::Path) -> Option<PathBuf> {
    let mut current = path;
    loop {
        if current.is_dir() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn file_event_matches_target(
    file_path: Option<&std::path::Path>,
    other_path: Option<&std::path::Path>,
    target: &std::path::Path,
) -> bool {
    let absolute_target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(target))
            .unwrap_or_else(|_| target.to_path_buf())
    };
    let target_parent = absolute_target.parent();
    [file_path, other_path].into_iter().flatten().any(|path| {
        if path == target || path == absolute_target || target_parent == Some(path) {
            return true;
        }
        // If the configured parent did not exist when monitoring started,
        // watching its nearest existing ancestor still catches creation of a
        // path component and allows the first refresh to observe the file.
        if absolute_target.starts_with(path) {
            return true;
        }
        match (
            std::fs::canonicalize(path),
            std::fs::canonicalize(&absolute_target),
        ) {
            (Ok(path), Ok(target)) => path == target,
            _ => false,
        }
    })
}

fn publish_source_event(
    refresh: &Rc<RefCell<RefreshCoordinator>>,
    scheduler: &Rc<RefCell<Scheduler>>,
    wake: &async_channel::Sender<()>,
    card_metas: &Rc<RefCell<HashMap<String, CardMeta>>>,
    source_nodes: &Arc<Mutex<HashMap<String, Arc<SourceNode>>>>,
    source_key: &str,
    kind: SourceEventKind,
) {
    let source_key = SourceKey::new(source_key);
    invalidate_source(source_nodes, source_key.as_str());
    cache::invalidate(source_key.as_str());
    let (_, requests) = refresh.borrow_mut().publish(source_key, kind);
    let had_requests = !requests.is_empty();
    let mut projection_keys = Vec::new();
    for request in &requests {
        // A signal source is intentionally separate from a card's canonical
        // source key. Invalidate each dependent projection once, or a
        // cache_ttl card could replay the pre-edge value before collecting.
        if let Some(meta) = card_metas.borrow().get(&request.task) {
            if !projection_keys.iter().any(|key| key == &meta.source_key) {
                projection_keys.push(meta.source_key.clone());
            }
        }
    }
    for key in projection_keys {
        invalidate_source(source_nodes, &key);
        cache::invalidate(&key);
    }
    for request in requests {
        let _ = scheduler.borrow_mut().request_with_reason(
            &request.task,
            request.reason,
            request.requested_revision,
        );
    }
    if had_requests {
        let _ = wake.try_send(());
    }
}

fn invalidate_source(
    source_nodes: &Arc<Mutex<HashMap<String, Arc<SourceNode>>>>,
    source_key: &str,
) {
    let nodes = source_nodes
        .lock()
        .ok()
        .map(|nodes| {
            nodes
                .values()
                .filter(|node| node.descriptor_key == source_key)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // The collector may be blocking on a command/HTTP request. Keep the GTK
    // event path non-blocking; the atomic generation makes the next caller
    // bypass the short source coalescing window.
    for node in nodes {
        node.invalidate();
    }
}

fn next_sampling_consumer_key() -> u64 {
    NEXT_SAMPLING_CONSUMER_KEY.fetch_add(1, Ordering::Relaxed)
}

fn source_node_key(source_key: &str, card: &CardConfig, consumer_key: u64) -> String {
    let Some(metric) = card.source.as_ref().and_then(SourceConfig::builtin_metric) else {
        return source_key.to_owned();
    };
    if !builtin_uses_stateful_sampling(metric) {
        return source_key.to_owned();
    }
    // Stateful collectors hold deltas/windows between samples. Keep their
    // state separate for independent consumers while retaining the canonical
    // descriptor for event fan-out and cache identity. The consumer key is an
    // opaque in-memory lifetime token, never a card/device/path identifier.
    format!(
        "{source_key};sampling=refresh:{};schedule:{:?};runtime:{:?};consumer:{consumer_key}",
        card.refresh_interval, card.schedule, card.runtime
    )
}

fn prune_source_nodes(
    source_nodes: &Arc<Mutex<HashMap<String, Arc<SourceNode>>>>,
    card_metas: &Rc<RefCell<HashMap<String, CardMeta>>>,
) {
    let active_node_keys = card_metas
        .borrow()
        .values()
        .map(|meta| meta.source_node_key.clone())
        .collect::<std::collections::HashSet<_>>();
    let active_source_keys = card_metas
        .borrow()
        .values()
        .map(|meta| meta.source_key.clone())
        .collect::<std::collections::HashSet<_>>();
    let removed_source_keys = if let Ok(mut nodes) = source_nodes.lock() {
        let removed = nodes
            .iter()
            .filter(|(key, _)| !active_node_keys.contains(*key))
            .map(|(_, node)| node.descriptor_key.clone())
            .collect::<std::collections::HashSet<_>>();
        nodes.retain(|key, _| active_node_keys.contains(key));
        removed
    } else {
        std::collections::HashSet::new()
    };
    for source_key in removed_source_keys {
        if !active_source_keys.contains(&source_key) {
            cache::forget(&source_key);
        }
    }
    cache::prune(&active_source_keys);
}

fn collect_card_metric(
    source_key: &str,
    node_key: &str,
    source: &Option<SourceConfig>,
    ctx: &Arc<MetricContext>,
    source_nodes: &Arc<Mutex<HashMap<String, Arc<SourceNode>>>>,
    max_output: usize,
) -> MetricResult {
    let source = match source {
        Some(s) => s,
        None => {
            return MetricResult {
                value: CardValue::Text("等待配置...".into()),
                subtitle: None,
                tooltip: Some("此卡片未配置数据源".into()),
                state: MetricState::Unavailable,
                cached: false,
                metadata: None,
            }
        }
    };

    match source {
        SourceConfig::Builtin(metric_name) => {
            let node = {
                let mut registry = source_nodes.lock().unwrap();
                if let Some(node) = registry.get(node_key).cloned() {
                    node
                } else {
                    let metric = match create_builtin_metric(metric_name) {
                        Some(metric) => metric,
                        None => {
                            return MetricResult::error(format!("未知的内置指标: {metric_name}"));
                        }
                    };
                    let node = Arc::new(SourceNode::new(
                        SourceCollector::Builtin(metric),
                        source_key.to_string(),
                    ));
                    registry.insert(node_key.to_string(), node.clone());
                    node
                }
            };
            node.collect(ctx, max_output)
        }

        SourceConfig::Command(_)
        | SourceConfig::File(_)
        | SourceConfig::Http(_)
        | SourceConfig::Text(_) => {
            let node = {
                let mut registry = source_nodes.lock().unwrap();
                if let Some(node) = registry.get(node_key).cloned() {
                    node
                } else {
                    let source = match build_persistent_source(source) {
                        Ok(source) => source,
                        Err(result) => return result,
                    };
                    let node = Arc::new(SourceNode::new(
                        SourceCollector::Persistent(source),
                        source_key.to_string(),
                    ));
                    registry.insert(node_key.to_string(), node.clone());
                    node
                }
            };
            node.collect(ctx, max_output)
        }
    }
}

fn build_persistent_source(source: &SourceConfig) -> Result<PersistentSource, MetricResult> {
    match source {
        SourceConfig::Command(command) => {
            let (program, args) = command
                .run
                .split_first()
                .map(|(program, args)| (program.clone(), args.to_vec()))
                .ok_or_else(|| MetricResult::error("command.run 至少需要一个程序名"))?;
            let max_out = command.max_output_bytes.max(1);

            Ok(PersistentSource::Command(CommandMetric::new(
                program,
                args,
                command.timeout_seconds,
                max_out,
                command.reverse_lines,
                command.subtitle_lines,
            )))
        }
        SourceConfig::File(file) => Ok(PersistentSource::File(FileMetric::new(
            PathBuf::from(&file.path),
            file.first_line,
        ))),
        SourceConfig::Http(http) => {
            let max_out = http.max_output_bytes.max(1);

            Ok(PersistentSource::Http(HttpMetric::new(
                http.url.clone(),
                http.method.clone(),
                http.headers.clone(),
                http.body.clone(),
                http.timeout_seconds,
                http.parser.clone(),
                max_out,
            )))
        }
        SourceConfig::Text(value) => Ok(PersistentSource::Static(MetricResult {
            value: CardValue::Text(value.clone()),
            subtitle: None,
            tooltip: None,
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        })),
        SourceConfig::Builtin(_) => {
            Err(MetricResult::error("内置数据源不能作为通用持久数据源构建"))
        }
    }
}

fn task_policy(card: &CardConfig) -> TaskPolicy {
    let source_type = card.source.as_ref().map(SourceConfig::kind);
    let metric = card
        .source
        .as_ref()
        .and_then(SourceConfig::builtin_metric)
        .unwrap_or_default();
    let workload = match card.runtime.workload {
        CardWorkload::Live => Workload::Live,
        CardWorkload::Normal => Workload::Normal,
        CardWorkload::Expensive => Workload::Expensive,
        CardWorkload::Event => Workload::Event,
        CardWorkload::Auto => match source_type {
            Some(SourceKind::Command | SourceKind::Http) => Workload::Expensive,
            Some(SourceKind::File | SourceKind::Text) => Workload::Event,
            Some(SourceKind::Builtin) if builtin_is_event_driven(metric) => Workload::Event,
            Some(SourceKind::Builtin) if metric == "network_traffic" => Workload::Live,
            Some(SourceKind::Builtin) => Workload::Normal,
            _ => Workload::Normal,
        },
    };
    let behavior = |value| match value {
        CardWorkBehavior::Inherit => WorkBehavior::Inherit,
        CardWorkBehavior::Keep => WorkBehavior::Keep,
        CardWorkBehavior::Throttle => WorkBehavior::Throttle,
        CardWorkBehavior::Pause => WorkBehavior::Pause,
    };
    TaskPolicy {
        workload,
        inactive_behavior: behavior(card.runtime.inactive_behavior),
        idle_behavior: behavior(card.runtime.idle_behavior),
        inactive_interval_secs: card.runtime.inactive_interval_seconds,
        idle_interval_secs: card.runtime.idle_interval_seconds,
        minimum_interval_secs: card.runtime.minimum_interval_seconds,
        scheduled: card.schedule.is_some(),
    }
}

fn setting_switch_row(title: &str, description: &str, active: bool) -> gtk::Box {
    let row = GtkBox::new(Orientation::Horizontal, 14);
    row.set_hexpand(true);
    row.set_overflow(gtk::Overflow::Hidden);
    row.add_css_class("card");
    row.add_css_class("pulsedeck-card");
    row.add_css_class("settings-card-row");
    let labels = GtkBox::new(Orientation::Vertical, 2);
    labels.set_hexpand(true);
    labels.set_size_request(1, -1);
    let name = Label::new(Some(title));
    name.set_halign(Align::Start);
    name.add_css_class("settings-name");
    labels.append(&name);
    let desc = Label::new(Some(description));
    desc.set_halign(Align::Start);
    desc.set_wrap(true);
    desc.add_css_class("settings-desc");
    labels.append(&desc);
    row.append(&labels);
    let switch = gtk::Switch::new();
    switch.set_active(active);
    switch.set_valign(Align::Center);
    row.append(&switch);
    row
}

fn setting_spin_row(
    title: &str,
    description: &str,
    value: u64,
    minimum: u64,
    maximum: u64,
) -> (gtk::Box, gtk::SpinButton) {
    let row = GtkBox::new(Orientation::Horizontal, 14);
    row.set_hexpand(true);
    row.set_overflow(gtk::Overflow::Hidden);
    row.add_css_class("card");
    row.add_css_class("pulsedeck-card");
    row.add_css_class("settings-card-row");
    let labels = GtkBox::new(Orientation::Vertical, 2);
    labels.set_hexpand(true);
    labels.set_size_request(1, -1);
    let name = Label::new(Some(title));
    name.set_halign(Align::Start);
    name.add_css_class("settings-name");
    labels.append(&name);
    let desc = Label::new(Some(description));
    desc.set_halign(Align::Start);
    desc.set_wrap(true);
    desc.add_css_class("settings-desc");
    labels.append(&desc);
    row.append(&labels);
    let adjustment =
        gtk::Adjustment::new(value as f64, minimum as f64, maximum as f64, 1.0, 10.0, 0.0);
    let spin = gtk::SpinButton::new(Some(&adjustment), 1.0, 0);
    spin.set_valign(Align::Center);
    row.append(&spin);
    (row, spin)
}

fn setting_status_row(title: &str, value: &str) -> gtk::Box {
    let row = GtkBox::new(Orientation::Vertical, 4);
    row.set_hexpand(true);
    row.set_overflow(gtk::Overflow::Hidden);
    row.add_css_class("card");
    row.add_css_class("pulsedeck-card");
    let name = Label::new(Some(title));
    name.set_halign(Align::Start);
    name.add_css_class("settings-name");
    row.append(&name);
    let value = Label::new(Some(value));
    value.set_halign(Align::Start);
    value.set_xalign(0.0);
    value.set_hexpand(true);
    value.set_size_request(1, -1);
    value.set_max_width_chars(48);
    value.set_wrap(true);
    value.set_selectable(true);
    value.add_css_class("settings-desc");
    row.append(&value);
    row
}

fn setting_dropdown_row(
    title: &str,
    description: &str,
    values: &[&str],
    selected: u32,
) -> (gtk::Box, gtk::DropDown) {
    let row = GtkBox::new(Orientation::Horizontal, 14);
    row.set_hexpand(true);
    row.set_overflow(gtk::Overflow::Hidden);
    row.add_css_class("card");
    row.add_css_class("pulsedeck-card");
    row.add_css_class("settings-card-row");
    let labels = GtkBox::new(Orientation::Vertical, 2);
    labels.set_hexpand(true);
    labels.set_size_request(1, -1);
    let name = Label::new(Some(title));
    name.set_halign(Align::Start);
    name.add_css_class("settings-name");
    labels.append(&name);
    let desc = Label::new(Some(description));
    desc.set_halign(Align::Start);
    desc.set_wrap(true);
    desc.add_css_class("settings-desc");
    labels.append(&desc);
    row.append(&labels);
    let dropdown = gtk::DropDown::from_strings(values);
    dropdown.set_selected(selected);
    row.append(&dropdown);
    (row, dropdown)
}

fn update_idle_clock(time: &gtk::Label, status: &gtk::Label) {
    let now = chrono::Local::now();
    time.set_text(&now.format("%H:%M").to_string());
    // Discrete five-minute shifts distribute static AMOLED pixels without a
    // continuously running animation.
    let slot = ((now.timestamp() / 300).rem_euclid(5)) as i32;
    let horizontal = [0, 8, -8, 4, -4][slot as usize];
    let vertical = [0, -6, 6, 3, -3][slot as usize];
    time.set_margin_start(horizontal.max(0) as i32);
    time.set_margin_end((-horizontal).max(0) as i32);
    time.set_margin_top(vertical.max(0) as i32);
    status.set_margin_bottom((-vertical).max(0) as i32);
}

fn metric_result_collection_health(result: &MetricResult) -> CollectionHealth {
    CollectionHealth::from_collector(result)
}

fn results_equivalent(
    prev: &MetricResult,
    curr: &MetricResult,
    display: Option<&DisplayConfig>,
) -> bool {
    if prev.state != curr.state || prev.cached != curr.cached || prev.metadata != curr.metadata {
        return false;
    }

    let threshold = display.and_then(|d| d.minimum_change).unwrap_or(0.0);

    if threshold > 0.0 {
        if let (
            CardValue::Number {
                value: pv,
                unit: pu,
                decimals: pdecimals,
            },
            CardValue::Number {
                value: cv,
                unit: cu,
                decimals: cdecimals,
            },
        ) = (&prev.value, &curr.value)
        {
            if pu == cu
                && pdecimals == cdecimals
                && (cv - pv).abs() < threshold
                && prev.subtitle == curr.subtitle
                && prev.tooltip == curr.tooltip
            {
                return true;
            }
        }

        if let (CardValue::Percentage(pp), CardValue::Percentage(pc)) = (&prev.value, &curr.value) {
            let diff = (pc - pp).abs();
            if diff < threshold && prev.subtitle == curr.subtitle && prev.tooltip == curr.tooltip {
                return true;
            }
        }
    }

    prev.value == curr.value && prev.subtitle == curr.subtitle && prev.tooltip == curr.tooltip
}

fn apply_metric_result(
    card: &mut crate::ui::metric_card::MetricCard,
    result: &MetricResult,
    display: Option<&DisplayConfig>,
) {
    let state = match result.state {
        MetricState::Normal => CardState::Normal,
        MetricState::Loading => CardState::Loading,
        MetricState::Unavailable => CardState::Unavailable,
        MetricState::Error => CardState::Error,
        MetricState::Stale => CardState::Cached,
    };

    card.set_customization(display);
    let model = CardModel {
        id: String::new(),
        title: String::new(),
        subtitle: result.subtitle.clone(),
        icon: None,
        renderer: card.renderer_kind,
        state,
        value: result.value.clone(),
        tooltip: result.tooltip.clone(),
        cached: result.cached,
        columns_after: card.model.as_ref().and_then(|m| m.columns_after),
        columns: card.model.as_ref().and_then(|m| m.columns),
    };

    card.set_refresh_pending(false);
    card.set_value_level(None);
    card.set_model(&model);
    if let Some(level) = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("value_level"))
        .and_then(serde_json::Value::as_str)
    {
        card.set_value_level(Some(level));
    }
}

fn current_action_config(
    config: &Rc<RefCell<ConfigManager>>,
    action_id: &str,
) -> Option<crate::core::config::ActionConfig> {
    config
        .borrow()
        .config()
        .actions
        .iter()
        .find(|action| action.id == action_id)
        .cloned()
}

fn current_card_action(
    config: &Rc<RefCell<ConfigManager>>,
    card_id: &str,
) -> Option<crate::core::config::ActionConfig> {
    let config = config.borrow();
    let action_id = config
        .config()
        .cards
        .iter()
        .find(|card| card.id == card_id)
        .and_then(|card| card.click_action.as_deref())?;
    config
        .config()
        .actions
        .iter()
        .find(|action| action.id == action_id)
        .cloned()
}

fn execute_card_action(
    config: &Rc<RefCell<ConfigManager>>,
    card_id: &str,
    action_inbox: &Arc<LatestInbox<ActionUpdate>>,
    handle: &tokio::runtime::Handle,
    shutdown: Arc<AtomicBool>,
    result_card_id: Option<String>,
    set_running: Option<&Rc<dyn Fn(bool)>>,
) {
    let Some(action_cfg) = current_card_action(config, card_id) else {
        tracing::warn!(card = %card_id, "card click action is no longer configured");
        return;
    };
    if let Some(set_running) = set_running {
        set_running(true);
    }
    execute_action_async(
        action_cfg,
        action_inbox.clone(),
        handle.clone(),
        config.clone(),
        shutdown,
        result_card_id,
    );
}

fn bind_metric_action(
    metric_card: &crate::ui::metric_card::MetricCard,
    card_id: &str,
    config: Rc<RefCell<ConfigManager>>,
    action_inbox: Arc<LatestInbox<ActionUpdate>>,
    handle: tokio::runtime::Handle,
    runtime: RuntimeHandle,
    shutdown: Arc<AtomicBool>,
) {
    let has_click_action = config
        .borrow()
        .config()
        .cards
        .iter()
        .find(|card| card.id == card_id)
        .and_then(|card| card.click_action.as_ref())
        .is_some();
    if !has_click_action {
        return;
    }

    metric_card.set_action_enabled(true);
    let action_controls = match &metric_card.render_widgets {
        crate::ui::metric_card::RenderWidgets::Action(widgets) => Some(widgets.clone()),
        _ => None,
    };
    let set_running = action_controls.as_ref().map(|controls| {
        let controls = controls.clone();
        Rc::new(move |running| controls.set_running(running)) as Rc<dyn Fn(bool)>
    });
    if let Some(controls) = action_controls {
        let config = config.clone();
        let action_inbox = action_inbox.clone();
        let handle = handle.clone();
        let shutdown = shutdown.clone();
        let runtime = runtime.clone();
        let card_id = card_id.to_owned();
        let set_running = set_running.clone();
        controls.button.connect_clicked(move |button| {
            runtime.report_user_activity(UserActivity::Click);
            let Some(action) = current_card_action(&config, &card_id) else {
                tracing::warn!(card = %card_id, "card click action is no longer configured");
                return;
            };
            let (confirm_title, confirm_detail) = action_confirmation_text(&action);
            let run = {
                let config = config.clone();
                let action_inbox = action_inbox.clone();
                let handle = handle.clone();
                let shutdown = shutdown.clone();
                let card_id = card_id.clone();
                let set_running = set_running.clone();
                move || {
                    execute_card_action(
                        &config,
                        &card_id,
                        &action_inbox,
                        &handle,
                        shutdown,
                        Some(card_id.clone()),
                        set_running.as_ref(),
                    );
                }
            };
            if action.confirm {
                confirm_action(
                    button.upcast_ref(),
                    &confirm_title,
                    &confirm_detail,
                    runtime.clone(),
                    run,
                );
            } else {
                run();
            }
        });
    }

    metric_card.card.add_css_class("click-action-card");
    metric_card.card.set_cursor_from_name(Some("pointer"));
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_PRIMARY);
    let config_for_click = config.clone();
    let action_inbox_for_click = action_inbox.clone();
    let handle_for_click = handle.clone();
    let shutdown_for_click = shutdown.clone();
    let runtime_for_click = runtime.clone();
    let card_id_for_click = card_id.to_owned();
    let set_running_for_click = set_running.clone();
    click.connect_released(move |gesture, presses, x, y| {
        if presses != 1 {
            return;
        }
        let Some(widget) = gesture.widget() else {
            return;
        };
        if widget
            .pick(x, y, gtk::PickFlags::DEFAULT)
            .is_some_and(|target| widget_or_ancestor_is_button(target, &widget))
        {
            return;
        }
        runtime_for_click.report_user_activity(UserActivity::Click);
        let Some(action) = current_card_action(&config_for_click, &card_id_for_click) else {
            tracing::warn!(card = %card_id_for_click, "card click action is no longer configured");
            return;
        };
        let (confirm_title, confirm_detail) = action_confirmation_text(&action);
        let run = {
            let config = config_for_click.clone();
            let action_inbox = action_inbox_for_click.clone();
            let handle = handle_for_click.clone();
            let shutdown = shutdown_for_click.clone();
            let card_id = card_id_for_click.clone();
            let set_running = set_running_for_click.clone();
            move || {
                execute_card_action(
                    &config,
                    &card_id,
                    &action_inbox,
                    &handle,
                    shutdown,
                    Some(card_id.clone()),
                    set_running.as_ref(),
                );
            }
        };
        if action.confirm {
            confirm_action(
                &widget,
                &confirm_title,
                &confirm_detail,
                runtime_for_click.clone(),
                run,
            );
        } else {
            run();
        }
    });
    metric_card.card.add_controller(click);
}

fn execute_action_async(
    action_cfg: crate::core::config::ActionConfig,
    inbox: Arc<LatestInbox<ActionUpdate>>,
    handle: tokio::runtime::Handle,
    config: Rc<RefCell<ConfigManager>>,
    shutdown: Arc<AtomicBool>,
    result_card_id: Option<String>,
) {
    static NEXT_ACTION_INVOCATION: AtomicU64 = AtomicU64::new(1);
    let invocation_id = NEXT_ACTION_INVOCATION.fetch_add(1, Ordering::Relaxed);
    let action_id = action_cfg.id.clone();
    let command_parts = action_cfg.command.clone().unwrap_or_default();
    let timeout = action_cfg.timeout;
    // Read the global cap at invocation time so an app-only hot reload also
    // constrains already-created action controls.
    let global_max_output = config.borrow().config().app.max_output_bytes;
    let max_output = action_cfg
        .max_output_bytes
        .unwrap_or(global_max_output)
        .min(global_max_output)
        .max(1);

    if command_parts.is_empty() {
        let result = ActionResult {
            success: false,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: -1,
            message: "未配置命令".to_string(),
        };
        publish_action(
            &inbox,
            ActionUpdate {
                action_id,
                invocation_id,
                result_card_id,
                result,
            },
        );
        return;
    }

    let program = command_parts[0].clone();
    let args: Vec<String> = command_parts.iter().skip(1).cloned().collect();

    handle.spawn(async move {
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        let output = crate::execution::subprocess::run_command_with_shutdown(
            &program,
            &args,
            timeout,
            max_output,
            shutdown.clone(),
        )
        .await;

        if shutdown.load(Ordering::Acquire) {
            return;
        }

        let result = match output {
            Ok(o) => ActionResult {
                success: o.success,
                stdout: o.stdout,
                stderr: o.stderr,
                exit_code: o.exit_code,
                message: if o.success {
                    "命令执行成功".to_string()
                } else {
                    format!("命令退出码: {}", o.exit_code)
                },
            },
            Err(e) => ActionResult {
                success: false,
                stdout: String::new(),
                stderr: e.clone(),
                exit_code: -1,
                message: format!("执行失败: {}", e),
            },
        };

        publish_action(
            &inbox,
            ActionUpdate {
                action_id,
                invocation_id,
                result_card_id,
                result,
            },
        );
    });
}

fn action_confirmation_text(action: &crate::core::config::ActionConfig) -> (String, String) {
    let title = action
        .confirm_title
        .clone()
        .unwrap_or_else(|| format!("确认执行「{}」？", action.name));
    let detail = action
        .confirm_detail
        .clone()
        .or_else(|| action.description.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "该操作已配置为需要确认。".to_string());
    (title, detail)
}

fn confirm_action(
    parent: &gtk::Widget,
    title: &str,
    detail: &str,
    runtime: RuntimeHandle,
    run: impl FnOnce() + 'static,
) {
    let Some(window) = parent
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok())
    else {
        return;
    };
    let dialog = gtk::AlertDialog::builder()
        .message(title)
        .detail(detail)
        .buttons(["取消", "执行"])
        .cancel_button(0)
        .default_button(1)
        .build();
    let interaction = runtime.begin_interaction(Duration::from_secs(300));
    glib::MainContext::default().spawn_local(async move {
        let response = dialog.choose_future(Some(&window)).await;
        drop(interaction);
        runtime.report_user_activity(UserActivity::Dialog);
        if response == Ok(1) {
            run();
        }
    });
}

fn widget_or_ancestor_is_button(mut widget: gtk::Widget, boundary: &gtk::Widget) -> bool {
    loop {
        if widget.is::<gtk::Button>() {
            return true;
        }
        if widget == *boundary {
            return false;
        }
        let Some(parent) = widget.parent() else {
            return false;
        };
        widget = parent;
    }
}

fn show_action_result_dialog(
    parent: &gtk::Box,
    action_id: &str,
    result: &ActionResult,
    runtime: RuntimeHandle,
) {
    let root = parent.root().and_then(|r| r.downcast::<gtk::Window>().ok());
    let window = match root {
        Some(w) => w,
        None => return,
    };

    let detail = if result.success {
        format!(
            "输出:\n{}",
            if result.stdout.is_empty() {
                "(无输出)"
            } else {
                &result.stdout
            }
        )
    } else {
        format!(
            "{}\n\nstderr:\n{}",
            result.message,
            if result.stderr.is_empty() {
                "(无)"
            } else {
                &result.stderr
            }
        )
    };

    let labels: &[&str] = &["确定"];
    let dialog = gtk::AlertDialog::builder()
        .message(&format!("操作结果: {}", action_id))
        .detail(&detail)
        .buttons(labels)
        .build();

    glib::MainContext::default().spawn_local(async move {
        let _ = dialog.choose_future(Some(&window)).await;
        runtime.report_user_activity(UserActivity::Dialog);
    });
}

fn do_reload_config(config: &Rc<RefCell<ConfigManager>>) -> bool {
    let mut cfg = config.borrow_mut();
    match cfg.load() {
        Ok(()) => {
            crate::core::error_limiter::recovered("config:reload");
            tracing::info!("config hot-reloaded from {:?}", cfg.path());
            true
        }
        Err(e) => {
            crate::core::error_limiter::warn(
                "config:reload",
                format!("config reload failed (keeping current): {e}"),
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(value: CardValue) -> MetricResult {
        MetricResult {
            value,
            subtitle: Some("same".into()),
            tooltip: Some("same".into()),
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        }
    }

    #[test]
    fn collection_health_distinguishes_collector_fallback_from_cache_hits() {
        let mut stale = result(CardValue::Text("cached-looking".into()));
        stale.state = MetricState::Stale;
        stale.cached = true;
        assert_eq!(
            metric_result_collection_health(&stale),
            CollectionHealth::Failed
        );
        assert_eq!(CollectionHealth::cache_hit(), CollectionHealth::Healthy);
        assert_eq!(
            metric_result_collection_health(&result(CardValue::Text("ok".into()))),
            CollectionHealth::Healthy
        );
        assert_eq!(
            metric_result_collection_health(&MetricResult::error("failed")),
            CollectionHealth::Failed
        );
    }

    #[test]
    fn latest_inbox_replaces_pending_result_for_same_key() {
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let inbox = LatestInbox::new(wake_tx);
        inbox.publish("cpu".into(), 1_u32);
        inbox.publish("cpu".into(), 2_u32);
        assert_eq!(wake_rx.try_recv().unwrap(), ());
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox.take_batch(24), vec![2]);
        assert!(!inbox.has_pending());
    }

    #[test]
    fn equivalent_results_skip_identical_gtk_updates() {
        let current = result(CardValue::Text("ok".into()));
        assert!(results_equivalent(&current, &current, None));
    }

    #[test]
    fn numeric_minimum_change_requires_derived_text_to_match() {
        let mut previous = result(CardValue::Number {
            value: 10.0,
            unit: Some("W".into()),
            decimals: 1,
        });
        let mut current = previous.clone();
        current.value = CardValue::Number {
            value: 10.2,
            unit: Some("W".into()),
            decimals: 1,
        };
        let display = DisplayConfig {
            minimum_change: Some(1.0),
            columns_after: None,
            columns: None,
            card_width: None,
            card_height: None,
            fixed_size: None,
            logo_svg: None,
            background_svg: None,
            colors: Default::default(),
            states: Vec::new(),
            transition: None,
        };
        assert!(results_equivalent(&previous, &current, Some(&display)));
        current.subtitle = Some("changed derived status".into());
        assert!(!results_equivalent(&previous, &current, Some(&display)));
        previous.metadata = Some(serde_json::json!({"level":"normal"}));
        assert!(!results_equivalent(&previous, &current, Some(&display)));
    }

    #[test]
    fn signal_backed_builtins_do_not_keep_independent_card_pollers() {
        let cards = default_builtin_cards();
        for id in ["battery", "battery-temp", "network-status"] {
            let card = cards.iter().find(|card| card.id == id).unwrap();
            assert_eq!(task_policy(card).workload, Workload::Event);
        }
        let power = cards.iter().find(|card| card.id == "power").unwrap();
        assert_eq!(task_policy(power).workload, Workload::Normal);
    }

    #[test]
    fn source_keys_share_descriptors_without_card_ids() {
        let source = Some(SourceConfig::Builtin("network".into()));
        assert_eq!(source_key_for(source.as_ref()), "builtin:network");
        let left = source_key_for(Some(&SourceConfig::Text("same".into())));
        let right = source_key_for(Some(&SourceConfig::Text("same".into())));
        assert_eq!(left, right);
        assert!(!left.contains("card"));
    }

    #[test]
    fn http_source_keys_are_stable_across_header_insertion_order() {
        let mut first_headers = std::collections::HashMap::new();
        first_headers.insert("Accept".into(), "application/json".into());
        first_headers.insert("X-Token".into(), "secret".into());
        let mut second_headers = std::collections::HashMap::new();
        second_headers.insert("X-Token".into(), "secret".into());
        second_headers.insert("Accept".into(), "application/json".into());
        let source = |headers| {
            SourceConfig::Http(crate::core::config::HttpSourceConfig {
                url: "https://example.invalid/status".into(),
                method: Some("GET".into()),
                headers: Some(headers),
                body: None,
                timeout_seconds: 5,
                max_output_bytes: 1024,
                parser: None,
            })
        };
        assert_eq!(
            source_key_for(Some(&source(first_headers))),
            source_key_for(Some(&source(second_headers)))
        );
    }

    #[test]
    fn file_event_matches_atomic_replacement_paths() {
        let target = std::path::Path::new("/tmp/pulsedeck-state");
        assert!(file_event_matches_target(Some(target), None, target));
        assert!(file_event_matches_target(
            Some(std::path::Path::new("/tmp/temporary")),
            Some(target),
            target
        ));
        assert!(!file_event_matches_target(
            Some(std::path::Path::new("/tmp/other")),
            None,
            target
        ));
        let relative = std::path::Path::new("relative-file");
        let absolute = std::env::current_dir().unwrap().join(relative);
        assert!(file_event_matches_target(Some(&absolute), None, relative));
    }

    #[test]
    fn generated_plugin_pages_layer_over_the_default_page_registry() {
        let mut config = AppConfig::default();
        config.pages.push(crate::core::config::PageConfig {
            id: "plugin-page".into(),
            title: "Plugin".into(),
            icon: None,
            order: 40,
            kind: None,
            plugin: None,
        });
        let pages = effective_pages(&config, true);
        for id in ["monitor", "actions", "settings", "plugin-page"] {
            assert!(pages.iter().any(|page| page.id == id));
        }
    }

    #[test]
    fn empty_configuration_uses_one_parsed_default_card_registry() {
        let config = AppConfig::default();
        assert_eq!(
            effective_card_config(&config, "cpu", true)
                .and_then(|card| card.source)
                .and_then(|source| source.builtin_metric().map(str::to_owned)),
            Some("cpu".into())
        );
        let mut configured = config;
        configured.cards.push(CardConfig {
            id: "disabled".into(),
            title: "Disabled".into(),
            page: "monitor".into(),
            order: 1,
            renderer: crate::model::card_model::RendererKind::Value,
            refresh_interval: 30,
            enabled: false,
            icon: None,
            description: None,
            source: Some(SourceConfig::Builtin("uptime".into())),
            display: None,
            cache_ttl_seconds: None,
            schedule: None,
            click_action: None,
            kind: None,
            plugin: None,
            runtime: crate::core::config::CardRuntimeConfig::default(),
        });
        assert!(effective_card_config(&configured, "cpu", false).is_none());
    }

    #[test]
    fn stateful_source_nodes_include_sampling_policy_without_card_identity() {
        let mut first = AppConfig::default();
        first.cards = default_builtin_cards();
        let cpu = first.cards.iter().find(|card| card.id == "cpu").unwrap();
        let mut slow = cpu.clone();
        slow.refresh_interval = 60;
        let first_key = source_node_key(&source_key_for(cpu.source.as_ref()), cpu, 1);
        let slow_key = source_node_key(&source_key_for(slow.source.as_ref()), &slow, 1);
        let renamed_key = source_node_key(&source_key_for(cpu.source.as_ref()), cpu, 2);
        assert_ne!(first_key, slow_key);
        assert_ne!(first_key, renamed_key);
        assert_eq!(
            source_key_for(cpu.source.as_ref()),
            source_key_for(cpu.source.as_ref())
        );
        assert!(!first_key.contains("different-card-id"));
    }

    #[test]
    fn removing_a_default_card_override_rebinds_the_shipped_card() {
        let default_cpu = default_builtin_cards()
            .into_iter()
            .find(|card| card.id == "cpu")
            .unwrap();
        let mut override_cpu = default_cpu.clone();
        override_cpu.title = "Override".into();

        let mut previous_raw = AppConfig::default();
        previous_raw.cards = vec![override_cpu.clone()];
        let next_raw = AppConfig::default();
        let mut previous = previous_raw.clone();
        previous.cards = effective_cards(&previous_raw, true);
        let mut next = next_raw.clone();
        next.cards = effective_cards(&next_raw, true);
        let changed = changed_card_ids(&previous, &next);
        assert!(changed.iter().any(|id| id == "cpu"));

        let scheduler = Rc::new(RefCell::new(Scheduler::new()));
        scheduler.borrow_mut().register_with_policy(
            "cpu",
            override_cpu.refresh_interval,
            &override_cpu.page,
            task_policy(&override_cpu),
        );
        let source_key = source_key_for(override_cpu.source.as_ref());
        let sampling_consumer_key = 1;
        let card_metas = Rc::new(RefCell::new(HashMap::from([(
            "cpu".into(),
            CardMeta {
                page_id: override_cpu.page.clone(),
                interval_secs: override_cpu.refresh_interval,
                source_node_key: source_node_key(&source_key, &override_cpu, sampling_consumer_key),
                source_key,
                config_epoch: 0,
                sampling_consumer_key,
            },
        )])));
        let source_nodes = Arc::new(Mutex::new(HashMap::new()));

        reconcile_scheduler_cards(
            &scheduler,
            &card_metas,
            &source_nodes,
            &previous,
            &next,
            &changed,
            1,
        );

        assert!(scheduler.borrow().generation("cpu").is_some());
        let metas = card_metas.borrow();
        let meta = metas.get("cpu").expect("default card remains registered");
        assert_eq!(meta.config_epoch, 1);
        assert_eq!(meta.source_key, source_key_for(default_cpu.source.as_ref()));
    }

    #[test]
    fn scheduled_reload_keeps_an_inflight_slot_without_duplicate_registration() {
        let mut previous = AppConfig::default();
        let mut card = default_builtin_cards()
            .into_iter()
            .find(|card| card.id == "cpu")
            .unwrap();
        card.id = "scheduled".into();
        card.schedule = Some("daily@00:00".into());
        card.refresh_interval = 60;
        previous.cards = vec![card.clone()];
        let mut next = previous.clone();
        next.cards[0].title = "Reloaded".into();

        let scheduler = Rc::new(RefCell::new(Scheduler::new()));
        scheduler.borrow_mut().register_with_policy(
            &card.id,
            card.refresh_interval,
            &card.page,
            task_policy(&card),
        );
        assert_eq!(scheduler.borrow_mut().poll(), vec![card.id.clone()]);
        scheduler.borrow_mut().mark_started(&card.id);
        let generation = scheduler.borrow().generation(&card.id).unwrap();
        let active_slot = scheduler.borrow().next_run(&card.id).unwrap();

        let card_metas = Rc::new(RefCell::new(HashMap::new()));
        card_metas.borrow_mut().insert(
            card.id.clone(),
            CardMeta {
                page_id: card.page.clone(),
                interval_secs: card.refresh_interval,
                source_key: source_key_for(card.source.as_ref()),
                source_node_key: source_node_key(&source_key_for(card.source.as_ref()), &card, 1),
                config_epoch: 0,
                sampling_consumer_key: 1,
            },
        );
        let source_nodes = Arc::new(Mutex::new(HashMap::new()));

        reconcile_scheduler_cards(
            &scheduler,
            &card_metas,
            &source_nodes,
            &previous,
            &next,
            &[card.id.clone()],
            1,
        );

        assert!(scheduler.borrow().is_running(&card.id));
        assert_eq!(scheduler.borrow().generation(&card.id), Some(generation));
        assert_eq!(scheduler.borrow().next_run(&card.id), Some(active_slot));
        assert!(scheduler.borrow_mut().poll().is_empty());
    }

    #[test]
    fn action_completion_reverse_links_all_enabled_cards() {
        let config: crate::core::config::AppConfig = toml::from_str(
            "schema_version=4\n[[cards]]\nid='two'\ntitle='Two'\npage='monitor'\nenabled=true\nclick_action='toggle'\nsource={text='two'}\n\
             [[cards]]\nid='one'\ntitle='One'\npage='monitor'\nenabled=true\nclick_action='toggle'\nsource={text='one'}\n\
             [[cards]]\nid='disabled'\ntitle='Disabled'\npage='monitor'\nenabled=false\nclick_action='toggle'\nsource={text='disabled'}\n\
             [[cards]]\nid='other'\ntitle='Other'\npage='monitor'\nenabled=true\nclick_action='other'\nsource={text='other'}\n",
        )
        .unwrap();
        assert_eq!(linked_card_ids(&config, "toggle"), ["one", "two"]);
        assert_eq!(linked_card_ids(&config, "other"), ["other"]);
    }

    #[test]
    fn action_lookup_reads_the_current_definition_after_reload() {
        let mut manager = ConfigManager::new(std::path::PathBuf::new());
        *manager.config_mut() = toml::from_str(
            "schema_version=4\n[[cards]]\nid='service'\ntitle='Service'\npage='monitor'\nclick_action='toggle'\nsource={text='state'}\n\
             [[actions]]\nid='toggle'\nname='Toggle'\npage='actions'\ncommand=['old-command']\nmax_output_bytes=100\n",
        )
        .unwrap();
        let config = Rc::new(RefCell::new(manager));
        assert_eq!(
            current_card_action(&config, "service").and_then(|action| action.command),
            Some(vec!["old-command".into()])
        );
        config.borrow_mut().config_mut().actions[0].command = Some(vec!["new-command".into()]);
        config.borrow_mut().config_mut().actions[0].max_output_bytes = Some(8);
        let current = current_card_action(&config, "service").unwrap();
        assert_eq!(current.command, Some(vec!["new-command".into()]));
        assert_eq!(current.max_output_bytes, Some(8));
    }
}

fn effective_pages(
    config: &AppConfig,
    use_default_registry: bool,
) -> Vec<crate::core::config::PageConfig> {
    if !use_default_registry {
        return config.pages.clone();
    }
    let mut pages = default_pages();
    for configured in &config.pages {
        if let Some(existing) = pages.iter_mut().find(|page| page.id == configured.id) {
            *existing = configured.clone();
        } else {
            pages.push(configured.clone());
        }
    }
    pages
}

fn default_pages() -> Vec<crate::core::config::PageConfig> {
    toml::from_str::<AppConfig>(DEFAULT_CONFIG)
        .map(|config| config.pages)
        .unwrap_or_default()
}

fn default_builtin_cards() -> Vec<CardConfig> {
    toml::from_str::<AppConfig>(DEFAULT_CONFIG)
        .map(|config| {
            config
                .cards
                .into_iter()
                .filter(|card| card.enabled)
                .collect()
        })
        .unwrap_or_default()
}

fn compact_grid_preference_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("pulsedeck/compact-grid")
}

fn load_compact_grid_preference() -> bool {
    std::fs::read_to_string(compact_grid_preference_path())
        .map(|value| value.trim() == "compact")
        .unwrap_or(false)
}

fn save_compact_grid_preference(compact: bool) -> std::io::Result<()> {
    let path = compact_grid_preference_path();
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "compact-grid preference has no parent",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, if compact { "compact\n" } else { "normal\n" })?;
    std::fs::rename(temporary, path)
}
