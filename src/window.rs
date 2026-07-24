use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use adw::prelude::AdwApplicationWindowExt;
use gtk::prelude::*;
use gtk::{Align, Box as GtkBox, Button, Label, Orientation};

use crate::core::cache;
use crate::core::config::{config_path, CardConfig, ConfigManager, DisplayConfig, SourceConfig};
use crate::core::runtime::{RuntimeHandle, RuntimeManager, RuntimeMode, UserActivity};
use crate::core::scheduler::{IdleBehavior, Scheduler, TaskClass, TaskPolicy};
use crate::metrics::builtin::create_builtin_metric;
use crate::metrics::command::CommandMetric;
use crate::metrics::file::FileMetric;
use crate::metrics::http::HttpMetric;
use crate::metrics::traits::{BuiltinMetric, MetricContext};
use crate::model::action_result::ActionResult;
use crate::model::card_model::{CardModel, CardState, CardValue};
use crate::model::metric_result::{MetricResult, MetricState};
use crate::tokio_handle;
use crate::ui::page::Page;

const APP_CSS: &str = r#"
.tab-bar-area { padding: 5px 6px; }
.tab-bar-area tab { border-radius: 10px; min-height: 32px; font-size: 13px; }
.compact-grid-button { min-width: 32px; min-height: 32px; padding: 0; }
.pulsedeck-card { padding: 10px 8px 8px; border-radius: 14px; border: 1px solid alpha(currentColor, 0.12); background: alpha(currentColor, 0.035); box-shadow: 0 2px 8px alpha(black, 0.08); }
.metric-card { }
.accent-blue   { border-left: 2px solid #3584e4; }
.accent-purple { border-left: 2px solid #9141ac; }
.accent-green  { border-left: 2px solid #33d17a; }
.accent-orange { border-left: 2px solid #e5a50a; }
.accent-teal   { border-left: 2px solid #2190a0; }
.metric-header-icon { opacity: 0.65; }
.metric-header-name { font-weight: 700; font-size: 13px; }
.metric-header-sub { font-size: 9px; opacity: 0.62; margin-top: 0px; }
.metric-value-box { margin: 5px 0 2px 0; }
.metric-value { font-size: 19px; font-weight: 800; font-feature-settings: "tnum"; }
.content-medium .metric-value { font-size: 15px; }
.content-dense .metric-value { font-size: 10px; font-weight: 650; }
.metric-value-placeholder { font-size: 11px; font-weight: 400; opacity: 0.3; }
.metric-value-warning  { color: #e5a50a; }
.metric-value-critical { color: #e01b24; }
.metric-value-good     { color: #33d17a; }
.metric-card.click-action-card { transition: background-color 120ms ease; }
.metric-card.click-action-card:hover { background-color: alpha(@accent_bg_color, 0.12); }
.metric-footer { font-size: 9px; opacity: 0.7; margin-top: 1px; }
.content-medium .metric-footer, .content-medium .metric-header-sub { font-size: 8px; }
.content-dense .metric-footer, .content-dense .metric-header-sub { font-size: 7px; }
.compact-card { padding: 6px 4px; border-radius: 10px; }
.compact-card .metric-header-name { font-size: 11px; }
.compact-card .metric-header-icon { opacity: 0; min-width: 0; min-height: 0; }
.compact-card .metric-header-sub, .compact-card .metric-footer { font-size: 8px; }
.compact-card .metric-value-box { margin: 2px 0 0 0; }
.compact-card .metric-value { font-size: 16px; }
.compact-card.content-medium .metric-value { font-size: 15px; }
.compact-card.content-dense .metric-value { font-size: 13px; }
.compact-card .action-icon { opacity: 0; min-width: 0; min-height: 0; }
.compact-card .action-desc, .compact-card .action-confirm-badge { font-size: 8px; }
.compact-card .action-name { font-size: 11px; }
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
    result: MetricResult,
    interval_secs: u64,
    next_delay: Option<Duration>,
}

struct ActionUpdate {
    action_id: String,
    result_card_id: Option<String>,
    result: ActionResult,
}

struct CardMeta {
    page_id: String,
    interval_secs: u64,
}

enum PersistentSource {
    Command(CommandMetric),
    File(FileMetric),
    Http(HttpMetric),
    Static(MetricResult),
}

impl PersistentSource {
    fn collect(&mut self, ctx: &MetricContext) -> MetricResult {
        match self {
            Self::Command(source) => source.collect_no_ctx(),
            Self::File(source) => source.collect(ctx),
            Self::Http(source) => source.collect(ctx),
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
    view_stack: adw::ViewStack,
    pages: Rc<RefCell<HashMap<String, Page>>>,
    config: Rc<RefCell<ConfigManager>>,
    scheduler: Rc<RefCell<Scheduler>>,
    handle: tokio::runtime::Handle,
    metric_ctx: Arc<MetricContext>,
    previous_results: Rc<RefCell<HashMap<String, MetricResult>>>,
    builtin_metrics: Arc<Mutex<HashMap<String, Arc<Mutex<BuiltinMetric>>>>>,
    persistent_sources: Arc<Mutex<HashMap<String, Arc<Mutex<PersistentSource>>>>>,
    card_metas: Rc<RefCell<HashMap<String, CardMeta>>>,
    current_page_id: Rc<RefCell<String>>,
    metric_tx: async_channel::Sender<MetricUpdate>,
    action_tx: async_channel::Sender<ActionUpdate>,
    reload_guard: Rc<ConfigReloadGuard>,
    config_monitor: Option<gio::FileMonitor>,
    scheduler_wake: async_channel::Sender<()>,
    compact_grid: Rc<Cell<bool>>,
    runtime: RuntimeHandle,
    dashboard_content: gtk::Box,
    dim_layer: gtk::Box,
    idle_status: gtk::Label,
    idle_time: gtk::Label,
    _power_monitor: Rc<crate::core::power_supply::PowerSupplyMonitor>,
    file_monitors: RefCell<Vec<gio::FileMonitor>>,
    _network_monitor: gio::NetworkMonitor,
}

impl MonitorWindow {
    pub fn new(app: &adw::Application, config: ConfigManager) -> Self {
        let window = adw::ApplicationWindow::new(app);
        window.set_default_size(420, 720);
        window.set_title(Some(&config.config().app.title));
        let runtime_manager = RuntimeManager::new(config.config().runtime.clone());
        let runtime = runtime_manager.handle();

        let provider = gtk::CssProvider::new();
        provider.load_from_data(APP_CSS);
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().unwrap(),
            &provider,
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
        let http_client = reqwest::Client::new();

        let battery_root = PathBuf::from("/sys/class/power_supply");
        let procfs_root = PathBuf::from("/proc");
        let metric_ctx = Arc::new(MetricContext::new(
            handle.clone(),
            http_client.clone(),
            battery_root,
            procfs_root,
        ));
        let power_monitor = crate::core::power_supply::PowerSupplyMonitor::start(
            runtime.clone(),
            PathBuf::from("/sys/class/power_supply"),
            PathBuf::from("/sys/class/thermal"),
        );

        let (metric_tx, metric_rx) = async_channel::unbounded::<MetricUpdate>();
        let (action_tx, action_rx) = async_channel::unbounded::<ActionUpdate>();
        let (scheduler_wake, scheduler_wake_rx) = async_channel::bounded::<()>(1);

        let previous_results: Rc<RefCell<HashMap<String, MetricResult>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let builtin_metrics: Arc<Mutex<HashMap<String, Arc<Mutex<BuiltinMetric>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let persistent_sources = Arc::new(Mutex::new(HashMap::new()));
        let card_metas: Rc<RefCell<HashMap<String, CardMeta>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let current_page_id = Rc::new(RefCell::new(String::new()));
        let reload_guard = ConfigReloadGuard::new(500);
        let network_monitor = gio::NetworkMonitor::default();

        let mut win = Self {
            window,
            view_stack,
            pages: pages.clone(),
            config: config_ref.clone(),
            scheduler: scheduler.clone(),
            handle,
            metric_ctx,
            previous_results,
            builtin_metrics,
            persistent_sources,
            card_metas,
            current_page_id: current_page_id.clone(),
            metric_tx,
            action_tx,
            reload_guard: reload_guard.clone(),
            config_monitor: None,
            scheduler_wake,
            compact_grid: compact_preference,
            runtime,
            dashboard_content: content,
            dim_layer,
            idle_status,
            idle_time,
            _power_monitor: power_monitor,
            file_monitors: RefCell::new(Vec::new()),
            _network_monitor: network_monitor.clone(),
        };

        win.setup_pages();
        win.setup_screen_inhibit(app);
        win.setup_lifecycle();
        win.setup_config_monitor();
        win.setup_network_monitor(&network_monitor);
        win.start_scheduler_polling(scheduler_wake_rx);
        win.start_metric_receiver(metric_rx);
        win.start_action_receiver(action_rx);

        win
    }

    fn setup_screen_inhibit(&mut self, app: &adw::Application) {
        let application = app.clone();
        let inhibit_cookie: Rc<RefCell<Option<u32>>> = Rc::new(RefCell::new(None));
        let runtime = self.runtime.clone();
        runtime.set_foreground(self.window.is_mapped());
        self.window.connect_map({
            let runtime = runtime.clone();
            move |_| runtime.set_foreground(true)
        });
        self.window.connect_unmap({
            let runtime = runtime.clone();
            move |_| runtime.set_foreground(false)
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
                let keep_screen_on = runtime_config.keep_screen_on;
                if let crate::core::runtime::CodexPhase::Attention { task_id, event_id } =
                    &snapshot.codex_phase
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
                if snapshot.foreground && keep_screen_on {
                    if inhibit_cookie.borrow().is_none() {
                        let flags = gtk::ApplicationInhibitFlags::IDLE
                            | gtk::ApplicationInhibitFlags::SUSPEND;
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

                let idle = snapshot.mode == RuntimeMode::ForegroundIdle;
                if idle {
                    let runtime_config = runtime.config();
                    let minimal = runtime_config.idle_display == "minimal";
                    let brightness = runtime_config.idle_visual_brightness_percent.min(100) as f64;
                    dashboard_content.set_child_visible(!minimal);
                    idle_status.set_visible(minimal);
                    idle_time.set_visible(minimal);
                    if minimal {
                        idle_status.set_text(&format!(
                            "{:?}\n供电 {:?} · 温度 {:?}\n{}",
                            snapshot.codex_phase,
                            snapshot.power_verdict,
                            snapshot.thermal_verdict,
                            snapshot.reason
                        ));
                        update_idle_clock(&idle_time, &idle_status);
                        if clock_source.borrow().is_none() {
                            let time = idle_time.clone();
                            let status = idle_status.clone();
                            let mode = runtime.clone();
                            let holder = clock_source.clone();
                            let source =
                                glib::timeout_add_local(Duration::from_secs(60), move || {
                                    if mode.snapshot().mode != RuntimeMode::ForegroundIdle
                                        || mode.config().idle_display != "minimal"
                                    {
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

        let mut cards = app_config.cards.clone();
        if cards.is_empty() {
            cards = default_builtin_cards();
        } else {
            // The external configuration is authoritative. In particular, a
            // disabled card must not be silently recreated from Rust defaults.
            cards.retain(|card| card.enabled);
        }

        cards.sort_by_key(|c| c.order);

        let actions = app_config.actions.clone();

        (cards, actions)
    }

    fn sorted_pages(&self) -> Vec<crate::core::config::PageConfig> {
        let cfg = self.config.borrow();
        let app_config = cfg.config();

        let mut pages_list = if app_config.pages.is_empty() {
            vec![
                crate::core::config::PageConfig {
                    id: "monitor".into(),
                    title: "监控".into(),
                    icon: Some("computer-symbolic".into()),
                    order: 10,
                    kind: None,
                    plugin: None,
                },
                crate::core::config::PageConfig {
                    id: "actions".into(),
                    title: "操作".into(),
                    icon: Some("system-run-symbolic".into()),
                    order: 20,
                    kind: None,
                    plugin: None,
                },
                crate::core::config::PageConfig {
                    id: "settings".into(),
                    title: "设置".into(),
                    icon: Some("preferences-system-symbolic".into()),
                    order: 30,
                    kind: None,
                    plugin: None,
                },
            ]
        } else {
            app_config.pages.clone()
        };

        pages_list.sort_by_key(|p| p.order);
        pages_list
    }

    fn setup_pages(&mut self) {
        let pages_list = self.sorted_pages();
        let (cards, actions) = self.drain_cards();
        self.setup_file_monitors(&cards);

        let mut page_ids = Vec::new();

        self.pages.borrow_mut().clear();
        self.card_metas.borrow_mut().clear();
        self.previous_results.borrow_mut().clear();
        self.builtin_metrics.lock().unwrap().clear();
        self.persistent_sources.lock().unwrap().clear();

        let plugin_context = crate::plugins::PluginContext {
            handle: self.handle.clone(),
            presentation: None,
            runtime: self.runtime.clone(),
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
    }

    fn setup_file_monitors(&self, cards: &[CardConfig]) {
        self.file_monitors.borrow_mut().clear();
        for card in cards {
            let Some(source) = card.source.as_ref() else {
                continue;
            };
            if source.source_type != "file" {
                continue;
            }
            let Some(path) = source.path.as_ref() else {
                continue;
            };
            let Ok(monitor) = gio::File::for_path(path)
                .monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
            else {
                continue;
            };
            let scheduler = self.scheduler.clone();
            let wake = self.scheduler_wake.clone();
            let card_id = card.id.clone();
            monitor.connect_changed(move |_, _, _, event| {
                if matches!(
                    event,
                    gio::FileMonitorEvent::Changed
                        | gio::FileMonitorEvent::ChangesDoneHint
                        | gio::FileMonitorEvent::Created
                        | gio::FileMonitorEvent::MovedIn
                ) {
                    scheduler.borrow_mut().request_now(&card_id);
                    let _ = wake.try_send(());
                }
            });
            self.file_monitors.borrow_mut().push(monitor);
        }
    }

    fn setup_network_monitor(&self, monitor: &gio::NetworkMonitor) {
        let scheduler = self.scheduler.clone();
        let wake = self.scheduler_wake.clone();
        let config = self.config.clone();
        monitor.connect_network_changed(move |_, _| {
            for card in &config.borrow().config().cards {
                if card.source.as_ref().is_some_and(|source| {
                    source.source_type == "builtin" && source.metric.as_deref() == Some("network")
                }) {
                    scheduler.borrow_mut().request_now(&card.id);
                }
            }
            let _ = wake.try_send(());
        });
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
                };
                match crate::plugins::build_card(&context, card_cfg) {
                    Ok(Some(widget)) => {
                        page.add_plugin_card(
                            &card_cfg.id,
                            &widget,
                            card_cfg.display.as_ref(),
                            presentation,
                        );
                        let pages = self.pages.clone();
                        let page_id = page_id.to_string();
                        let card_id = card_cfg.id.clone();
                        glib::MainContext::default().spawn_local(async move {
                            while let Ok(request) = presentation_rx.recv().await {
                                if let Some(page) = pages.borrow_mut().get_mut(&page_id) {
                                    page.set_plugin_card_presentation(&card_id, request);
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
                metric_card.refresh_btn.connect_clicked(move |_| {
                    scheduler.borrow_mut().request_now(&card_id);
                    let _ = wake.try_send(());
                });

                if let Some(action_id) = card_cfg.click_action.as_deref() {
                    if let Some(action_cfg) =
                        all_actions.iter().find(|action| action.id == action_id)
                    {
                        metric_card.card.add_css_class("click-action-card");
                        metric_card.card.set_cursor_from_name(Some("pointer"));
                        let click = gtk::GestureClick::new();
                        click.set_button(gtk::gdk::BUTTON_PRIMARY);
                        let action_cfg = action_cfg.clone();
                        let action_tx = self.action_tx.clone();
                        let handle = self.handle.clone();
                        let result_card_id = card_cfg.id.clone();
                        let (confirm_title, confirm_detail) = action_confirmation_text(&action_cfg);
                        let global_max_output = self.config.borrow().config().app.max_output_bytes;
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
                            let run = {
                                let action_cfg = action_cfg.clone();
                                let action_tx = action_tx.clone();
                                let handle = handle.clone();
                                let result_card_id = result_card_id.clone();
                                move || {
                                    execute_action_async(
                                        action_cfg,
                                        action_tx,
                                        handle,
                                        global_max_output,
                                        Some(result_card_id),
                                    );
                                }
                            };
                            if action_cfg.confirm {
                                confirm_action(&widget, &confirm_title, &confirm_detail, run);
                            } else {
                                run();
                            }
                        });
                        metric_card.card.add_controller(click);
                    } else {
                        tracing::warn!(
                            card = %card_cfg.id,
                            action = %action_id,
                            "card click action not found"
                        );
                    }
                }
            }

            self.card_metas.borrow_mut().insert(
                card_cfg.id.clone(),
                CardMeta {
                    page_id: page_id.to_string(),
                    interval_secs: card_cfg.refresh_interval,
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
            let action_cfg_clone = (*action_cfg).clone();
            let action_tx = self.action_tx.clone();
            let handle = self.handle.clone();
            let global_max_output = self.config.borrow().config().app.max_output_bytes;
            let (confirm_title, confirm_detail) = action_confirmation_text(action_cfg);

            page.add_action_card(
                &action_id,
                &action_cfg.name,
                action_cfg.description.as_deref().unwrap_or(""),
                icon,
                action_cfg.confirm,
                &confirm_title,
                &confirm_detail,
                move |_id| {
                    let cfg = action_cfg_clone.clone();
                    let tx = action_tx.clone();
                    let h = handle.clone();
                    execute_action_async(cfg, tx, h, global_max_output, None);
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

        let status_label = Label::new(Some("卡片开关会立即生效，并自动保存到配置文件"));
        status_label.set_wrap(true);
        status_label.set_xalign(0.0);
        status_label.set_hexpand(true);
        status_label.set_size_request(1, -1);
        status_label.add_css_class("status-text");
        status_card.append(&status_label);

        page.flow_insert(&status_card);

        let keep_screen_on = self.config.borrow().config().runtime.keep_screen_on;
        {
            let row = GtkBox::new(Orientation::Horizontal, 14);
            row.set_hexpand(true);
            row.set_overflow(gtk::Overflow::Hidden);
            row.add_css_class("card");
            row.add_css_class("pulsedeck-card");
            row.add_css_class("settings-card-row");

            let row_icon = gtk::Image::from_icon_name("display-brightness-symbolic");
            row_icon.set_pixel_size(28);
            row_icon.set_valign(Align::Center);
            row_icon.add_css_class("settings-icon");
            row.append(&row_icon);

            let label_box = GtkBox::new(Orientation::Vertical, 2);
            label_box.set_hexpand(true);
            label_box.set_size_request(1, -1);
            label_box.set_valign(Align::Center);

            let title = Label::new(Some("保持屏幕开启"));
            title.set_halign(Align::Start);
            title.add_css_class("settings-name");
            label_box.append(&title);

            let desc = Label::new(Some("应用显示时阻止熄屏与系统休眠"));
            desc.set_halign(Align::Start);
            desc.add_css_class("settings-desc");
            label_box.append(&desc);
            row.append(&label_box);

            let sw = gtk::Switch::new();
            sw.set_valign(Align::Center);
            sw.set_active(keep_screen_on);
            let config = self.config.clone();
            let runtime = self.runtime.clone();
            sw.connect_active_notify(move |switch| {
                let mut config = config.borrow_mut();
                config.config_mut().runtime.keep_screen_on = switch.is_active();
                let next = config.config().runtime.clone();
                if let Err(error) = config.save() {
                    tracing::warn!("failed to save keep-screen setting: {}", error);
                }
                runtime.update_config(next);
            });
            row.append(&sw);

            page.flow_insert(&row);
        }

        let runtime_section = Label::new(Some("运行与省电"));
        runtime_section.set_halign(Align::Start);
        runtime_section.add_css_class("settings-name");
        runtime_section.set_margin_top(8);
        page.flow_insert(&runtime_section);

        let runtime_cfg = self.config.borrow().config().runtime.clone();
        for (title, description, active, field) in [
            (
                "启用空闲低功耗",
                "无真实用户操作后降低显示、刷新和动画",
                runtime_cfg.idle_power_saving,
                "idle_power_saving",
            ),
            (
                "外接电源高实时",
                "检测到外接电源在线时立即升档",
                runtime_cfg.external_realtime,
                "external_realtime",
            ),
            (
                "外接供电时禁止空闲",
                "检测到外接电源在线时保持正常亮度",
                runtime_cfg.external_prevents_idle,
                "external_prevents_idle",
            ),
            (
                "Codex 工作保持正常亮度",
                "只保护亮度；普通卡片仍可在用户空闲时节流",
                runtime_cfg.codex_keep_bright,
                "codex_keep_bright",
            ),
            (
                "Codex 完成提示音",
                "重要任务边沿只提示一次，后台仍保留",
                runtime_cfg.codex_completion_sound,
                "codex_completion_sound",
            ),
            (
                "重要事件带回前台",
                "Codex 完成或需要处理时主动显示窗口；默认关闭",
                runtime_cfg.bring_to_foreground_on_attention,
                "bring_to_foreground_on_attention",
            ),
            (
                "CPU 活动辅助判断",
                "仅作为空闲稳定期辅助信号，不替代真实用户操作",
                runtime_cfg.cpu_activity_hint,
                "cpu_activity_hint",
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
                    "idle_power_saving" => config.config_mut().runtime.idle_power_saving = value,
                    "external_realtime" => config.config_mut().runtime.external_realtime = value,
                    "external_prevents_idle" => {
                        config.config_mut().runtime.external_prevents_idle = value
                    }
                    "codex_keep_bright" => config.config_mut().runtime.codex_keep_bright = value,
                    "codex_completion_sound" => {
                        config.config_mut().runtime.codex_completion_sound = value
                    }
                    "bring_to_foreground_on_attention" => {
                        config.config_mut().runtime.bring_to_foreground_on_attention = value
                    }
                    "cpu_activity_hint" => config.config_mut().runtime.cpu_activity_hint = value,
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
                "应用内遮罩保留的近似亮度百分比",
                runtime_cfg.idle_visual_brightness_percent as u64,
                5,
                100,
                "idle_visual_brightness_percent",
            ),
            (
                "Codex 亮度保护",
                "新任务开始后的保护分钟数",
                runtime_cfg.codex_protection_minutes,
                0,
                1440,
                "codex_protection_minutes",
            ),
            (
                "完成唤醒时间",
                "重要事件后保持正常亮度的秒数",
                runtime_cfg.codex_attention_seconds,
                1,
                300,
                "codex_attention_seconds",
            ),
        ] {
            let (row, spin) = setting_spin_row(title, description, value, min, max);
            let config = self.config.clone();
            let runtime = self.runtime.clone();
            spin.connect_value_changed(move |spin| {
                let value = spin.value().round().max(0.0) as u64;
                let mut config = config.borrow_mut();
                match field {
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
                    "codex_protection_minutes" => {
                        config.config_mut().runtime.codex_protection_minutes = value
                    }
                    "codex_attention_seconds" => {
                        config.config_mut().runtime.codex_attention_seconds = value
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

        let (saving_row, saving_dropdown) = setting_dropdown_row(
            "刷新节能强度",
            "控制空闲模式下普通卡片的默认节流倍率",
            &["温和", "均衡", "强力"],
            match runtime_cfg.refresh_saving_strength.as_str() {
                "mild" => 0,
                "aggressive" => 2,
                _ => 1,
            },
        );
        {
            let config = self.config.clone();
            let runtime = self.runtime.clone();
            saving_dropdown.connect_selected_notify(move |dropdown| {
                let value = match dropdown.selected() {
                    0 => "mild",
                    2 => "aggressive",
                    _ => "balanced",
                };
                let mut config = config.borrow_mut();
                config.config_mut().runtime.refresh_saving_strength = value.into();
                let next = config.config().runtime.clone();
                let _ = config.save();
                runtime.update_config(next);
            });
        }
        page.flow_insert(&saving_row);

        let (display_row, display_dropdown) = setting_dropdown_row(
            "空闲显示方式",
            "遮罩保留完整页面；纯黑极简模式进一步降低 OLED 发光与合成",
            &["深色遮罩", "纯黑极简"],
            u32::from(runtime_cfg.idle_display == "minimal"),
        );
        {
            let config = self.config.clone();
            let runtime = self.runtime.clone();
            display_dropdown.connect_selected_notify(move |dropdown| {
                let mut config = config.borrow_mut();
                config.config_mut().runtime.idle_display = if dropdown.selected() == 1 {
                    "minimal"
                } else {
                    "dim"
                }
                .into();
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
                    "{:?} · {:?}\n供电 {:?} · 温度 {:?}\n{}\nCodex {:?} · 保护 {}s · 唤醒 {}s",
                    snapshot.mode,
                    snapshot.refresh_mode,
                    snapshot.power_verdict,
                    snapshot.thermal_verdict,
                    snapshot.reason,
                    snapshot.codex_phase,
                    snapshot.codex_protection_remaining_seconds,
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
                value.set_text(&format!(
                    "调度唤醒 {} · 卡片采集 {} · 外部进程 {}\nHTTP {} · 图片解码 {} · 动画帧 {}\nGTK 更新 {} · 磁盘读 {} · 磁盘写 {}",
                    counters[0],
                    counters[1],
                    counters[2],
                    counters[3],
                    counters[4],
                    counters[5],
                    counters[6],
                    counters[7],
                    counters[8]
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
                    .map(|source| source.source_type == "builtin")
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
                .and_then(|source| source.metric.as_deref())
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
            let pages = self.pages.clone();
            let scheduler = self.scheduler.clone();
            let card_metas = self.card_metas.clone();
            let previous_results = self.previous_results.clone();
            let scheduler_wake = self.scheduler_wake.clone();
            let current_page_id = self.current_page_id.clone();
            toggle.connect_active_notify(move |switch| {
                let mut config = config.borrow_mut();
                let changed_card = config
                    .config_mut()
                    .cards
                    .iter_mut()
                    .find(|card| card.id == card_id)
                    .map(|card| {
                        card.enabled = switch.is_active();
                        card.clone()
                    });
                if let Some(card) = changed_card {
                    if let Err(error) = config.save() {
                        tracing::warn!("failed to save card setting: {}", error);
                    }
                    drop(config);

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
                                    let scheduler = scheduler.clone();
                                    let wake = scheduler_wake.clone();
                                    let card_id = card.id.clone();
                                    metric_card.refresh_btn.connect_clicked(move |_| {
                                        scheduler.borrow_mut().request_now(&card_id);
                                        let _ = wake.try_send(());
                                    });
                                }
                                card_metas.borrow_mut().insert(
                                    card.id.clone(),
                                    CardMeta {
                                        page_id: card.page.clone(),
                                        interval_secs: card.refresh_interval,
                                    },
                                );
                                scheduler.borrow_mut().register_with_policy(
                                    &card.id,
                                    card.refresh_interval,
                                    &card.page,
                                    task_policy(&card),
                                );
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
                    }
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
        refresh_btn.connect_clicked(move |_| {
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
        let runtime_config = self.runtime.clone();
        let runtime_scheduler = self.scheduler.clone();
        let runtime_wake = self.scheduler_wake.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(snapshot) = runtime_rx.recv().await {
                let mut scheduler = runtime_scheduler.borrow_mut();
                scheduler.set_window_active(snapshot.foreground);
                scheduler.set_saving_strength(&runtime_config.config().refresh_saving_strength);
                scheduler.set_refresh_mode(snapshot.refresh_mode);
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
        if !config_path_buf.exists() {
            return;
        }

        let file = gio::File::for_path(&config_path_buf);
        let monitor = match file.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            Ok(m) => m,
            Err(_) => return,
        };

        let config_ref = self.config.clone();
        let reload_guard = self.reload_guard.clone();
        let runtime = self.runtime.clone();
        let persistent_sources = self.persistent_sources.clone();
        let scheduler = self.scheduler.clone();
        let scheduler_wake = self.scheduler_wake.clone();

        monitor.connect_changed(move |_monitor, _file, _other_file, event_type| {
            if event_type == gio::FileMonitorEvent::ChangesDoneHint
                || event_type == gio::FileMonitorEvent::Created
            {
                let guard = reload_guard.clone();
                let cfg = config_ref.clone();
                let runtime = runtime.clone();
                let sources = persistent_sources.clone();
                let scheduler = scheduler.clone();
                let scheduler_wake = scheduler_wake.clone();

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
                        if do_reload_config(&cfg) {
                            sources.lock().unwrap().clear();
                            for card in &cfg.borrow().config().cards {
                                scheduler.borrow_mut().request_now(&card.id);
                            }
                            let _ = scheduler_wake.try_send(());
                            runtime.update_config(cfg.borrow().config().runtime.clone());
                        }
                    }
                }

                if let Some(remaining_ms) = should_schedule {
                    let cfg2 = cfg.clone();
                    let runtime2 = runtime.clone();
                    let sources2 = sources.clone();
                    let scheduler2 = scheduler.clone();
                    let scheduler_wake2 = scheduler_wake.clone();
                    let sid_cell = guard.source_id.clone();

                    let sid = glib::timeout_add_local(
                        Duration::from_millis(remaining_ms + 50),
                        move || {
                            guard.pending.replace(false);
                            if do_reload_config(&cfg2) {
                                sources2.lock().unwrap().clear();
                                for card in &cfg2.borrow().config().cards {
                                    scheduler2.borrow_mut().request_now(&card.id);
                                }
                                let _ = scheduler_wake2.try_send(());
                                runtime2.update_config(cfg2.borrow().config().runtime.clone());
                            }
                            glib::ControlFlow::Break
                        },
                    );
                    sid_cell.replace(Some(sid));
                }
            }
        });
        self.config_monitor = Some(monitor);
    }

    fn start_scheduler_polling(&self, wake_rx: async_channel::Receiver<()>) {
        let scheduler = self.scheduler.clone();
        let card_metas = self.card_metas.clone();
        let config = self.config.clone();
        let handle = self.handle.clone();
        let metric_ctx = self.metric_ctx.clone();
        let builtin_metrics = self.builtin_metrics.clone();
        let persistent_sources = self.persistent_sources.clone();
        let metric_tx = self.metric_tx.clone();

        glib::MainContext::default().spawn_local(async move {
            loop {
                let delay = scheduler
                    .borrow_mut()
                    .next_task()
                    .map(|next| next.saturating_duration_since(Instant::now()))
                    .unwrap_or(Duration::from_secs(3600));
                let timer = Box::pin(glib::timeout_future(delay.max(Duration::from_millis(10))));
                let wake = Box::pin(wake_rx.recv());
                let _ = futures_util::future::select(timer, wake).await;
                crate::core::power_debug::increment(
                    crate::core::power_debug::Counter::SchedulerWake,
                );
                let ready = scheduler.borrow_mut().poll();

                for card_id in ready {
                    let meta = match card_metas.borrow().get(&card_id) {
                        Some(m) => CardMeta {
                            page_id: m.page_id.clone(),
                            interval_secs: m.interval_secs,
                        },
                        None => continue,
                    };

                    scheduler.borrow_mut().mark_started(&card_id);

                    let cfg = config.borrow();
                    let card_cfg = cfg.config().cards.iter().find(|c| c.id == card_id).cloned();
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

                    let tx = metric_tx.clone();
                    let h = handle.clone();
                    let ctx = metric_ctx.clone();
                    let bm = builtin_metrics.clone();
                    let sources = persistent_sources.clone();
                    let max_output = config.borrow().config().app.max_output_bytes;

                    let source = card_cfg.source.clone();
                    let needs_initial_follow_up = source.as_ref().is_some_and(|source| {
                        source.source_type == "builtin" && source.metric.as_deref() == Some("cpu")
                    });
                    let cache_ttl = card_cfg.cache_ttl_seconds;
                    let schedule = card_cfg
                        .schedule
                        .as_deref()
                        .map(crate::core::schedule::evaluate);
                    let schedule_state = match schedule {
                        Some(Ok(state)) => Some(state),
                        Some(Err(error)) => {
                            let _ = tx.try_send(MetricUpdate {
                                card_id,
                                page_id: meta.page_id,
                                result: MetricResult::error(error),
                                interval_secs: meta.interval_secs,
                                next_delay: None,
                            });
                            continue;
                        }
                        None => None,
                    };

                    // Scheduled data is immutable within one configured time slot. Load it
                    // from disk instead of repeating external requests after every launch.
                    if let Some(schedule) = &schedule_state {
                        let cached = match schedule.period.as_deref() {
                            Some(period) => cache::load(&card_id, None, Some(period)),
                            None => cache::load(&card_id, None, None),
                        };
                        if let Some(result) = cached {
                            let _ = tx.try_send(MetricUpdate {
                                card_id,
                                page_id: meta.page_id,
                                result,
                                interval_secs: schedule.next_delay_seconds,
                                next_delay: None,
                            });
                            continue;
                        }
                        if schedule.period.is_none() {
                            let _ = tx.try_send(MetricUpdate {
                                card_id,
                                page_id: meta.page_id,
                                result: MetricResult::unavailable("等待第一个计划更新时间"),
                                interval_secs: schedule.next_delay_seconds,
                                next_delay: None,
                            });
                            continue;
                        }
                    } else if let Some(result) =
                        cache_ttl.and_then(|ttl| cache::load(&card_id, Some(ttl), None))
                    {
                        let _ = tx.try_send(MetricUpdate {
                            card_id,
                            page_id: meta.page_id,
                            result,
                            interval_secs: meta.interval_secs,
                            next_delay: None,
                        });
                        continue;
                    }

                    h.spawn(async move {
                        let result = tokio::task::spawn_blocking(move || {
                            crate::core::power_debug::increment(
                                crate::core::power_debug::Counter::CardCollect,
                            );
                            collect_card_metric(
                                &card_cfg.id,
                                &source,
                                &ctx,
                                &bm,
                                &sources,
                                max_output,
                            )
                        })
                        .await
                        .unwrap_or_else(|e| {
                            MetricResult::error(format!("metric task panicked: {}", e))
                        });

                        if schedule_state.is_some() || cache_ttl.is_some() {
                            if let Err(error) = cache::store(
                                &card_id,
                                schedule_state
                                    .as_ref()
                                    .and_then(|state| state.period.as_deref()),
                                &result,
                            ) {
                                crate::core::error_limiter::warn(
                                    format!("cache:{card_id}"),
                                    format!("failed to persist cache for {card_id}: {error}"),
                                );
                            }
                        }

                        let page_id = meta.page_id.clone();
                        let interval = if let Some(schedule) = &schedule_state {
                            schedule.next_delay_seconds
                        } else {
                            meta.interval_secs
                        };

                        let _ = tx.try_send(MetricUpdate {
                            card_id,
                            page_id,
                            next_delay: if needs_initial_follow_up
                                && result.state == MetricState::Loading
                            {
                                Some(Duration::from_millis(250))
                            } else {
                                None
                            },
                            result,
                            interval_secs: interval,
                        });
                    });
                }
            }
        });
    }

    fn start_metric_receiver(&self, rx: async_channel::Receiver<MetricUpdate>) {
        let pages = self.pages.clone();
        let previous_results = self.previous_results.clone();
        let scheduler = self.scheduler.clone();
        let card_metas = self.card_metas.clone();
        let config = self.config.clone();
        let runtime = self.runtime.clone();
        let scheduler_wake = self.scheduler_wake.clone();

        glib::MainContext::default().spawn_local(async move {
            while let Ok(first) = rx.recv().await {
                let mut updates = vec![first];
                while let Ok(update) = rx.try_recv() {
                    updates.push(update);
                }
                let cfg = config.borrow();
                for update in updates {
                    let _meta = match card_metas.borrow().get(&update.card_id) {
                        Some(m) => CardMeta {
                            page_id: m.page_id.clone(),
                            interval_secs: m.interval_secs,
                        },
                        None => continue,
                    };

                    let card_cfg = cfg.config().cards.iter().find(|c| c.id == update.card_id);
                    let is_cpu =
                        card_cfg
                            .and_then(|card| card.source.as_ref())
                            .is_some_and(|source| {
                                source.source_type == "builtin"
                                    && source.metric.as_deref() == Some("cpu")
                            });
                    if is_cpu {
                        if let CardValue::Percentage(percent) = &update.result.value {
                            runtime.report_cpu_activity(*percent);
                        }
                    }
                    let display = card_cfg.and_then(|c| c.display.as_ref()).cloned();

                    let should_skip = {
                        let prev = previous_results.borrow();
                        if let Some(last) = prev.get(&update.card_id) {
                            results_equivalent(last, &update.result, display.as_ref())
                        } else {
                            false
                        }
                    };

                    if !should_skip {
                        crate::core::power_debug::increment(
                            crate::core::power_debug::Counter::GtkUpdate,
                        );
                        if let Some(page) = pages.borrow_mut().get_mut(&update.page_id) {
                            if let Some(card) = page.get_metric_card(&update.card_id) {
                                apply_metric_result(card, &update.result);
                            }
                        }
                        previous_results
                            .borrow_mut()
                            .insert(update.card_id.clone(), update.result.clone());
                    }

                    // A persisted response (including a persisted API error) is a
                    // completed round and must not trigger short retry wakeups.
                    let success = update.result.cached || update.result.state != MetricState::Error;
                    scheduler.borrow_mut().mark_done_after(
                        &update.card_id,
                        update.interval_secs,
                        success,
                        update.next_delay,
                    );
                    let _ = scheduler_wake.try_send(());
                }
                drop(cfg);
            }
        });
    }

    fn start_action_receiver(&self, rx: async_channel::Receiver<ActionUpdate>) {
        let pages = self.pages.clone();

        glib::MainContext::default().spawn_local(async move {
            while let Ok(first) = rx.recv().await {
                let mut updates = vec![first];
                while let Ok(update) = rx.try_recv() {
                    updates.push(update);
                }
                for update in updates {
                    if let Some(card_id) = update.result_card_id.as_deref() {
                        for page in pages.borrow_mut().values_mut() {
                            if let Some(card) = page.get_metric_card(card_id) {
                                show_action_result_dialog(
                                    &card.card,
                                    &update.action_id,
                                    &update.result,
                                );
                                break;
                            }
                        }
                        continue;
                    }
                    for (_page_id, page) in pages.borrow_mut().iter_mut() {
                        if let Some(card) = page.get_action_card(&update.action_id) {
                            card.set_running(false);
                            show_action_result_dialog(
                                &card.card,
                                &update.action_id,
                                &update.result,
                            );
                            break;
                        }
                    }
                }
            }
        });
    }

    pub fn present(&self) {
        self.window.present();
    }
}

fn collect_card_metric(
    card_id: &str,
    source: &Option<SourceConfig>,
    ctx: &Arc<MetricContext>,
    builtin_metrics: &Arc<Mutex<HashMap<String, Arc<Mutex<BuiltinMetric>>>>>,
    persistent_sources: &Arc<Mutex<HashMap<String, Arc<Mutex<PersistentSource>>>>>,
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

    match source.source_type.as_str() {
        "builtin" => {
            let metric_name = match &source.metric {
                Some(n) => n.clone(),
                None => {
                    return MetricResult::error("未指定内置指标名称");
                }
            };

            // Keep registry lookup and insertion under one guard. An `if let`
            // directly on `lock()` keeps its temporary guard alive through the
            // entire expression; trying to lock again in the `else` branch then
            // deadlocks the blocking worker pool and leaves every native card
            // blank.
            let builtin = {
                let mut registry = builtin_metrics.lock().unwrap();
                if let Some(metric) = registry.get(card_id).cloned() {
                    metric
                } else {
                    let new_metric = match create_builtin_metric(&metric_name) {
                        Some(metric) => metric,
                        None => {
                            return MetricResult::error(format!("未知的内置指标: {}", metric_name));
                        }
                    };
                    let metric = Arc::new(Mutex::new(new_metric));
                    registry.insert(card_id.to_string(), metric.clone());
                    metric
                }
            };
            let result = builtin.lock().unwrap().collect(ctx);
            result
        }

        "command" | "file" | "http" | "static_value" | "static" => {
            let persistent = {
                let mut registry = persistent_sources.lock().unwrap();
                if let Some(source) = registry.get(card_id).cloned() {
                    source
                } else {
                    let source = match build_persistent_source(source, max_output) {
                        Ok(source) => source,
                        Err(result) => return result,
                    };
                    let source = Arc::new(Mutex::new(source));
                    registry.insert(card_id.to_string(), source.clone());
                    source
                }
            };
            let result = persistent.lock().unwrap().collect(ctx);
            result
        }

        other => MetricResult::error(format!("不支持的数据源类型: {}", other)),
    }
}

fn build_persistent_source(
    source: &SourceConfig,
    max_output: usize,
) -> Result<PersistentSource, MetricResult> {
    match source.source_type.as_str() {
        "command" => {
            let program = source.program.as_deref().unwrap_or("echo").to_string();
            let args = source.args.clone().unwrap_or_default();
            let timeout = source.timeout_seconds;
            let max_out = source.max_output_bytes.min(max_output).max(1);
            let reverse = source
                .options
                .as_ref()
                .and_then(|o| o.get("reverse_lines"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let max_sub = source
                .options
                .as_ref()
                .and_then(|o| o.get("max_subtitle_lines"))
                .and_then(|v| v.as_integer())
                .unwrap_or(0) as usize;

            Ok(PersistentSource::Command(CommandMetric::new(
                program, args, timeout, max_out, reverse, max_sub,
            )))
        }
        "file" => {
            let path = match &source.path {
                Some(p) => PathBuf::from(p),
                None => {
                    return Err(MetricResult::error("未指定文件路径"));
                }
            };

            let first_line_only = source
                .options
                .as_ref()
                .and_then(|o| o.get("first_line_only"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            Ok(PersistentSource::File(FileMetric::new(
                path,
                first_line_only,
            )))
        }
        "http" => {
            let url = match &source.url {
                Some(u) => u.clone(),
                None => {
                    return Err(MetricResult::error("未指定 HTTP URL"));
                }
            };

            let method = source.method.clone();
            let headers = source.headers.clone();
            let body = source.body.clone();
            let timeout = source.timeout_seconds;
            let parser = source.parser.clone();
            let max_out = source.max_output_bytes.min(max_output).max(1);

            Ok(PersistentSource::Http(HttpMetric::new(
                url, method, headers, body, timeout, parser, max_out,
            )))
        }
        "static_value" | "static" => {
            let value = source
                .options
                .as_ref()
                .and_then(|o| o.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");

            Ok(PersistentSource::Static(MetricResult {
                value: CardValue::Text(value.to_string()),
                subtitle: None,
                tooltip: None,
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            }))
        }
        other => Err(MetricResult::error(format!(
            "不支持的持久数据源类型: {other}"
        ))),
    }
}

fn task_policy(card: &CardConfig) -> TaskPolicy {
    let configured = card.runtime.class.as_str();
    let source_type = card
        .source
        .as_ref()
        .map(|source| source.source_type.as_str())
        .unwrap_or("other");
    let metric = card
        .source
        .as_ref()
        .and_then(|source| source.metric.as_deref())
        .unwrap_or_default();
    let class = match configured {
        "system" | "system-realtime" => TaskClass::SystemRealtime,
        "network" | "network-rate" => TaskClass::NetworkRate,
        "network-status" => TaskClass::NetworkStatus,
        "battery" | "thermal" | "battery-thermal" => TaskClass::BatteryThermal,
        "command" => TaskClass::Command,
        "http" => TaskClass::Http,
        "file" => TaskClass::File,
        "static" => TaskClass::Static,
        _ => match source_type {
            "command" => TaskClass::Command,
            "http" => TaskClass::Http,
            "file" => TaskClass::File,
            "static" | "static_value" => TaskClass::Static,
            "builtin"
                if matches!(
                    metric,
                    "battery_capacity" | "battery_temperature" | "power" | "cpu_temperature"
                ) =>
            {
                TaskClass::BatteryThermal
            }
            "builtin" if metric == "network_traffic" => TaskClass::NetworkRate,
            "builtin" if metric == "network" => TaskClass::NetworkStatus,
            "builtin" => TaskClass::SystemRealtime,
            _ => TaskClass::Other,
        },
    };
    TaskPolicy {
        class,
        idle_behavior: if card.runtime.idle_behavior == "pause" {
            IdleBehavior::Pause
        } else {
            IdleBehavior::Throttle
        },
        idle_multiplier: card.runtime.idle_multiplier,
        external_realtime: card.runtime.external_realtime,
        realtime_multiplier: card.runtime.realtime_multiplier,
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
                ..
            },
            CardValue::Number {
                value: cv,
                unit: cu,
                ..
            },
        ) = (&prev.value, &curr.value)
        {
            if pu == cu {
                let diff = (cv - pv).abs();
                if diff < threshold {
                    // Subtitle and tooltip are commonly formatted from the same
                    // sample. Do not let their rounding bypass minimum_change.
                    return true;
                }
            }
        }

        if let (CardValue::Percentage(pp), CardValue::Percentage(pc)) = (&prev.value, &curr.value) {
            let diff = (pc - pp).abs();
            if diff < threshold {
                return true;
            }
        }
    }

    prev.value == curr.value && prev.subtitle == curr.subtitle && prev.tooltip == curr.tooltip
}

fn apply_metric_result(card: &mut crate::ui::metric_card::MetricCard, result: &MetricResult) {
    let state = match result.state {
        MetricState::Normal => CardState::Normal,
        MetricState::Loading => CardState::Loading,
        MetricState::Unavailable => CardState::Unavailable,
        MetricState::Error => CardState::Error,
        MetricState::Stale => CardState::Cached,
    };

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

fn execute_action_async(
    action_cfg: crate::core::config::ActionConfig,
    tx: async_channel::Sender<ActionUpdate>,
    handle: tokio::runtime::Handle,
    global_max_output: usize,
    result_card_id: Option<String>,
) {
    let action_id = action_cfg.id.clone();
    let command_parts = action_cfg.command.clone().unwrap_or_default();
    let timeout = action_cfg.timeout;
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
        let _ = tx.try_send(ActionUpdate {
            action_id,
            result_card_id,
            result,
        });
        return;
    }

    let program = command_parts[0].clone();
    let args: Vec<String> = command_parts.iter().skip(1).cloned().collect();

    handle.spawn(async move {
        let output =
            crate::execution::subprocess::run_command(&program, &args, timeout, max_output).await;

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

        let _ = tx.try_send(ActionUpdate {
            action_id,
            result_card_id,
            result,
        });
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

fn confirm_action(parent: &gtk::Widget, title: &str, detail: &str, run: impl FnOnce() + 'static) {
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
    glib::MainContext::default().spawn_local(async move {
        if dialog.choose_future(Some(&window)).await == Ok(1) {
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

fn show_action_result_dialog(parent: &gtk::Box, action_id: &str, result: &ActionResult) {
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

    dialog.show(Some(&window));
}

fn do_reload_config(config: &Rc<RefCell<ConfigManager>>) -> bool {
    let mut cfg = config.borrow_mut();
    match cfg.load() {
        Ok(()) => {
            tracing::info!("config hot-reloaded from {:?}", cfg.path());
            true
        }
        Err(e) => {
            tracing::warn!("config reload failed (keeping current): {}", e);
            false
        }
    }
}

fn default_builtin_cards() -> Vec<CardConfig> {
    vec![
        card(
            "cpu",
            "CPU 使用率",
            "monitor",
            10,
            "progress",
            5,
            "builtin",
            "cpu",
            Some("computer-symbolic"),
            Some("处理器总占用"),
        ),
        card(
            "memory",
            "内存使用",
            "monitor",
            20,
            "progress",
            10,
            "builtin",
            "memory",
            Some("chip-symbolic"),
            Some("已用 / 总量"),
        ),
        card(
            "battery",
            "电池电量",
            "monitor",
            30,
            "progress",
            30,
            "builtin",
            "battery_capacity",
            Some("battery-level-100-symbolic"),
            Some("当前剩余容量"),
        ),
        card(
            "battery-temp",
            "电池温度",
            "monitor",
            40,
            "value",
            30,
            "builtin",
            "battery_temperature",
            Some("sensors-temperature-symbolic"),
            Some("电池当前温度"),
        ),
        card(
            "uptime",
            "运行时间",
            "monitor",
            50,
            "value",
            60,
            "builtin",
            "uptime",
            Some("hourglass-symbolic"),
            Some("系统已运行时长"),
        ),
        card(
            "power",
            "实时功耗",
            "monitor",
            60,
            "value",
            15,
            "builtin",
            "power",
            Some("battery-symbolic"),
            Some("瞬时与平均功耗"),
        ),
        card(
            "network-status",
            "网络状态",
            "monitor",
            70,
            "status",
            15,
            "builtin",
            "network",
            Some("network-wireless-signal-excellent-symbolic"),
            Some("连接状态与 IP 地址"),
        ),
    ]
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

fn card(
    id: &str,
    title: &str,
    page: &str,
    order: i32,
    renderer: &str,
    interval: u64,
    source_type: &str,
    metric: &str,
    icon: Option<&str>,
    desc: Option<&str>,
) -> CardConfig {
    use crate::model::card_model::RendererKind;
    CardConfig {
        id: id.to_string(),
        title: title.to_string(),
        page: page.to_string(),
        order,
        renderer: match renderer {
            "progress" => RendererKind::Progress,
            "status" => RendererKind::Status,
            _ => RendererKind::Value,
        },
        refresh_interval: interval,
        enabled: true,
        icon: icon.map(|s| s.to_string()),
        description: desc.map(|s| s.to_string()),
        source: Some(SourceConfig {
            source_type: source_type.to_string(),
            metric: Some(metric.to_string()),
            path: None,
            program: None,
            args: None,
            timeout_seconds: 10,
            max_output_bytes: 20000,
            method: None,
            url: None,
            headers: None,
            body: None,
            shell: None,
            options: None,
            parser: None,
            plugin_id: None,
        }),
        display: None,
        cache_ttl_seconds: None,
        schedule: None,
        click_action: None,
        kind: None,
        plugin: None,
        runtime: crate::core::config::CardRuntimeConfig::default(),
    }
}
