use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use gio::prelude::*;
use gtk::gdk;
use gtk::prelude::*;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::config::{AnimationConfig, PetConfig};
use crate::core::config::CardConfig;
use crate::core::error::AppError;
use crate::core::runtime::{
    ImportantEventKind, RuntimeHandle, RuntimeSnapshot, ThermalVerdict, UserActivity, Visibility,
    VisualPolicy,
};
use crate::core::text::MAX_UI_TEXT_BYTES;
use crate::plugins::{CardPresentation, CardPresentationHandle};

const RUNTIME_DATA_KEY: &str = "pulsedeck-pet-runtime";
const MAX_FRAME_CACHE_STATES: usize = 3;
const MAX_FRAME_COUNT: usize = 256;
const MAX_FRAME_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FRAME_STATE_BYTES: usize = 16 * 1024 * 1024;
const MAX_FRAME_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_FRAME_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_FRAME_DIMENSION: u64 = 4096;
const MAX_FRAME_HEADER_BYTES: u64 = 64 * 1024;
const MAX_FRAME_LOAD_WORKERS: usize = 1;
const MAX_MISSING_FRAME_WARNINGS: usize = 512;
const MAX_STATE_FILE_BYTES: usize = 128 * 1024;
const MAX_STATE_NAME_BYTES: usize = 128;
const MAX_STATE_ID_BYTES: usize = 256;

static FRAME_LOAD_PERMITS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(MAX_FRAME_LOAD_WORKERS)));

#[derive(Debug, Deserialize)]
struct StateEvent {
    state: String,
    #[serde(default)]
    detail: Option<String>,
    timestamp_ms: u64,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    event_id: Option<String>,
}

struct LoadedFrames {
    frames: Vec<gdk::Texture>,
    failures: Vec<(PathBuf, String)>,
}

struct Runtime {
    root: glib::WeakRef<gtk::Box>,
    picture: gtk::Picture,
    fallback: gtk::Label,
    status: gtk::Label,
    config: PetConfig,
    frames: RefCell<Vec<gdk::Texture>>,
    frame_cache: RefCell<HashMap<String, Vec<gdk::Texture>>>,
    cache_order: RefCell<VecDeque<String>>,
    frame_cache_bytes: Cell<usize>,
    loading_states: RefCell<HashMap<String, u64>>,
    missing_frame_warnings: RefCell<HashSet<PathBuf>>,
    frame_load_generation: Cell<u64>,
    state_load_generation: Cell<u64>,
    state_load_running: Cell<bool>,
    frame_index: Cell<usize>,
    visual_policy: Cell<VisualPolicy>,
    animation_source: RefCell<Option<glib::SourceId>>,
    offline_source: RefCell<Option<glib::SourceId>>,
    transition_source: RefCell<Option<glib::SourceId>>,
    presentation_reset_source: RefCell<Option<glib::SourceId>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    current_state: RefCell<String>,
    preferred_presentation: Cell<CardPresentation>,
    current_presentation: Cell<CardPresentation>,
    presentation: Option<CardPresentationHandle>,
    runtime: RuntimeHandle,
    handle: tokio::runtime::Handle,
    cancellation: CancellationToken,
}

pub fn build(
    card: &CardConfig,
    config: PetConfig,
    presentation: Option<CardPresentationHandle>,
    runtime_handle: RuntimeHandle,
    tokio_handle: tokio::runtime::Handle,
    cancellation: CancellationToken,
) -> Result<gtk::Box, AppError> {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.add_css_class("card");
    root.add_css_class("pulsedeck-card");
    root.add_css_class("pet-card");
    root.set_halign(gtk::Align::Fill);
    root.set_valign(gtk::Align::Fill);

    let title = gtk::Label::new(Some(&card.title));
    title.set_halign(gtk::Align::Start);
    title.add_css_class("metric-header-name");
    root.append(&title);

    let picture = gtk::Picture::new();
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Contain);
    root.append(&picture);

    let fallback = gtk::Label::new(Some("💤"));
    fallback.set_hexpand(true);
    fallback.set_vexpand(true);
    fallback.set_justify(gtk::Justification::Center);
    fallback.set_css_classes(&["title-1"]);
    root.append(&fallback);

    let status = gtk::Label::new(Some("Codex 未运行"));
    status.set_halign(gtk::Align::Center);
    status.add_css_class("metric-footer");
    status.set_visible(config.show_status);
    root.append(&status);

    let preferred_presentation = load_presentation(&config.presentation_file);
    let runtime = Rc::new(Runtime {
        root: root.downgrade(),
        picture,
        fallback,
        status,
        config,
        frames: RefCell::new(Vec::new()),
        frame_cache: RefCell::new(HashMap::new()),
        cache_order: RefCell::new(VecDeque::new()),
        frame_cache_bytes: Cell::new(0),
        loading_states: RefCell::new(HashMap::new()),
        missing_frame_warnings: RefCell::new(HashSet::new()),
        frame_load_generation: Cell::new(0),
        state_load_generation: Cell::new(0),
        state_load_running: Cell::new(false),
        frame_index: Cell::new(0),
        visual_policy: Cell::new(VisualPolicy::Stopped),
        animation_source: RefCell::new(None),
        offline_source: RefCell::new(None),
        transition_source: RefCell::new(None),
        presentation_reset_source: RefCell::new(None),
        monitor: RefCell::new(None),
        current_state: RefCell::new(String::new()),
        preferred_presentation: Cell::new(preferred_presentation),
        current_presentation: Cell::new(CardPresentation::Normal),
        presentation,
        runtime: runtime_handle,
        handle: tokio_handle,
        cancellation,
    });
    Runtime::setup_presentation_menu(&runtime);
    runtime.set_state("offline", None, true);
    Runtime::watch_state_file(&runtime)?;
    Runtime::watch_visual_policy(&runtime);
    Runtime::watch_mapping(&runtime);

    // Store the controller on the widget itself, but explicitly steal it from
    // the widget during destroy. This keeps the runtime alive while the card
    // is mounted without leaving the Runtime -> root -> Runtime cycle behind.
    unsafe {
        root.set_data(RUNTIME_DATA_KEY, runtime);
    }
    root.connect_destroy(move |root| unsafe {
        if let Some(runtime) = root.steal_data::<Rc<Runtime>>(RUNTIME_DATA_KEY) {
            runtime.stop_timers();
        }
    });
    Ok(root)
}

impl Runtime {
    fn setup_presentation_menu(this: &Rc<Self>) {
        if this.presentation.is_none() {
            return;
        }

        let popover = gtk::Popover::new();
        popover.set_has_arrow(true);
        popover.set_autohide(true);
        let Some(root) = this.root.upgrade() else {
            return;
        };
        popover.set_parent(&root);

        let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
        menu.set_margin_top(6);
        menu.set_margin_bottom(6);
        menu.set_margin_start(6);
        menu.set_margin_end(6);

        for (label, mode) in [
            ("普通大小", CardPresentation::Normal),
            ("占四个格", CardPresentation::Quad),
            ("占六个格", CardPresentation::Expanded),
            ("全屏显示", CardPresentation::Fullscreen),
        ] {
            let button = gtk::Button::with_label(label);
            button.add_css_class("flat");
            let weak = Rc::downgrade(this);
            let menu_popover = popover.clone();
            button.connect_clicked(move |_| {
                if let Some(runtime) = weak.upgrade() {
                    runtime
                        .runtime
                        .report_user_activity(UserActivity::PluginControl);
                    runtime.select_presentation(mode);
                }
                menu_popover.popdown();
            });
            menu.append(&button);
        }
        popover.set_child(Some(&menu));

        let gesture = gtk::GestureLongPress::new();
        let menu_popover = popover.clone();
        gesture.connect_pressed(move |_, x, y| {
            menu_popover.set_pointing_to(Some(&gdk::Rectangle::new(
                x.round() as i32,
                y.round() as i32,
                1,
                1,
            )));
            menu_popover.popup();
        });
        root.add_controller(gesture);

        let double_click = gtk::GestureClick::new();
        double_click.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(this);
        double_click.connect_released(move |_, presses, _, _| {
            if presses == 2 {
                if let Some(runtime) = weak.upgrade() {
                    runtime
                        .runtime
                        .report_user_activity(UserActivity::PluginControl);
                    let next = next_presentation(runtime.current_presentation.get());
                    runtime.select_presentation(next);
                }
            }
        });
        root.add_controller(double_click);
        root.set_tooltip_text(Some("双击依次切换大小；长按可直接选择显示方式"));
    }

    fn watch_state_file(this: &Rc<Self>) -> Result<(), AppError> {
        let parent = this
            .config
            .state_file
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        crate::core::config::ensure_private_parent(&this.config.state_file)?;
        let monitor = gio::File::for_path(parent)
            .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
            .map_err(|error| AppError::Plugin(format!("cannot monitor pet state: {error}")))?;
        let weak = Rc::downgrade(this);
        monitor.connect_changed(move |_, file, other_file, event| {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            let file_path = file.path();
            let other_path = other_file.and_then(|other| other.path());
            if state_file_event_matches(
                event,
                file_path.as_deref(),
                other_path.as_deref(),
                runtime.config.state_file.as_path(),
            ) {
                runtime.load_state();
            }
        });
        this.monitor.replace(Some(monitor));
        this.load_state();
        Ok(())
    }

    fn load_state(self: &Rc<Self>) {
        if self.cancellation.is_cancelled() {
            return;
        }
        let generation = self.state_load_generation.get().wrapping_add(1);
        self.state_load_generation.set(generation);
        if self.state_load_running.replace(true) {
            return;
        }
        let path = self.config.state_file.clone();
        let handle = self.handle.clone();
        let weak = Rc::downgrade(self);
        let join = handle.spawn_blocking(move || read_state_file(path));
        glib::MainContext::default().spawn_local(async move {
            let loaded = join.await.unwrap_or_else(|error| {
                Err(format!("读取 PetCard 状态 worker 失败: {error}"))
            });
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            if runtime.cancellation.is_cancelled() {
                return;
            }
            runtime.state_load_running.set(false);
            if runtime.state_load_generation.get() != generation {
                runtime.load_state();
                return;
            }
            match loaded {
                Ok(Some(event)) => runtime.apply_state_event(event),
                Ok(None) => runtime.set_state("offline", None, true),
                Err(error) => {
                    tracing::warn!(path = ?runtime.config.state_file, %error, "ignored invalid pet-card state");
                }
            }
        });
    }

    fn apply_state_event(self: &Rc<Self>, event: StateEvent) {
        let now = now_ms();
        let max_age = self.config.offline_after_seconds.saturating_mul(1000);
        if now.saturating_sub(event.timestamp_ms) >= max_age {
            self.set_state("offline", None, true);
            return;
        }
        if let Some(source) = self.transition_source.borrow_mut().take() {
            source.remove();
        }
        let task_id = event
            .task_id
            .clone()
            .unwrap_or_else(|| format!("legacy-{}", event.timestamp_ms));
        if let Some(kind) = important_event_kind(&event.state) {
            let event_id = event
                .event_id
                .clone()
                .unwrap_or_else(|| event.timestamp_ms.to_string());
            if self.runtime.report_agent_event(task_id, event_id, kind)
                && self.runtime.config().agent_completion_sound
            {
                self.play_completion_sound();
            }
        } else if is_active_agent_state(&event.state) {
            self.runtime.report_agent_started(task_id);
        }
        self.set_state(
            &event.state,
            event.detail.as_deref(),
            event.state == "offline",
        );
        if event.state == "done" {
            self.schedule_ready(self.config.done_hold_seconds);
        }
        self.schedule_offline(max_age.saturating_sub(now.saturating_sub(event.timestamp_ms)));
    }

    fn schedule_ready(self: &Rc<Self>, delay_seconds: u64) {
        if let Some(source) = self.transition_source.borrow_mut().take() {
            source.remove();
        }
        let weak = Rc::downgrade(self);
        let source =
            glib::timeout_add_local_once(Duration::from_secs(delay_seconds.max(1)), move || {
                if let Some(runtime) = weak.upgrade() {
                    runtime.set_state("ready", None, false);
                    runtime.transition_source.borrow_mut().take();
                }
            });
        self.transition_source.replace(Some(source));
    }

    fn schedule_offline(self: &Rc<Self>, delay_ms: u64) {
        if let Some(source) = self.offline_source.borrow_mut().take() {
            source.remove();
        }
        let weak = Rc::downgrade(self);
        let source =
            glib::timeout_add_local_once(Duration::from_millis(delay_ms.max(1)), move || {
                if let Some(runtime) = weak.upgrade() {
                    runtime.set_state("offline", None, true);
                    runtime.offline_source.borrow_mut().take();
                }
            });
        self.offline_source.replace(Some(source));
    }

    fn set_state(
        self: &Rc<Self>,
        requested: &str,
        detail: Option<&str>,
        clear_agent_on_offline: bool,
    ) {
        let state = normalize_state(requested);
        let previous_state = self.current_state.borrow().clone();
        if previous_state == state {
            if state == "offline" && clear_agent_on_offline {
                self.runtime.clear_agent();
            }
            if let Some(detail) = detail {
                self.status.set_text(detail);
            }
            return;
        }
        self.current_state.replace(state.to_string());
        if state == "offline" {
            if clear_agent_on_offline {
                self.runtime.clear_agent();
            }
            self.schedule_presentation_reset();
        } else {
            self.cancel_presentation_reset();
            self.request_presentation(self.preferred_presentation.get());
        }
        self.stop_animation();
        self.status
            .set_text(detail.unwrap_or_else(|| state_label(state)));
        self.fallback.set_text(state_emoji(state));
        self.frame_load_generation
            .set(self.frame_load_generation.get().wrapping_add(1));

        let animation = self
            .config
            .animations
            .get(state)
            .or_else(|| self.config.animations.get("default"))
            .cloned();
        let generation = self.frame_load_generation.get();
        if let Some(textures) = self.cached_frames(state) {
            self.apply_frames(textures);
        } else {
            self.apply_frames(Vec::new());
            if let Some(animation) = animation {
                self.load_frames_async(state, animation, generation);
            }
        }
    }

    fn play_completion_sound(&self) {
        let argv = completion_sound_argv(self.config.completion_sound_file.as_deref());
        let argv = argv.iter().map(OsString::as_os_str).collect::<Vec<_>>();
        let flags = gio::SubprocessFlags::STDOUT_SILENCE | gio::SubprocessFlags::STDERR_SILENCE;
        match gio::Subprocess::newv(&argv, flags) {
            Ok(process) => {
                let completed = process.clone();
                process.wait_async(None::<&gio::Cancellable>, move |result| {
                    if let Err(error) = result {
                        tracing::debug!(%error, "completion sound process wait failed");
                    } else if !completed.is_successful() {
                        tracing::debug!("completion sound player exited unsuccessfully");
                    }
                });
            }
            Err(error) => {
                tracing::debug!(%error, "completion sound player unavailable");
                if let Some(root) = self.root.upgrade() {
                    root.error_bell();
                }
            }
        }
    }

    fn cached_frames(&self, state: &str) -> Option<Vec<gdk::Texture>> {
        let frames = self.frame_cache.borrow().get(state).cloned();
        if frames.is_some() {
            let mut order = self.cache_order.borrow_mut();
            order.retain(|key| key != state);
            order.push_back(state.to_string());
        }
        frames
    }

    fn apply_frames(self: &Rc<Self>, frames: Vec<gdk::Texture>) {
        self.frames.replace(frames);
        self.frame_index.set(0);
        if let Some(texture) = self.frames.borrow().first() {
            self.picture.set_paintable(Some(texture));
            self.picture.set_visible(true);
            self.fallback.set_visible(false);
        } else {
            self.picture.set_paintable(Option::<&gdk::Texture>::None);
            self.picture.set_visible(false);
            self.fallback.set_visible(true);
            return;
        }
        if self.frames.borrow().len() > 1 {
            self.apply_runtime_snapshot(&self.runtime.snapshot());
        }
    }

    fn load_frames_async(
        self: &Rc<Self>,
        state: &str,
        animation: AnimationConfig,
        generation: u64,
    ) {
        if self.cancellation.is_cancelled() {
            return;
        }
        let claimed = claim_frame_load(&mut self.loading_states.borrow_mut(), state, generation);
        if !claimed {
            return;
        }
        let state = state.to_string();
        let asset_root = self.config.asset_root.clone();
        let known_missing = self.missing_frame_warnings.borrow().clone();
        let paths = animation
            .frames
            .into_iter()
            .map(|path| resolve_path(asset_root.as_deref(), &path))
            .filter(|path| !known_missing.contains(path))
            .collect::<Vec<_>>();
        if paths.is_empty() {
            self.loading_states.borrow_mut().remove(&state);
            self.cache_frames(&state, Vec::new());
            return;
        }
        let handle = self.handle.clone();
        let cancellation = self.cancellation.clone();
        let load = load_frames_with_budget(handle, paths, cancellation);
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let Some(loaded) = load.await else {
                return;
            };
            let loaded = match loaded {
                Ok(loaded) => loaded,
                Err(error) => LoadedFrames {
                    frames: Vec::new(),
                    failures: vec![(PathBuf::from("<worker>"), error.to_string())],
                },
            };
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            if runtime.cancellation.is_cancelled() {
                return;
            }
            // A -> B -> A is valid, and an old worker must not consume a new
            // A load that was started after stop_timers cleared the previous
            // ownership token. The per-state generation is the completion
            // guard; state equality alone is insufficient here.
            let current_load =
                finish_frame_load(&mut runtime.loading_states.borrow_mut(), &state, generation);
            if !current_load {
                return;
            }
            runtime.warn_frame_failures(loaded.failures);
            runtime.cache_frames(&state, loaded.frames.clone());
            if *runtime.current_state.borrow() == state {
                runtime.apply_frames(loaded.frames);
            }
        });
    }

    fn cache_frames(&self, state: &str, frames: Vec<gdk::Texture>) {
        let new_bytes = estimate_frames_bytes(&frames);
        let old_bytes = self
            .frame_cache
            .borrow_mut()
            .insert(state.to_string(), frames)
            .map_or(0, |old| estimate_frames_bytes(&old));
        self.frame_cache_bytes.set(
            self.frame_cache_bytes
                .get()
                .saturating_sub(old_bytes)
                .saturating_add(new_bytes),
        );
        let mut order = self.cache_order.borrow_mut();
        order.retain(|key| key != state);
        order.push_back(state.to_string());
        while order.len() > MAX_FRAME_CACHE_STATES
            || self.frame_cache_bytes.get() > MAX_FRAME_CACHE_BYTES
        {
            if let Some(old) = order.pop_front() {
                if let Some(frames) = self.frame_cache.borrow_mut().remove(&old) {
                    self.frame_cache_bytes.set(
                        self.frame_cache_bytes
                            .get()
                            .saturating_sub(estimate_frames_bytes(&frames)),
                    );
                }
            } else {
                break;
            }
        }
    }

    fn warn_frame_failures(&self, failures: Vec<(PathBuf, String)>) {
        let mut warned = self.missing_frame_warnings.borrow_mut();
        for (path, error) in failures {
            if warned.contains(&path) || warned.len() >= MAX_MISSING_FRAME_WARNINGS {
                continue;
            }
            warned.insert(path.clone());
            tracing::warn!(?path, %error, "failed to load pet frame");
        }
    }

    fn start_animation(self: &Rc<Self>, animation: &AnimationConfig, cap: Option<u32>) {
        self.stop_animation();
        let Some(root) = self.root.upgrade() else {
            return;
        };
        if !root.is_mapped() {
            return;
        }
        let mut fps = animation.fps.unwrap_or(self.config.fps).clamp(1, 12);
        if let Some(cap) = cap {
            fps = fps.min(cap.max(1));
        }
        let interval = Duration::from_millis((1000 / fps) as u64);
        let looping = animation.r#loop;
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local(interval, move || {
            let Some(runtime) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if runtime.cancellation.is_cancelled() {
                runtime.animation_source.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            // B2's unmapped hard stop is unconditional. The retained
            // compatibility field is decoded below but cannot keep a hidden
            // animation alive.
            let Some(root) = runtime.root.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !root.is_mapped() {
                runtime.animation_source.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            let frames = runtime.frames.borrow();
            if frames.len() < 2 {
                runtime.animation_source.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            let next = runtime.frame_index.get() + 1;
            if next >= frames.len() && !looping {
                runtime.animation_source.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            let next = next % frames.len();
            runtime.frame_index.set(next);
            crate::core::power_debug::increment(crate::core::power_debug::Counter::AnimationTick);
            runtime.picture.set_paintable(Some(&frames[next]));
            glib::ControlFlow::Continue
        });
        self.animation_source.replace(Some(source));
    }

    fn stop_animation(&self) {
        if let Some(source) = self.animation_source.borrow_mut().take() {
            source.remove();
        }
    }

    fn watch_visual_policy(this: &Rc<Self>) {
        let rx = this.runtime.subscribe();
        let weak = Rc::downgrade(this);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(snapshot) = rx.recv().await {
                let Some(runtime) = weak.upgrade() else {
                    break;
                };
                runtime.apply_runtime_snapshot(&snapshot);
            }
        });
    }

    fn watch_mapping(this: &Rc<Self>) {
        let Some(root) = this.root.upgrade() else {
            return;
        };
        let weak = Rc::downgrade(this);
        root.connect_map(move |_| {
            if let Some(runtime) = weak.upgrade() {
                runtime.apply_runtime_snapshot(&runtime.runtime.snapshot());
            }
        });
        let weak = Rc::downgrade(this);
        root.connect_unmap(move |_| {
            if let Some(runtime) = weak.upgrade() {
                runtime.stop_animation();
            }
        });
    }

    fn apply_runtime_snapshot(self: &Rc<Self>, snapshot: &RuntimeSnapshot) {
        let state = self.current_state.borrow().clone();
        let animation = self
            .config
            .animations
            .get(&state)
            .or_else(|| self.config.animations.get("default"));
        let policy = animation.map_or(snapshot.visual_policy, |animation| {
            pet_visual_policy(
                snapshot.visual_policy,
                snapshot.visibility,
                snapshot.thermal_verdict,
                snapshot.periodic_refresh_paused,
                &state,
                animation.r#loop,
            )
        });
        let Some(root) = self.root.upgrade() else {
            return;
        };
        let should_animate = root.is_mapped()
            && self.frames.borrow().len() > 1
            && matches!(policy, VisualPolicy::Full | VisualPolicy::Capped(_));
        if self.visual_policy.get() == policy
            && ((should_animate && self.animation_source.borrow().is_some()) || !should_animate)
        {
            return;
        }
        self.visual_policy.set(policy);
        self.stop_animation();
        if !root.is_mapped() || matches!(policy, VisualPolicy::Stopped | VisualPolicy::Frozen) {
            return;
        }
        let Some(animation) = animation else {
            return;
        };
        if self.frames.borrow().len() < 2 {
            return;
        }
        let cap = match policy {
            VisualPolicy::Full => None,
            VisualPolicy::Capped(fps) => Some(fps),
            VisualPolicy::Frozen | VisualPolicy::Stopped => return,
        };
        self.start_animation(animation, cap);
    }

    fn select_presentation(self: &Rc<Self>, presentation: CardPresentation) {
        self.preferred_presentation.set(presentation);
        if let Err(error) = save_presentation(&self.config.presentation_file, presentation) {
            tracing::warn!(path = ?self.config.presentation_file, %error, "failed to save pet-card presentation");
        }
        self.request_presentation(presentation);
        if self.current_state.borrow().as_str() == "offline" {
            self.schedule_presentation_reset();
        }
    }

    fn request_presentation(&self, presentation: CardPresentation) {
        if self.current_presentation.get() == presentation {
            return;
        }
        if let Some(request) = &self.presentation {
            request.request(presentation);
            self.current_presentation.set(presentation);
        }
    }

    fn schedule_presentation_reset(self: &Rc<Self>) {
        self.cancel_presentation_reset();
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(
            Duration::from_secs(self.config.offline_normal_after_seconds.max(1)),
            move || {
                if let Some(runtime) = weak.upgrade() {
                    runtime.request_presentation(CardPresentation::Normal);
                    runtime.presentation_reset_source.borrow_mut().take();
                }
            },
        );
        self.presentation_reset_source.replace(Some(source));
    }

    fn cancel_presentation_reset(&self) {
        if let Some(source) = self.presentation_reset_source.borrow_mut().take() {
            source.remove();
        }
    }

    fn stop_timers(&self) {
        self.frame_load_generation
            .set(self.frame_load_generation.get().wrapping_add(1));
        self.loading_states.borrow_mut().clear();
        if let Some(presentation) = &self.presentation {
            presentation.close();
        }
        self.stop_animation();
        if let Some(source) = self.offline_source.borrow_mut().take() {
            source.remove();
        }
        if let Some(source) = self.transition_source.borrow_mut().take() {
            source.remove();
        }
        self.cancel_presentation_reset();
        self.monitor.borrow_mut().take();
        self.frames.replace(Vec::new());
        self.frame_cache.borrow_mut().clear();
        self.cache_order.borrow_mut().clear();
        self.frame_cache_bytes.set(0);
    }
}

async fn load_frames_with_budget(
    handle: tokio::runtime::Handle,
    paths: Vec<PathBuf>,
    cancellation: CancellationToken,
) -> Option<Result<LoadedFrames, tokio::task::JoinError>> {
    let permit = tokio::select! {
        permit = FRAME_LOAD_PERMITS.clone().acquire_owned() => permit.ok()?,
        _ = cancellation.cancelled() => return None,
    };
    let join = handle.spawn_blocking(move || {
        let _permit = permit;
        load_frames(paths)
    });
    Some(join.await)
}

fn load_frames(paths: Vec<PathBuf>) -> LoadedFrames {
    let mut loaded = LoadedFrames {
        frames: Vec::new(),
        failures: Vec::new(),
    };
    let mut decoded_bytes = 0_u64;
    if paths.len() > MAX_FRAME_COUNT {
        loaded.failures.push((
            PathBuf::from("<animation>"),
            format!("动画帧数超过 {MAX_FRAME_COUNT} 限制"),
        ));
        return loaded;
    }
    for path in paths {
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.len() > MAX_FRAME_FILE_BYTES => {
                loaded
                    .failures
                    .push((path, format!("文件超过 {MAX_FRAME_FILE_BYTES} 字节限制")));
                continue;
            }
            Err(error) => {
                loaded
                    .failures
                    .push((path, format!("读取文件信息失败: {error}")));
                continue;
            }
            Ok(_) => {}
        }
        match preflight_dimensions(&path) {
            Ok(Some((width, height)))
                if width == 0
                    || height == 0
                    || width > MAX_FRAME_DIMENSION
                    || height > MAX_FRAME_DIMENSION
                    || width.saturating_mul(height) > MAX_FRAME_PIXELS =>
            {
                loaded.failures.push((
                    path,
                    format!(
                        "图片尺寸 {width}x{height} 超过限制 ({MAX_FRAME_DIMENSION}px / {MAX_FRAME_PIXELS} pixels)"
                    ),
                ));
                continue;
            }
            Ok(Some(_)) | Ok(None) => {}
            Err(error) => {
                loaded.failures.push((path, error));
                continue;
            }
        }
        match gdk::Texture::from_file(&gio::File::for_path(&path)) {
            Ok(texture) => {
                let pixels =
                    (texture.width().max(0) as u64).saturating_mul(texture.height().max(0) as u64);
                let estimated = pixels.saturating_mul(4);
                let next_decoded_bytes = decoded_bytes.saturating_add(estimated);
                if pixels == 0
                    || pixels > MAX_FRAME_PIXELS
                    || next_decoded_bytes > MAX_FRAME_STATE_BYTES as u64
                {
                    loaded.failures.push((
                        path,
                        format!("解码后的图像尺寸/内存超过限制 ({MAX_FRAME_STATE_BYTES} bytes)"),
                    ));
                    continue;
                }
                crate::core::power_debug::increment(crate::core::power_debug::Counter::ImageDecode);
                decoded_bytes = next_decoded_bytes;
                loaded.frames.push(texture);
            }
            Err(error) => loaded.failures.push((path, error.to_string())),
        }
    }
    loaded
}

/// Read only a small image header before handing a file to GDK. Unknown
/// containers are allowed through and remain protected by the post-decode
/// pixel budget; known dimensions are rejected before a potentially expensive
/// allocation/decode occurs.
fn preflight_dimensions(path: &Path) -> Result<Option<(u64, u64)>, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("读取图片头失败: {error}"))?;
    let mut header = Vec::with_capacity(MAX_FRAME_HEADER_BYTES as usize);
    file.take(MAX_FRAME_HEADER_BYTES)
        .read_to_end(&mut header)
        .map_err(|error| format!("读取图片头失败: {error}"))?;

    if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok((header.len() >= 24).then(|| {
            (
                u32::from_be_bytes(header[16..20].try_into().unwrap()) as u64,
                u32::from_be_bytes(header[20..24].try_into().unwrap()) as u64,
            )
        }));
    }
    if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        return Ok((header.len() >= 10).then(|| {
            (
                u16::from_le_bytes(header[6..8].try_into().unwrap()) as u64,
                u16::from_le_bytes(header[8..10].try_into().unwrap()) as u64,
            )
        }));
    }
    if header.starts_with(b"BM") && header.len() >= 26 {
        let width = i32::from_le_bytes(header[18..22].try_into().unwrap());
        let height = i32::from_le_bytes(header[22..26].try_into().unwrap());
        return Ok(Some((
            width.unsigned_abs() as u64,
            height.unsigned_abs() as u64,
        )));
    }
    if header.len() >= 10
        && u16::from_le_bytes(header[0..2].try_into().unwrap()) == 0
        && u16::from_le_bytes(header[2..4].try_into().unwrap()) == 1
    {
        let count = u16::from_le_bytes(header[4..6].try_into().unwrap()) as usize;
        let required = 6_usize.saturating_add(count.saturating_mul(16));
        if header.len() >= required && count > 0 {
            let mut dimensions: Option<(u64, u64)> = None;
            for entry in header[6..required].chunks_exact(16) {
                let width = if entry[0] == 0 { 256 } else { entry[0] as u64 };
                let height = if entry[1] == 0 { 256 } else { entry[1] as u64 };
                dimensions = Some(match dimensions {
                    Some((old_width, old_height)) => (old_width.max(width), old_height.max(height)),
                    None => (width, height),
                });
            }
            return Ok(dimensions);
        }
    }
    if header.len() >= 30
        && &header[0..4] == b"RIFF"
        && &header[8..12] == b"WEBP"
        && &header[12..16] == b"VP8X"
    {
        let width =
            1 + (header[24] as u64 | ((header[25] as u64) << 8) | ((header[26] as u64) << 16));
        let height =
            1 + (header[27] as u64 | ((header[28] as u64) << 8) | ((header[29] as u64) << 16));
        return Ok(Some((width, height)));
    }
    if header.starts_with(&[0xff, 0xd8]) {
        return Ok(jpeg_dimensions(&header));
    }
    Ok(None)
}

fn jpeg_dimensions(header: &[u8]) -> Option<(u64, u64)> {
    let mut index = 2;
    while index + 1 < header.len() {
        if header[index] != 0xff {
            index += 1;
            continue;
        }
        while index < header.len() && header[index] == 0xff {
            index += 1;
        }
        if index >= header.len() {
            break;
        }
        let marker = header[index];
        index += 1;
        if marker == 0xd8 || marker == 0xd9 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if index + 2 > header.len() {
            break;
        }
        let length = u16::from_be_bytes([header[index], header[index + 1]]) as usize;
        if length < 2 || index.saturating_add(length) > header.len() {
            break;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) && length >= 7 {
            let height = u16::from_be_bytes([header[index + 3], header[index + 4]]) as u64;
            let width = u16::from_be_bytes([header[index + 5], header[index + 6]]) as u64;
            return Some((width, height));
        }
        index += length;
    }
    None
}

fn estimate_frames_bytes(frames: &[gdk::Texture]) -> usize {
    frames.iter().fold(0_usize, |total, texture| {
        total.saturating_add(
            (texture.width().max(0) as usize)
                .saturating_mul(texture.height().max(0) as usize)
                .saturating_mul(4),
        )
    })
}

fn read_state_file(path: PathBuf) -> Result<Option<StateEvent>, String> {
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取文件失败: {error}")),
    };
    let mut bytes = Vec::with_capacity(MAX_STATE_FILE_BYTES.min(8192).saturating_add(1));
    file.take((MAX_STATE_FILE_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取文件失败: {error}"))?;
    if bytes.len() > MAX_STATE_FILE_BYTES {
        return Err(format!("状态文件超过 {} 字节限制", MAX_STATE_FILE_BYTES));
    }
    let event: StateEvent =
        serde_json::from_slice(&bytes).map_err(|error| format!("状态 JSON 无效: {error}"))?;
    validate_state_event(event).map(Some)
}

fn validate_state_event(event: StateEvent) -> Result<StateEvent, String> {
    if event.state.trim().is_empty() || event.state.len() > MAX_STATE_NAME_BYTES {
        return Err(format!(
            "状态名称必须非空且不超过 {MAX_STATE_NAME_BYTES} 字节"
        ));
    }
    if event
        .detail
        .as_ref()
        .is_some_and(|detail| detail.len() > MAX_UI_TEXT_BYTES)
    {
        return Err(format!("状态详情超过 {MAX_UI_TEXT_BYTES} 字节限制"));
    }
    for (field, value) in [
        ("task_id", event.task_id.as_deref()),
        ("event_id", event.event_id.as_deref()),
    ] {
        if value.is_some_and(|value| value.trim().is_empty() || value.len() > MAX_STATE_ID_BYTES) {
            return Err(format!(
                "{field} 必须非空且不超过 {MAX_STATE_ID_BYTES} 字节"
            ));
        }
    }
    Ok(event)
}

fn state_file_event_matches(
    event: gio::FileMonitorEvent,
    file_path: Option<&Path>,
    other_path: Option<&Path>,
    state_file: &Path,
) -> bool {
    if !matches!(
        event,
        gio::FileMonitorEvent::Created
            | gio::FileMonitorEvent::ChangesDoneHint
            | gio::FileMonitorEvent::Deleted
            | gio::FileMonitorEvent::Moved
            | gio::FileMonitorEvent::Renamed
            | gio::FileMonitorEvent::MovedIn
            | gio::FileMonitorEvent::MovedOut
    ) {
        return false;
    }
    file_path == Some(state_file) || other_path == Some(state_file)
}

fn presentation_name(presentation: CardPresentation) -> &'static str {
    match presentation {
        CardPresentation::Normal => "normal",
        CardPresentation::Quad => "quad",
        CardPresentation::Expanded => "expanded",
        CardPresentation::Fullscreen => "fullscreen",
    }
}

fn next_presentation(presentation: CardPresentation) -> CardPresentation {
    match presentation {
        CardPresentation::Normal => CardPresentation::Quad,
        CardPresentation::Quad => CardPresentation::Expanded,
        CardPresentation::Expanded => CardPresentation::Fullscreen,
        CardPresentation::Fullscreen => CardPresentation::Normal,
    }
}

fn parse_presentation(value: &str) -> CardPresentation {
    match value.trim() {
        "quad" => CardPresentation::Quad,
        "expanded" => CardPresentation::Expanded,
        "fullscreen" => CardPresentation::Fullscreen,
        _ => CardPresentation::Normal,
    }
}

fn load_presentation(path: &Path) -> CardPresentation {
    std::fs::read_to_string(path)
        .map(|value| parse_presentation(&value))
        .unwrap_or(CardPresentation::Normal)
}

fn save_presentation(path: &Path, presentation: CardPresentation) -> std::io::Result<()> {
    crate::core::config::ensure_private_parent(path)?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, format!("{}\n", presentation_name(presentation)))?;
    #[cfg(unix)]
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(temporary, path)
}

fn resolve_path(root: Option<&Path>, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.unwrap_or_else(|| Path::new(".")).join(path)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn normalize_state(state: &str) -> &str {
    match state {
        "ready" | "thinking" | "working" | "coding" | "waiting" | "confirm" | "cancelled"
        | "aborted" | "error" | "done" => state,
        _ => "offline",
    }
}

fn pet_visual_policy(
    policy: VisualPolicy,
    visibility: Visibility,
    thermal: ThermalVerdict,
    periodic_refresh_paused: bool,
    state: &str,
    looping: bool,
) -> VisualPolicy {
    if visibility == Visibility::Unmapped {
        return VisualPolicy::Stopped;
    }
    if matches!(thermal, ThermalVerdict::Hot | ThermalVerdict::Throttled) {
        return VisualPolicy::Frozen;
    }
    if periodic_refresh_paused && looping {
        // Quiet hours without an observation lease mean nobody is watching.
        // Keep consuming Agent state events, but do not spend frames on any
        // unattended loop. Real input reopens the bounded observation window.
        return VisualPolicy::Frozen;
    }
    if is_active_agent_state(state)
        && matches!(policy, VisualPolicy::Full | VisualPolicy::Capped(_))
    {
        // A mapped daytime/observed Agent state is visible work even when GTK
        // focus is absent or the local interaction clock is idle.
        return VisualPolicy::Full;
    }
    if looping && !is_active_agent_state(state) {
        return match policy {
            VisualPolicy::Full | VisualPolicy::Capped(_) => VisualPolicy::Capped(1),
            other => other,
        };
    }
    policy
}

fn is_active_agent_state(state: &str) -> bool {
    matches!(
        state,
        "thinking" | "working" | "coding" | "waiting" | "confirm"
    )
}

fn important_event_kind(state: &str) -> Option<ImportantEventKind> {
    match state {
        "done" => Some(ImportantEventKind::Completed),
        "error" => Some(ImportantEventKind::Failed),
        "cancelled" => Some(ImportantEventKind::Cancelled),
        "waiting" => Some(ImportantEventKind::WaitingInput),
        "confirm" => Some(ImportantEventKind::ConfirmationRequired),
        "aborted" => Some(ImportantEventKind::Aborted),
        _ => None,
    }
}

fn state_label(state: &str) -> &'static str {
    match state {
        "ready" => "Codex 已就绪",
        "thinking" => "正在思考",
        "working" => "正在执行工具",
        "coding" => "正在修改代码",
        "waiting" => "等待确认",
        "confirm" => "需要用户确认",
        "cancelled" => "任务已取消",
        "aborted" => "任务异常停止",
        "error" => "执行遇到问题",
        "done" => "本轮已完成",
        _ => "Codex 未运行",
    }
}

fn state_emoji(state: &str) -> &'static str {
    match state {
        "ready" => "👋",
        "thinking" => "🤔",
        "working" => "🛠️",
        "coding" => "⌨️",
        "waiting" => "❗",
        "confirm" => "❓",
        "cancelled" => "⛔",
        "aborted" => "⚠️",
        "error" => "💥",
        "done" => "🎉",
        _ => "💤",
    }
}

fn claim_frame_load(loading: &mut HashMap<String, u64>, state: &str, generation: u64) -> bool {
    if loading.contains_key(state) {
        return false;
    }
    loading.insert(state.to_string(), generation);
    true
}

fn finish_frame_load(loading: &mut HashMap<String, u64>, state: &str, generation: u64) -> bool {
    if loading.get(state).copied() != Some(generation) {
        return false;
    }
    loading.remove(state);
    true
}

fn completion_sound_argv(file: Option<&Path>) -> Vec<OsString> {
    let mut argv = vec![OsString::from("canberra-gtk-play")];
    if let Some(file) = file {
        argv.push(OsString::from("--file"));
        argv.push(file.as_os_str().to_owned());
    } else {
        argv.push(OsString::from("--id=complete"));
    }
    argv
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::Path;

    use super::{
        claim_frame_load, completion_sound_argv, finish_frame_load, is_active_agent_state,
        next_presentation, parse_presentation, pet_visual_policy, presentation_name,
        state_file_event_matches, validate_state_event, StateEvent,
    };
    use crate::core::runtime::{ThermalVerdict, Visibility, VisualPolicy};
    use crate::plugins::CardPresentation;

    #[test]
    fn presentation_preference_round_trips() {
        for presentation in [
            CardPresentation::Normal,
            CardPresentation::Quad,
            CardPresentation::Expanded,
            CardPresentation::Fullscreen,
        ] {
            assert_eq!(
                parse_presentation(presentation_name(presentation)),
                presentation
            );
        }
    }

    #[test]
    fn double_click_cycle_visits_every_presentation() {
        let mut presentation = CardPresentation::Normal;
        for expected in [
            CardPresentation::Quad,
            CardPresentation::Expanded,
            CardPresentation::Fullscreen,
            CardPresentation::Normal,
        ] {
            presentation = next_presentation(presentation);
            assert_eq!(presentation, expected);
        }
    }

    #[test]
    fn unknown_presentation_falls_back_to_normal() {
        assert_eq!(parse_presentation("future-mode"), CardPresentation::Normal);
    }

    #[test]
    fn atomic_state_replacements_match_both_rename_paths() {
        let state_file = Path::new("/run/user/10000/pulsedeck/codex-pet.json");
        let temporary = Path::new("/run/user/10000/pulsedeck/.pi-pet.tmp");

        assert!(state_file_event_matches(
            gio::FileMonitorEvent::Renamed,
            Some(temporary),
            Some(state_file),
            state_file,
        ));
        assert!(state_file_event_matches(
            gio::FileMonitorEvent::Moved,
            Some(temporary),
            Some(state_file),
            state_file,
        ));
        assert!(state_file_event_matches(
            gio::FileMonitorEvent::Created,
            Some(state_file),
            None,
            state_file,
        ));
        assert!(!state_file_event_matches(
            gio::FileMonitorEvent::Changed,
            Some(temporary),
            Some(state_file),
            state_file,
        ));
    }

    #[test]
    fn waiting_and_confirmation_are_active_agent_states() {
        for state in ["thinking", "working", "coding", "waiting", "confirm"] {
            assert!(is_active_agent_state(state));
        }
        for state in ["ready", "done", "error", "offline"] {
            assert!(!is_active_agent_state(state));
        }
    }

    #[test]
    fn state_event_text_is_bounded_before_reaching_gtk_or_runtime() {
        let valid = StateEvent {
            state: "working".into(),
            detail: Some("正在执行".into()),
            timestamp_ms: 1,
            task_id: Some("task-1".into()),
            event_id: Some("event-1".into()),
        };
        assert!(validate_state_event(valid).is_ok());

        let oversized = StateEvent {
            state: "working".into(),
            detail: Some("x".repeat(super::MAX_UI_TEXT_BYTES + 1)),
            timestamp_ms: 1,
            task_id: None,
            event_id: None,
        };
        assert!(validate_state_event(oversized).is_err());
    }

    #[test]
    fn frame_load_singleflight_preserves_a_worker_across_a_to_b_to_a() {
        let mut loading = HashMap::new();

        assert!(claim_frame_load(&mut loading, "A", 1));
        assert!(claim_frame_load(&mut loading, "B", 2));
        assert!(!claim_frame_load(&mut loading, "A", 3));
        assert_eq!(loading.get("A"), Some(&1));
        assert!(finish_frame_load(&mut loading, "B", 2));
        assert!(finish_frame_load(&mut loading, "A", 1));

        // A cancelled old worker can finish after a new A worker has claimed
        // the same state. It must not remove or apply the new generation.
        assert!(claim_frame_load(&mut loading, "A", 4));
        assert!(!finish_frame_load(&mut loading, "A", 1));
        assert_eq!(loading.get("A"), Some(&4));
        assert!(finish_frame_load(&mut loading, "A", 4));
        assert!(loading.is_empty());
    }

    #[test]
    fn mapped_active_agent_keeps_configured_rate_unless_thermal_is_hot() {
        assert_eq!(
            pet_visual_policy(
                VisualPolicy::Capped(1),
                Visibility::MappedActive,
                ThermalVerdict::Normal,
                false,
                "working",
                true,
            ),
            VisualPolicy::Full
        );
        assert_eq!(
            pet_visual_policy(
                VisualPolicy::Capped(1),
                Visibility::MappedActive,
                ThermalVerdict::Normal,
                true,
                "working",
                true,
            ),
            VisualPolicy::Frozen
        );
        assert_eq!(
            pet_visual_policy(
                VisualPolicy::Full,
                Visibility::MappedActive,
                ThermalVerdict::Normal,
                false,
                "ready",
                true,
            ),
            VisualPolicy::Capped(1)
        );
        assert_eq!(
            pet_visual_policy(
                VisualPolicy::Capped(1),
                Visibility::MappedActive,
                ThermalVerdict::Normal,
                true,
                "ready",
                true,
            ),
            VisualPolicy::Frozen
        );
        assert_eq!(
            pet_visual_policy(
                VisualPolicy::Full,
                Visibility::MappedActive,
                ThermalVerdict::Normal,
                false,
                "done",
                false,
            ),
            VisualPolicy::Full
        );
        assert_eq!(
            pet_visual_policy(
                VisualPolicy::Capped(1),
                Visibility::MappedInactive,
                ThermalVerdict::Normal,
                false,
                "working",
                true,
            ),
            VisualPolicy::Full
        );
        assert_eq!(
            pet_visual_policy(
                VisualPolicy::Capped(2),
                Visibility::MappedInactive,
                ThermalVerdict::Unknown,
                true,
                "working",
                true,
            ),
            VisualPolicy::Frozen
        );
        for thermal in [ThermalVerdict::Hot, ThermalVerdict::Throttled] {
            assert_eq!(
                pet_visual_policy(
                    VisualPolicy::Frozen,
                    Visibility::MappedInactive,
                    thermal,
                    false,
                    "working",
                    true,
                ),
                VisualPolicy::Frozen
            );
        }
        assert_eq!(
            pet_visual_policy(
                VisualPolicy::Full,
                Visibility::Unmapped,
                ThermalVerdict::Normal,
                false,
                "working",
                true,
            ),
            VisualPolicy::Stopped
        );
        assert_eq!(
            pet_visual_policy(
                VisualPolicy::Capped(2),
                Visibility::MappedActive,
                ThermalVerdict::Warm,
                false,
                "working",
                true,
            ),
            VisualPolicy::Full
        );
    }

    #[test]
    fn completion_sound_uses_theme_event_or_custom_file() {
        assert_eq!(
            completion_sound_argv(None),
            ["canberra-gtk-play", "--id=complete"]
        );
        assert_eq!(
            completion_sound_argv(Some(Path::new("/tmp/custom sound.oga"))),
            [
                OsString::from("canberra-gtk-play"),
                OsString::from("--file"),
                OsString::from("/tmp/custom sound.oga"),
            ]
        );
    }
}
