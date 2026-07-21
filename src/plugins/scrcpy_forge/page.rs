//! Configuration-driven external device-service page.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    rc::Rc,
    time::Duration,
};

use gtk::prelude::*;
use gtk::{
    Align, Box as GtkBox, Button, DropDown, Entry, FlowBox, Label, Orientation, Picture, Switch,
};

use super::config::{CardConfig, PageConfig as ScrcpyForgeConfig};
use super::service::{Client, DaemonController, Device, Snapshot};
use crate::core::config::PageConfig;

pub fn build(handle: tokio::runtime::Handle, page: &PageConfig) -> gtk::ScrolledWindow {
    let cfg = page.scrcpy_forge.clone().unwrap_or_default();
    let mut cards = if cfg.cards.is_empty() {
        default_cards()
    } else {
        cfg.cards.clone()
    };
    cards.retain(|card| card.enabled);
    cards.sort_by_key(|card| card.order);

    let flow = FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_min_children_per_line(1);
    flow.set_max_children_per_line(cfg.columns.max(1));
    // The preview, backend controls and script controls have very different
    // natural heights. Equal-height FlowBox children made short cards contain
    // large empty areas whenever they shared a row with a preview.
    flow.set_homogeneous(false);
    flow.set_row_spacing(6);
    flow.set_column_spacing(6);
    flow.set_margin_top(4);
    flow.set_margin_bottom(4);
    flow.set_margin_start(4);
    flow.set_margin_end(4);
    flow.set_halign(Align::Center);
    flow.set_valign(Align::Start);

    let client = Client::new(&cfg);
    let daemon = DaemonController::default();
    let mut devices_cfg = None;
    let mut scripts_cfg = None;

    for card in cards {
        match card.role.as_str() {
            "backend" => flow.append(&backend_card(
                &card,
                &cfg,
                client.clone(),
                daemon.clone(),
                handle.clone(),
            )),
            "devices" => {
                devices_cfg = Some(card);
            }
            "scripts" => {
                scripts_cfg = Some(card);
            }
            role => tracing::warn!(role, "unknown ScrcpyForge card role"),
        }
    }

    let device_views = Rc::new(RefCell::new(HashMap::<String, DevicePairWidgets>::new()));
    let scripts_signature = Rc::new(Cell::new(0_u64));
    let preview_height = cfg.preview_height.max(1);
    let preview_width = (cfg.card_width - 20).max(1);

    let (tx, rx) = async_channel::unbounded();
    let busy = Rc::new(Cell::new(false));
    let request = {
        let client = client.clone();
        let handle = handle.clone();
        let tx = tx.clone();
        move || {
            let client = client.clone();
            let tx = tx.clone();
            handle.spawn(async move {
                let _ = tx.try_send(client.snapshot().await);
            });
        }
    };
    request();
    busy.set(true);
    glib::timeout_add_local(Duration::from_secs(cfg.preview_interval_seconds.max(1)), {
        let busy = busy.clone();
        let visible_host = flow.clone();
        move || {
            if visible_host.is_mapped() && !busy.replace(true) {
                request();
            }
            glib::ControlFlow::Continue
        }
    });
    let update_flow = flow.clone();
    glib::MainContext::default().spawn_local(async move {
        while let Ok(result) = rx.recv().await {
            busy.set(false);
            match result {
                Ok(snapshot) => render_snapshot(
                    &update_flow,
                    &device_views,
                    &scripts_signature,
                    snapshot,
                    &client,
                    &handle,
                    devices_cfg.as_ref(),
                    scripts_cfg.as_ref(),
                    &cfg,
                    preview_width,
                    preview_height,
                ),
                Err(error) => {
                    scripts_signature.set(0);
                    tracing::warn!(%error, "ScrcpyForge snapshot failed");
                }
            }
        }
    });

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&flow));
    scroll
}

fn backend_card(
    card: &CardConfig,
    cfg: &ScrcpyForgeConfig,
    client: Client,
    daemon: DaemonController,
    handle: tokio::runtime::Handle,
) -> GtkBox {
    let card_shell = shell(card, cfg);
    let card_box = card_shell.root;
    let body = card_shell.body;
    let status = Label::new(Some("检查中…"));
    status.set_halign(Align::Start);
    status.set_hexpand(true);
    let toggle = Switch::new();
    let row = GtkBox::new(Orientation::Horizontal, 8);
    row.append(&status);
    row.append(&toggle);
    body.append(&row);
    let endpoint = Entry::new();
    endpoint.set_placeholder_text(Some("设备地址:端口"));
    endpoint.set_hexpand(true);
    let connect = Button::with_label("连接无线设备");
    let connect_row = GtkBox::new(Orientation::Vertical, 5);
    connect_row.set_hexpand(true);
    endpoint.set_width_chars(1);
    connect.set_hexpand(true);
    connect_row.append(&endpoint);
    connect_row.append(&connect);
    body.append(&connect_row);
    let (connect_tx, connect_rx) = async_channel::unbounded();
    connect.connect_clicked({
        let client = client.clone();
        let handle = handle.clone();
        let endpoint = endpoint.clone();
        let connect_tx = connect_tx.clone();
        move |_| {
            let value = endpoint.text().trim().to_owned();
            if value.is_empty() {
                return;
            }
            let client = client.clone();
            let tx = connect_tx.clone();
            handle.spawn(async move {
                let _ = tx.try_send(
                    client
                        .connect(&value)
                        .await
                        .map(|_| format!("已连接 {value}"))
                        .map_err(|e| e.to_string()),
                );
            });
        }
    });
    glib::MainContext::default().spawn_local({
        let status = status.clone();
        async move {
            while let Ok(result) = connect_rx.recv().await {
                status.set_text(&result.unwrap_or_else(|e| format!("连接失败：{e}")));
            }
        }
    });
    let changing = Rc::new(Cell::new(false));
    toggle.connect_active_notify({
        let daemon = daemon.clone();
        let status = status.clone();
        let cfg = cfg.clone();
        let changing = changing.clone();
        let client = client.clone();
        let handle = handle.clone();
        move |toggle| {
            if changing.get() {
                return;
            }
            if toggle.is_active() {
                if let Err(error) = daemon.start(&cfg.daemon_program, &cfg.daemon_args) {
                    status.set_text(&format!("启动失败：{error}"));
                    changing.set(true);
                    toggle.set_active(false);
                    changing.set(false);
                } else {
                    status.set_text("正在启动…");
                }
            } else {
                status.set_text("正在停止…");
                daemon.stop();
                let client = client.clone();
                handle.spawn(async move {
                    let _ = client.shutdown().await;
                });
            }
        }
    });
    let (tx, rx) = async_channel::unbounded();
    let health_busy = Rc::new(Cell::new(false));
    glib::timeout_add_local(
        Duration::from_secs(cfg.health_interval_seconds.max(1)),
        move || {
            if let Ok(healthy) = rx.try_recv() {
                health_busy.set(false);
                status.set_text(if healthy { "运行中" } else { "已停止" });
                changing.set(true);
                toggle.set_active(healthy);
                changing.set(false);
            }
            if status.is_mapped() && !health_busy.replace(true) {
                let client = client.clone();
                let tx = tx.clone();
                handle.spawn(async move {
                    let _ = tx.try_send(client.healthy().await);
                });
            }
            glib::ControlFlow::Continue
        },
    );
    card_box
}

fn render_snapshot(
    flow: &FlowBox,
    device_views: &Rc<RefCell<HashMap<String, DevicePairWidgets>>>,
    scripts_signature: &Rc<Cell<u64>>,
    snapshot: Snapshot,
    client: &Client,
    handle: &tokio::runtime::Handle,
    devices_config: Option<&CardConfig>,
    scripts_config: Option<&CardConfig>,
    page_config: &ScrcpyForgeConfig,
    preview_width: i32,
    preview_height: i32,
) {
    let serials: std::collections::HashSet<&str> = snapshot
        .devices
        .iter()
        .map(|(device, _)| device.serial.as_str())
        .collect();
    let mut views = device_views.borrow_mut();
    let removed: Vec<String> = views
        .keys()
        .filter(|serial| !serials.contains(serial.as_str()))
        .cloned()
        .collect();
    for serial in removed {
        if let Some(pair) = views.remove(&serial) {
            flow.remove(&pair.preview_card);
            flow.remove(&pair.scripts_card);
        }
    }
    for (device, png) in &snapshot.devices {
        let Some(preview_config) = devices_config else {
            continue;
        };
        let Some(script_config) = scripts_config else {
            continue;
        };
        let pair = views.entry(device.serial.clone()).or_insert_with(|| {
            let preview = DevicePreviewWidgets::new(preview_width, preview_height);
            let preview_shell = shell(preview_config, page_config);
            preview_shell.body.append(&preview.root);
            let script_shell = shell(script_config, page_config);
            flow.append(&preview_shell.root);
            flow.append(&script_shell.root);
            DevicePairWidgets {
                preview,
                preview_card: preview_shell.root,
                scripts_card: script_shell.root,
                scripts_body: script_shell.body,
            }
        });
        pair.preview
            .update(device, png.as_deref(), snapshot.metrics.get(&device.serial));
    }

    let signature = script_state_signature(&snapshot);
    if signature != scripts_signature.get() {
        scripts_signature.set(signature);
        for (device, _) in &snapshot.devices {
            if let Some(pair) = views.get(&device.serial) {
                clear(&pair.scripts_body);
                pair.scripts_body.append(&script_controls(
                    device.clone(),
                    &snapshot,
                    client.clone(),
                    handle.clone(),
                ));
            }
        }
    }
}

struct DevicePairWidgets {
    preview: DevicePreviewWidgets,
    preview_card: GtkBox,
    scripts_card: GtkBox,
    scripts_body: GtkBox,
}

struct DevicePreviewWidgets {
    root: GtkBox,
    picture: Picture,
    label: Label,
    details: Label,
    frame_hash: Cell<u64>,
}
impl DevicePreviewWidgets {
    fn new(preview_width: i32, preview_height: i32) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 5);
        let picture = Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_halign(Align::Fill);
        picture.set_hexpand(true);
        // Image textures report their pixel dimensions as natural size. Put the
        // picture behind a non-propagating viewport so a 1080px frame cannot
        // widen the FlowBox card beyond its configured logical width.
        let picture_view = gtk::ScrolledWindow::new();
        picture_view.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
        picture_view.set_propagate_natural_width(false);
        picture_view.set_propagate_natural_height(false);
        picture_view.set_size_request(preview_width, preview_height);
        picture_view.set_child(Some(&picture));
        root.append(&picture_view);
        let label = Label::new(None);
        label.set_wrap(true);
        label.set_max_width_chars(20);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        root.append(&label);
        let details = Label::new(None);
        details.add_css_class("settings-desc");
        details.set_wrap(true);
        details.set_max_width_chars(24);
        root.append(&details);
        Self {
            root,
            picture,
            label,
            details,
            frame_hash: Cell::new(0),
        }
    }
    fn update(
        &self,
        device: &Device,
        png: Option<&[u8]>,
        metrics: Option<&super::service::SessionMetrics>,
    ) {
        if let Some(png) = png {
            let mut hasher = DefaultHasher::new();
            png.hash(&mut hasher);
            let hash = hasher.finish();
            if hash != self.frame_hash.get() {
                if let Ok(texture) = gtk::gdk::Texture::from_bytes(&glib::Bytes::from(png)) {
                    self.picture.set_paintable(Some(&texture));
                    self.frame_hash.set(hash);
                }
            }
        } else {
            self.picture.set_paintable(gtk::gdk::Paintable::NONE);
            self.frame_hash.set(0);
        }
        let state = if device.state == "device" {
            "在线"
        } else {
            "ADB 离线，预览已暂停"
        };
        self.label.set_text(&format!(
            "{} · {state}",
            device.model.as_deref().unwrap_or(&device.serial)
        ));
        self.label.set_tooltip_text(Some(&device.serial));
        self.details.set_visible(metrics.is_some());
        if let Some(m) = metrics {
            self.details.set_text(&format!("解码 {:.1} · 预览 {:.1} · 脚本 {:.1} FPS · 帧龄 {:.0}ms\n延迟均值 {:.1} · P50 {:.1} / P95 {:.1} ms · 丢帧 {}",m.decoded_fps,m.preview_fps,m.script_fps,m.latest_frame_age_ms,m.average_script_ms,m.script_p50_ms,m.script_p95_ms,m.dropped_script_frames));
        }
    }
}

fn script_state_signature(snapshot: &Snapshot) -> u64 {
    let mut h = DefaultHasher::new();
    snapshot.scripts.hash(&mut h);
    snapshot.sessions.hash(&mut h);
    for (d, _) in &snapshot.devices {
        d.serial.hash(&mut h);
        d.state.hash(&mut h);
        if let Some(m) = snapshot.metrics.get(&d.serial) {
            m.profile.hash(&mut h);
            m.preview_profile.hash(&mut h);
        }
    }
    for r in &snapshot.runs {
        r.serial.hash(&mut h);
        r.name.hash(&mut h);
        r.running.hash(&mut h);
        r.stalled.hash(&mut h);
        r.error.hash(&mut h);
    }
    h.finish()
}

fn script_controls(
    device: Device,
    snapshot: &Snapshot,
    client: Client,
    handle: tokio::runtime::Handle,
) -> GtkBox {
    let box_ = GtkBox::new(Orientation::Vertical, 7);
    let device_serial = device.serial.clone();
    let profile_client = client.clone();
    let profile_handle = handle.clone();
    let name = Label::new(Some(device.model.as_deref().unwrap_or(&device.serial)));
    name.set_halign(Align::Start);
    name.set_max_width_chars(20);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    box_.append(&name);
    let scripts = Rc::new(snapshot.scripts.clone());
    let script_labels: Vec<String> = scripts.iter().map(|name| compact_label(name, 18)).collect();
    let script_refs: Vec<&str> = script_labels.iter().map(String::as_str).collect();
    let combo = DropDown::from_strings(&script_refs);
    box_.append(&combo);
    let active = snapshot
        .runs
        .iter()
        .find(|run| run.serial == device.serial && run.running);
    let stopping = active.is_some();
    let last_error = snapshot
        .runs
        .iter()
        .find(|run| run.serial == device.serial && !run.running)
        .and_then(|run| run.error.as_deref());
    let status_text = active
        .and_then(|run| run.name.as_deref())
        .map(|name| {
            if active.is_some_and(|run| run.stalled) {
                format!("疑似卡顿 · {name}")
            } else {
                format!("运行中 · {name}")
            }
        })
        .or_else(|| last_error.map(|error| format!("已停止：{error}")))
        .unwrap_or_else(|| {
            if device.state == "device" {
                "已停止".into()
            } else {
                "设备离线".into()
            }
        });
    let status = Label::new(Some(&status_text));
    status.set_wrap(true);
    status.set_max_width_chars(20);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.set_lines(2);
    box_.append(&status);
    let button = Button::with_label(if stopping {
        "停止脚本"
    } else {
        "运行脚本"
    });
    button.set_sensitive(stopping || (device.state == "device" && !snapshot.scripts.is_empty()));
    let has_session = snapshot.sessions.contains(&device.serial);
    let action_serial = device_serial.clone();
    let (tx, rx) = async_channel::unbounded::<Result<String, String>>();
    button.connect_clicked({
        let status = status.clone();
        let rx = rx.clone();
        let scripts = scripts.clone();
        move |button| {
            button.set_sensitive(false);
            status.set_text("处理中…");
            let client = client.clone();
            let serial = action_serial.clone();
            let selected = scripts.get(combo.selected() as usize).cloned();
            let tx = tx.clone();
            handle.spawn(async move {
                let result = async {
                    if stopping {
                        client.stop_script(&serial).await?;
                        Ok("已停止".into())
                    } else if let Some(script) = selected {
                        if !has_session {
                            client.start_session(&serial).await?;
                        }
                        client.run_script(&serial, &script).await?;
                        Ok(format!("运行中 · {script}"))
                    } else {
                        anyhow::bail!("未选择脚本")
                    }
                }
                .await
                .map_err(|e: anyhow::Error| e.to_string());
                let _ = tx.try_send(result);
            });
            let status = status.clone();
            let button = button.clone();
            let rx = rx.clone();
            glib::MainContext::default().spawn_local(async move {
                if let Ok(result) = rx.recv().await {
                    match result {
                        Ok(message) => status.set_text(&message),
                        Err(error) => status.set_text(&format!("失败：{error}")),
                    }
                    button.set_sensitive(true);
                }
            });
        }
    });
    box_.append(&button);
    let profiles = GtkBox::new(Orientation::Vertical, 5);
    let script_profile = profile_combo(
        "脚本",
        snapshot
            .metrics
            .get(&device_serial)
            .map(|m| m.profile.as_str())
            .unwrap_or("auto"),
    );
    let preview_profile = profile_combo(
        "预览",
        snapshot
            .metrics
            .get(&device_serial)
            .map(|m| m.preview_profile.as_str())
            .unwrap_or("eco"),
    );
    script_profile.set_sensitive(has_session);
    preview_profile.set_sensitive(has_session);
    profiles.append(&script_profile);
    profiles.append(&preview_profile);
    box_.append(&profiles);
    script_profile.connect_selected_notify({
        let client = profile_client.clone();
        let handle = profile_handle.clone();
        let serial = device_serial.clone();
        move |combo| {
            let value = profile_id(combo.selected()).to_string();
            let client = client.clone();
            let serial = serial.clone();
            handle.spawn(async move {
                if let Err(error) = client.set_script_profile(&serial, &value).await {
                    tracing::warn!(%error,%serial,"failed to set script profile");
                }
            });
        }
    });
    preview_profile.connect_selected_notify({
        let client = profile_client;
        let handle = profile_handle;
        let serial = device_serial;
        move |combo| {
            let value = profile_id(combo.selected()).to_string();
            let client = client.clone();
            let serial = serial.clone();
            handle.spawn(async move {
                if let Err(error) = client.set_preview_profile(&serial, &value).await {
                    tracing::warn!(%error,%serial,"failed to set preview profile");
                }
            });
        }
    });
    box_
}

fn compact_label(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn profile_combo(prefix: &str, active: &str) -> DropDown {
    let labels = [
        format!("{prefix}：自动"),
        format!("{prefix}：节能"),
        format!("{prefix}：均衡"),
        format!("{prefix}：实时"),
    ];
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let combo = DropDown::from_strings(&refs);
    combo.set_selected(match active {
        "eco" => 1,
        "balanced" => 2,
        "realtime" => 3,
        _ => 0,
    });
    combo
}
fn profile_id(selected: u32) -> &'static str {
    match selected {
        1 => "eco",
        2 => "balanced",
        3 => "realtime",
        _ => "auto",
    }
}

struct CardShell {
    root: GtkBox,
    body: GtkBox,
}

fn shell(config: &CardConfig, page: &ScrcpyForgeConfig) -> CardShell {
    let card = GtkBox::new(Orientation::Vertical, 5);
    card.add_css_class("card");
    card.add_css_class("pulsedeck-card");
    card.add_css_class("metric-card");
    card.add_css_class("scrcpy-forge-card");
    // A compact portrait card close to the classic 63:88 trading-card ratio.
    // Keeping the card itself narrow avoids placing a phone-shaped preview in
    // the middle of a stretched landscape panel with empty space on both sides.
    card.set_size_request(page.card_width.max(1), page.card_height.max(1));
    card.set_valign(Align::Start);
    card.set_halign(Align::Center);
    card.set_hexpand(false);
    card.set_vexpand(false);
    let title = Label::new(Some(&config.title));
    title.set_halign(Align::Center);
    title.add_css_class("metric-header-name");
    card.append(&title);
    if let Some(description) = &config.description {
        let label = Label::new(Some(description));
        label.set_wrap(true);
        label.add_css_class("settings-desc");
        card.append(&label);
    }
    let body = GtkBox::new(Orientation::Vertical, 5);
    let body_view = gtk::ScrolledWindow::new();
    body_view.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    body_view.set_propagate_natural_width(false);
    body_view.set_propagate_natural_height(false);
    body_view.set_min_content_height(0);
    body_view.set_vexpand(true);
    body_view.set_child(Some(&body));
    card.append(&body_view);
    CardShell { root: card, body }
}
fn clear(box_: &GtkBox) {
    while let Some(child) = box_.first_child() {
        box_.remove(&child)
    }
}
fn default_cards() -> Vec<CardConfig> {
    vec![
        card("backend", "外部服务", 10),
        card("devices", "设备预览", 20),
        card("scripts", "设备脚本", 30),
    ]
}
fn card(role: &str, title: &str, order: i32) -> CardConfig {
    CardConfig {
        role: role.into(),
        title: title.into(),
        order,
        enabled: true,
        icon: None,
        description: None,
    }
}
