use std::cell::RefCell;
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
use crate::core::scheduler::Scheduler;
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
.metric-value-good     { color: #1a5fb4; }
.metric-footer { font-size: 9px; opacity: 0.7; margin-top: 1px; }
.content-medium .metric-footer, .content-medium .metric-header-sub { font-size: 8px; }
.content-dense .metric-footer, .content-dense .metric-header-sub { font-size: 7px; }
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
"#;

struct MetricUpdate {
    card_id: String,
    page_id: String,
    result: MetricResult,
    interval_secs: u64,
}

struct ActionUpdate {
    action_id: String,
    result: ActionResult,
}

struct CardMeta {
    page_id: String,
    interval_secs: u64,
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
    card_metas: Rc<RefCell<HashMap<String, CardMeta>>>,
    current_page_id: Rc<RefCell<String>>,
    window_focused: Rc<RefCell<bool>>,
    metric_tx: async_channel::Sender<MetricUpdate>,
    action_tx: async_channel::Sender<ActionUpdate>,
    reload_guard: Rc<ConfigReloadGuard>,
    config_monitor: Option<gio::FileMonitor>,
    scheduler_wake: async_channel::Sender<()>,
}

impl MonitorWindow {
    pub fn new(app: &adw::Application, config: ConfigManager) -> Self {
        let window = adw::ApplicationWindow::new(app);
        window.set_default_size(420, 720);
        window.set_title(Some(&config.config().app.title));

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

        let sw_area = GtkBox::new(Orientation::Horizontal, 0);
        sw_area.set_halign(Align::Center);
        sw_area.set_hexpand(true);
        sw_area.set_margin_top(4);
        sw_area.append(&switcher);
        sw_area.add_css_class("tab-bar-area");

        let content = GtkBox::new(Orientation::Vertical, 0);
        content.append(&sw_area);
        content.append(&gtk::Separator::new(Orientation::Horizontal));
        content.append(&view_stack);

        window.set_content(Some(&content));

        let pages: Rc<RefCell<HashMap<String, Page>>> = Rc::new(RefCell::new(HashMap::new()));
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

        let (metric_tx, metric_rx) = async_channel::unbounded::<MetricUpdate>();
        let (action_tx, action_rx) = async_channel::unbounded::<ActionUpdate>();
        let (scheduler_wake, scheduler_wake_rx) = async_channel::bounded::<()>(1);

        let previous_results: Rc<RefCell<HashMap<String, MetricResult>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let builtin_metrics: Arc<Mutex<HashMap<String, Arc<Mutex<BuiltinMetric>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let card_metas: Rc<RefCell<HashMap<String, CardMeta>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let current_page_id = Rc::new(RefCell::new(String::new()));
        let window_focused = Rc::new(RefCell::new(true));
        let reload_guard = ConfigReloadGuard::new(500);

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
            card_metas,
            current_page_id: current_page_id.clone(),
            window_focused: window_focused.clone(),
            metric_tx,
            action_tx,
            reload_guard: reload_guard.clone(),
            config_monitor: None,
            scheduler_wake,
        };

        win.setup_pages();
        win.setup_screen_inhibit(app);
        win.setup_lifecycle();
        win.setup_config_monitor();
        win.start_scheduler_polling(scheduler_wake_rx);
        win.start_metric_receiver(metric_rx);
        win.start_action_receiver(action_rx);

        win
    }

    fn setup_screen_inhibit(&mut self, app: &adw::Application) {
        if !self.config.borrow().config().app.keep_screen_on {
            return;
        }

        let application = app.clone();
        let inhibit_cookie: Rc<RefCell<Option<u32>>> = Rc::new(RefCell::new(None));
        let cookie = inhibit_cookie.clone();

        // `is-active` represents the foreground application window. Unlike a
        // process-lifetime inhibit, this releases the system as soon as PulseDeck
        // is minimized or another application comes to the foreground.
        self.window.connect_is_active_notify(move |window| {
            if window.is_active() {
                if cookie.borrow().is_none() {
                    let flags =
                        gtk::ApplicationInhibitFlags::IDLE | gtk::ApplicationInhibitFlags::SUSPEND;
                    let id = application.inhibit(
                        Some(window),
                        flags,
                        Some("PulseDeck 正在前台显示实时监控信息"),
                    );
                    cookie.replace(Some(id));
                }
            } else if let Some(id) = cookie.borrow_mut().take() {
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
                    #[cfg(feature = "scrcpy-forge")]
                    scrcpy_forge: None,
                },
                crate::core::config::PageConfig {
                    id: "actions".into(),
                    title: "操作".into(),
                    icon: Some("system-run-symbolic".into()),
                    order: 20,
                    kind: None,
                    #[cfg(feature = "scrcpy-forge")]
                    scrcpy_forge: None,
                },
                crate::core::config::PageConfig {
                    id: "settings".into(),
                    title: "设置".into(),
                    icon: Some("preferences-system-symbolic".into()),
                    order: 30,
                    kind: None,
                    #[cfg(feature = "scrcpy-forge")]
                    scrcpy_forge: None,
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

        let page_ids: Vec<String> = pages_list.iter().map(|p| p.id.clone()).collect();

        self.pages.borrow_mut().clear();
        self.card_metas.borrow_mut().clear();
        self.builtin_metrics.lock().unwrap().clear();

        for page_cfg in &pages_list {
            if let Some(container) = crate::plugins::build_page(self.handle.clone(), page_cfg) {
                self.view_stack
                    .add_titled(&container, Some(&page_cfg.id), &page_cfg.title);
                continue;
            }
            if crate::plugins::is_optional_page(page_cfg.kind.as_deref()) {
                tracing::warn!(
                    page = %page_cfg.id,
                    kind = ?page_cfg.kind,
                    "optional page skipped because its Cargo feature is disabled"
                );
                continue;
            }
            let ui = self.config.borrow().config().ui.clone();
            let mut page = Page::new(&page_cfg.id, &ui);
            self.populate_page(&mut page, &page_cfg.id, &cards, &actions);

            self.view_stack
                .add_titled(&page.container, Some(&page_cfg.id), &page_cfg.title);

            self.pages.borrow_mut().insert(page_cfg.id.clone(), page);
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

        let mut page_actions: Vec<&crate::core::config::ActionConfig> =
            all_actions.iter().filter(|a| a.page == page_id).collect();
        page_actions.sort_by_key(|_| 0);

        for card_cfg in &page_cards {
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
            }

            self.card_metas.borrow_mut().insert(
                card_cfg.id.clone(),
                CardMeta {
                    page_id: page_id.to_string(),
                    interval_secs: card_cfg.refresh_interval,
                },
            );

            self.scheduler
                .borrow_mut()
                .register(&card_cfg.id, card_cfg.refresh_interval, page_id);
        }

        for action_cfg in &page_actions {
            let icon = action_cfg.icon.as_deref().unwrap_or("system-run-symbolic");
            let action_id = action_cfg.id.clone();
            let action_cfg_clone = (*action_cfg).clone();
            let action_tx = self.action_tx.clone();
            let handle = self.handle.clone();
            let global_max_output = self.config.borrow().config().app.max_output_bytes;

            page.add_action_card(
                &action_id,
                &action_cfg.name,
                action_cfg.description.as_deref().unwrap_or(""),
                icon,
                action_cfg.confirm,
                move |_id| {
                    let cfg = action_cfg_clone.clone();
                    let tx = action_tx.clone();
                    let h = handle.clone();
                    execute_action_async(cfg, tx, h, global_max_output);
                },
            );
        }

        if page_id == "settings" {
            self.add_settings_content(page);
        }
    }

    fn add_settings_content(&self, page: &mut Page) {
        let status_card = GtkBox::new(Orientation::Horizontal, 10);
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
        status_label.add_css_class("status-text");
        status_card.append(&status_label);

        page.flow_insert(&status_card);

        let keep_screen_on = self.config.borrow().config().app.keep_screen_on;
        {
            let row = GtkBox::new(Orientation::Horizontal, 14);
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
            sw.connect_active_notify(move |switch| {
                let mut config = config.borrow_mut();
                config.config_mut().app.keep_screen_on = switch.is_active();
                if let Err(error) = config.save() {
                    tracing::warn!("failed to save keep-screen setting: {}", error);
                }
            });
            row.append(&sw);

            page.flow_insert(&row);
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
                                scheduler.borrow_mut().register(
                                    &card.id,
                                    card.refresh_interval,
                                    &card.page,
                                );
                                scheduler
                                    .borrow_mut()
                                    .set_active_page(&current_page_id.borrow());
                                let _ = scheduler_wake.try_send(());
                            }
                        }
                    } else if let Some(page) = pages.borrow_mut().get_mut(&card.page) {
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
        let focused = self.window_focused.clone();
        let sched = self.scheduler.clone();
        let focus_wake = self.scheduler_wake.clone();
        let pause_when_inactive = self.config.borrow().config().app.pause_when_inactive;

        self.window.connect_has_focus_notify(move |_window| {
            let is_focused = _window.has_focus();
            focused.replace(is_focused);
            if pause_when_inactive {
                sched.borrow_mut().set_window_active(is_focused);
                let _ = focus_wake.try_send(());
            }
        });

        let current_page = self.current_page_id.clone();
        let scheduler = self.scheduler.clone();
        let scheduler_wake = self.scheduler_wake.clone();
        let view_stack = self.view_stack.clone();

        view_stack.connect_visible_child_name_notify(move |stack| {
            if let Some(name) = stack.visible_child_name() {
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

        monitor.connect_changed(move |_monitor, _file, _other_file, event_type| {
            if event_type == gio::FileMonitorEvent::ChangesDoneHint
                || event_type == gio::FileMonitorEvent::Created
            {
                let guard = reload_guard.clone();
                let cfg = config_ref.clone();

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
                        do_reload_config(&cfg);
                    }
                }

                if let Some(remaining_ms) = should_schedule {
                    let cfg2 = cfg.clone();
                    let sid_cell = guard.source_id.clone();

                    let sid = glib::timeout_add_local(
                        Duration::from_millis(remaining_ms + 50),
                        move || {
                            guard.pending.replace(false);
                            do_reload_config(&cfg2);
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
                    let max_output = config.borrow().config().app.max_output_bytes;

                    let source = card_cfg.source.clone();
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
                            });
                            continue;
                        }
                        if schedule.period.is_none() {
                            let _ = tx.try_send(MetricUpdate {
                                card_id,
                                page_id: meta.page_id,
                                result: MetricResult::unavailable("等待第一个计划更新时间"),
                                interval_secs: schedule.next_delay_seconds,
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
                        });
                        continue;
                    }

                    h.spawn(async move {
                        let result = tokio::task::spawn_blocking(move || {
                            collect_card_metric(&card_cfg.id, &source, &ctx, &bm, max_output)
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
                                tracing::warn!(
                                    "failed to persist cache for {}: {}",
                                    card_id,
                                    error
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
                    scheduler.borrow_mut().mark_done(
                        &update.card_id,
                        update.interval_secs,
                        success,
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

            let mut cmd = CommandMetric::new(program, args, timeout, max_out, reverse, max_sub);
            cmd.collect_no_ctx()
        }

        "file" => {
            let path = match &source.path {
                Some(p) => PathBuf::from(p),
                None => {
                    return MetricResult::error("未指定文件路径");
                }
            };

            let first_line_only = source
                .options
                .as_ref()
                .and_then(|o| o.get("first_line_only"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let mut file_metric = FileMetric::new(path, first_line_only);
            file_metric.collect(ctx)
        }

        "http" => {
            let url = match &source.url {
                Some(u) => u.clone(),
                None => {
                    return MetricResult::error("未指定 HTTP URL");
                }
            };

            let method = source.method.clone();
            let headers = source.headers.clone();
            let body = source.body.clone();
            let timeout = source.timeout_seconds;
            let parser = source.parser.clone();
            let max_out = source.max_output_bytes.min(max_output).max(1);

            let mut http_metric =
                HttpMetric::new(url, method, headers, body, timeout, parser, max_out);
            http_metric.collect(ctx)
        }

        "static_value" | "static" => {
            let value = source
                .options
                .as_ref()
                .and_then(|o| o.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");

            MetricResult {
                value: CardValue::Text(value.to_string()),
                subtitle: None,
                tooltip: None,
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            }
        }

        other => MetricResult::error(format!("不支持的数据源类型: {}", other)),
    }
}

fn results_equivalent(
    prev: &MetricResult,
    curr: &MetricResult,
    display: Option<&DisplayConfig>,
) -> bool {
    if prev.state != curr.state
        || prev.subtitle != curr.subtitle
        || prev.tooltip != curr.tooltip
        || prev.cached != curr.cached
    {
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
                return diff < threshold;
            }
        }

        if let (CardValue::Percentage(pp), CardValue::Percentage(pc)) = (&prev.value, &curr.value) {
            let diff = (pc - pp).abs();
            return diff < threshold;
        }
    }

    prev.value == curr.value
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
}

fn execute_action_async(
    action_cfg: crate::core::config::ActionConfig,
    tx: async_channel::Sender<ActionUpdate>,
    handle: tokio::runtime::Handle,
    global_max_output: usize,
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
        let _ = tx.try_send(ActionUpdate { action_id, result });
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

        let _ = tx.try_send(ActionUpdate { action_id, result });
    });
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

fn do_reload_config(config: &Rc<RefCell<ConfigManager>>) {
    let mut cfg = config.borrow_mut();
    match cfg.load() {
        Ok(()) => {
            tracing::info!("config hot-reloaded from {:?}", cfg.path());
        }
        Err(e) => {
            tracing::warn!("config reload failed (keeping current): {}", e);
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
    }
}
