use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gio::prelude::*;
use gtk::gdk;
use gtk::prelude::*;
use serde::Deserialize;

use super::config::{AnimationConfig, PetConfig};
use crate::core::config::CardConfig;
use crate::core::error::AppError;
use crate::core::runtime::{AnimationPolicy, ImportantEventKind, RuntimeHandle, UserActivity};
use crate::plugins::{CardPresentation, CardPresentationHandle};

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

struct Runtime {
    root: gtk::Box,
    picture: gtk::Picture,
    fallback: gtk::Label,
    status: gtk::Label,
    config: PetConfig,
    frames: RefCell<Vec<gdk::Texture>>,
    frame_cache: RefCell<HashMap<String, Vec<gdk::Texture>>>,
    cache_order: RefCell<VecDeque<String>>,
    frame_index: Cell<usize>,
    animation_policy: Cell<AnimationPolicy>,
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
}

pub fn build(
    card: &CardConfig,
    config: PetConfig,
    presentation: Option<CardPresentationHandle>,
    runtime_handle: RuntimeHandle,
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
        root: root.clone(),
        picture,
        fallback,
        status,
        config,
        frames: RefCell::new(Vec::new()),
        frame_cache: RefCell::new(HashMap::new()),
        cache_order: RefCell::new(VecDeque::new()),
        frame_index: Cell::new(0),
        animation_policy: Cell::new(AnimationPolicy::Stopped),
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
    });
    Runtime::setup_presentation_menu(&runtime);
    runtime.set_state("offline", None);
    Runtime::watch_state_file(&runtime)?;
    Runtime::watch_runtime_mode(&runtime);
    Runtime::watch_mapping(&runtime);

    // The signal closure owns the runtime for exactly as long as the widget.
    let keep_alive = runtime.clone();
    root.connect_destroy(move |_| keep_alive.stop_timers());
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
        popover.set_parent(&this.root);

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
        this.root.add_controller(gesture);

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
        this.root.add_controller(double_click);
        this.root
            .set_tooltip_text(Some("双击依次切换大小；长按可直接选择显示方式"));
    }

    fn watch_state_file(this: &Rc<Self>) -> Result<(), AppError> {
        let parent = this
            .config
            .state_file
            .parent()
            .ok_or_else(|| AppError::Plugin("pet-card state file has no parent".into()))?;
        std::fs::create_dir_all(parent)?;
        let monitor = gio::File::for_path(parent)
            .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
            .map_err(|error| AppError::Plugin(format!("cannot monitor pet state: {error}")))?;
        let weak = Rc::downgrade(this);
        monitor.connect_changed(move |_, file, _, event| {
            if !matches!(
                event,
                gio::FileMonitorEvent::Created
                    | gio::FileMonitorEvent::ChangesDoneHint
                    | gio::FileMonitorEvent::MovedIn
            ) {
                return;
            }
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            if file.path().as_deref() == Some(runtime.config.state_file.as_path()) {
                runtime.load_state();
            }
        });
        this.monitor.replace(Some(monitor));
        this.load_state();
        Ok(())
    }

    fn load_state(self: &Rc<Self>) {
        let Ok(data) = std::fs::read_to_string(&self.config.state_file) else {
            self.set_state("offline", None);
            return;
        };
        let Ok(event) = serde_json::from_str::<StateEvent>(&data) else {
            tracing::warn!(path = ?self.config.state_file, "ignored invalid pet-card state");
            return;
        };
        let now = now_ms();
        let max_age = self.config.offline_after_seconds.saturating_mul(1000);
        if now.saturating_sub(event.timestamp_ms) >= max_age {
            self.set_state("offline", None);
            return;
        }
        if let Some(source) = self.transition_source.borrow_mut().take() {
            source.remove();
        }
        let task_id = event
            .task_id
            .clone()
            .unwrap_or_else(|| format!("legacy-{}", event.timestamp_ms));
        if is_working_state(&event.state) {
            self.runtime.report_codex_started(task_id.clone());
        }
        if let Some(kind) = important_event_kind(&event.state) {
            let event_id = event
                .event_id
                .clone()
                .unwrap_or_else(|| event.timestamp_ms.to_string());
            if self.runtime.report_codex_event(task_id, event_id, kind)
                && self.runtime.config().codex_completion_sound
            {
                self.play_completion_sound();
            }
        }
        self.set_state(&event.state, event.detail.as_deref());
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
                    runtime.set_state("ready", None);
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
                    runtime.set_state("offline", None);
                    runtime.offline_source.borrow_mut().take();
                }
            });
        self.offline_source.replace(Some(source));
    }

    fn set_state(self: &Rc<Self>, requested: &str, detail: Option<&str>) {
        let state = normalize_state(requested);
        let previous_state = self.current_state.borrow().clone();
        if previous_state == state {
            if let Some(detail) = detail {
                self.status.set_text(detail);
            }
            return;
        }
        self.current_state.replace(state.to_string());
        if state == "offline" {
            self.runtime.clear_codex();
            self.schedule_presentation_reset();
        } else {
            self.cancel_presentation_reset();
            self.request_presentation(self.preferred_presentation.get());
        }
        self.stop_animation();
        self.status
            .set_text(detail.unwrap_or_else(|| state_label(state)));
        self.fallback.set_text(state_emoji(state));

        let animation = self
            .config
            .animations
            .get(state)
            .or_else(|| self.config.animations.get("default"));
        let textures = animation
            .map(|value| self.load_frames(state, value))
            .unwrap_or_default();
        self.frames.replace(textures);
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
            self.apply_animation_policy(self.runtime.snapshot().animation_policy);
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
                self.root.error_bell();
            }
        }
    }

    fn load_frames(&self, state: &str, animation: &AnimationConfig) -> Vec<gdk::Texture> {
        if let Some(frames) = self.frame_cache.borrow().get(state) {
            return frames.clone();
        }
        let frames = animation
            .frames
            .iter()
            .filter_map(|path| {
                let path = resolve_path(self.config.asset_root.as_deref(), path);
                match gdk::Texture::from_file(&gio::File::for_path(&path)) {
                    Ok(texture) => {
                        crate::core::power_debug::increment(
                            crate::core::power_debug::Counter::ImageDecode,
                        );
                        Some(texture)
                    }
                    Err(error) => {
                        tracing::warn!(?path, %error, "failed to load pet frame");
                        None
                    }
                }
            })
            .collect::<Vec<_>>();
        self.frame_cache
            .borrow_mut()
            .insert(state.to_string(), frames.clone());
        let mut order = self.cache_order.borrow_mut();
        order.retain(|key| key != state);
        order.push_back(state.to_string());
        while order.len() > 3 {
            if let Some(old) = order.pop_front() {
                if old != state {
                    self.frame_cache.borrow_mut().remove(&old);
                }
            }
        }
        frames
    }

    fn start_animation(self: &Rc<Self>, animation: &AnimationConfig, cap: Option<u32>) {
        self.stop_animation();
        if !self.root.is_mapped() {
            return;
        }
        let mut fps = animation.fps.unwrap_or(self.config.fps).clamp(1, 12);
        if let Some(cap) = cap {
            fps = fps.min(cap.max(1));
        }
        let interval = Duration::from_millis((1000 / fps) as u64);
        let looping = animation.r#loop;
        let pause_when_unmapped = self.config.pause_when_unmapped;
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local(interval, move || {
            let Some(runtime) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if pause_when_unmapped && !runtime.root.is_mapped() {
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

    fn watch_runtime_mode(this: &Rc<Self>) {
        let rx = this.runtime.subscribe();
        let weak = Rc::downgrade(this);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(snapshot) = rx.recv().await {
                let Some(runtime) = weak.upgrade() else {
                    break;
                };
                runtime.apply_animation_policy(snapshot.animation_policy);
            }
        });
    }

    fn watch_mapping(this: &Rc<Self>) {
        let weak = Rc::downgrade(this);
        this.root.connect_map(move |_| {
            if let Some(runtime) = weak.upgrade() {
                runtime.apply_animation_policy(runtime.runtime.snapshot().animation_policy);
            }
        });
        let weak = Rc::downgrade(this);
        this.root.connect_unmap(move |_| {
            if let Some(runtime) = weak.upgrade() {
                runtime.stop_animation();
            }
        });
    }

    fn apply_animation_policy(self: &Rc<Self>, policy: AnimationPolicy) {
        let should_animate = self.root.is_mapped()
            && self.frames.borrow().len() > 1
            && matches!(
                policy,
                AnimationPolicy::Normal | AnimationPolicy::Reduced(_)
            );
        if self.animation_policy.get() == policy
            && ((should_animate && self.animation_source.borrow().is_some()) || !should_animate)
        {
            return;
        }
        self.animation_policy.set(policy);
        self.stop_animation();
        if !self.root.is_mapped()
            || matches!(policy, AnimationPolicy::Stopped | AnimationPolicy::Frozen)
        {
            return;
        }
        let state = self.current_state.borrow().clone();
        let Some(animation) = self
            .config
            .animations
            .get(&state)
            .or_else(|| self.config.animations.get("default"))
        else {
            return;
        };
        if self.frames.borrow().len() < 2 {
            return;
        }
        let cap = match policy {
            AnimationPolicy::Normal => None,
            AnimationPolicy::Reduced(fps) => Some(fps),
            AnimationPolicy::Frozen | AnimationPolicy::Stopped => return,
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
        self.stop_animation();
        if let Some(source) = self.offline_source.borrow_mut().take() {
            source.remove();
        }
        if let Some(source) = self.transition_source.borrow_mut().take() {
            source.remove();
        }
        self.cancel_presentation_reset();
        self.monitor.borrow_mut().take();
        self.frame_cache.borrow_mut().clear();
        self.cache_order.borrow_mut().clear();
    }
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
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "presentation file has no parent",
        ));
    };
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, format!("{}\n", presentation_name(presentation)))?;
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

fn is_working_state(state: &str) -> bool {
    matches!(state, "thinking" | "working" | "coding")
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

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::{completion_sound_argv, next_presentation, parse_presentation, presentation_name};
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
