use gtk::prelude::*;
use gtk::{Align, Box as GtkBox, Button, Image, Label, Orientation};

use crate::model::card_model::{CardModel, CardState, RendererKind};
use crate::rendering::{
    action::ActionWidgets, composite::CompositeWidgets, list::ListWidgets,
    progress::ProgressWidgets, status::StatusWidgets, text::TextWidgets, value::ValueWidgets,
};

#[derive(Debug, Clone, Copy)]
pub struct CardLayout {
    pub width: Option<i32>,
    pub height: i32,
    pub fixed: bool,
}

pub enum RenderWidgets {
    Text(TextWidgets),
    Value(ValueWidgets),
    Progress(ProgressWidgets),
    Status(StatusWidgets),
    List(ListWidgets),
    Composite(CompositeWidgets),
    Action(ActionWidgets),
}

pub struct MetricCard {
    pub card: GtkBox,
    pub header_description: Label,
    header_icon: Image,
    pub refresh_btn: Button,
    pub render_widgets: RenderWidgets,
    value_box: GtkBox,
    state_label: Label,
    pub footer: Label,
    pub model: Option<CardModel>,
    pub renderer_kind: RendererKind,
}

impl MetricCard {
    pub fn new(model: &CardModel, layout: CardLayout) -> Self {
        let card = GtkBox::new(Orientation::Vertical, 0);
        card.add_css_class("card");
        card.add_css_class("pulsedeck-card");
        card.add_css_class("metric-card");
        card.set_valign(Align::Fill);
        card.set_halign(Align::Fill);
        card.set_hexpand(true);
        card.set_vexpand(!layout.fixed);
        card.set_size_request(layout.width.unwrap_or(-1), layout.height);
        card.set_overflow(gtk::Overflow::Hidden);

        let accent = accent_for_card(&model.id, &model.renderer);
        card.add_css_class(accent);

        // CenterBox keeps the title at the geometric center of the card even
        // when the icon and refresh button have different natural widths.
        let header = gtk::CenterBox::new();

        let icon_name = model.icon.as_deref().unwrap_or("computer-symbolic");
        let header_icon = Image::from_icon_name(icon_name);
        header_icon.set_pixel_size(18);
        header_icon.set_valign(Align::Center);
        header_icon.add_css_class("metric-header-icon");
        header.set_start_widget(Some(&header_icon));

        let text_box = GtkBox::new(Orientation::Vertical, 0);
        text_box.set_hexpand(true);
        text_box.set_valign(Align::Center);

        let header_name = Label::new(Some(&model.title));
        header_name.set_halign(Align::Center);
        header_name.set_hexpand(true);
        header_name.add_css_class("metric-header-name");
        header_name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text_box.append(&header_name);

        // The configured description is static card metadata.  Keep it apart
        // from the footer, which is reserved for live metric details.  The old
        // implementation reused `subtitle` for both and erased the description
        // as soon as the first metric result arrived.
        let header_description = Label::new(model.subtitle.as_deref());
        header_description.set_halign(Align::Center);
        header_description.set_hexpand(true);
        header_description.add_css_class("metric-header-sub");
        header_description.set_ellipsize(gtk::pango::EllipsizeMode::End);
        header_description.set_lines(1);
        header_description.set_visible(
            model
                .subtitle
                .as_deref()
                .is_some_and(|description| !description.is_empty()),
        );
        text_box.append(&header_description);

        header.set_center_widget(Some(&text_box));

        let refresh_btn = Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_valign(Align::Center);
        refresh_btn.add_css_class("flat");
        refresh_btn.set_tooltip_text(Some("刷新"));
        header.set_end_widget(Some(&refresh_btn));

        card.append(&header);

        let value_box = GtkBox::new(Orientation::Vertical, 1);
        value_box.add_css_class("metric-value-box");
        value_box.set_vexpand(true);
        value_box.set_valign(Align::Center);

        let render_widgets = match model.renderer {
            RendererKind::Value => {
                let w = ValueWidgets::new();
                value_box.append(&w.container);
                RenderWidgets::Value(w)
            }
            RendererKind::Text => {
                let w = TextWidgets::new();
                value_box.append(&w.value);
                RenderWidgets::Text(w)
            }
            RendererKind::Progress => {
                let w = ProgressWidgets::new();
                value_box.append(&w.value);
                value_box.append(&w.bar);
                RenderWidgets::Progress(w)
            }
            RendererKind::Status => {
                let w = StatusWidgets::new();
                value_box.append(&w.value);
                RenderWidgets::Status(w)
            }
            RendererKind::List => {
                let w = ListWidgets::new();
                value_box.append(&w.value);
                RenderWidgets::List(w)
            }
            RendererKind::Composite => {
                let w = CompositeWidgets::new(6);
                for row in &w.rows {
                    value_box.append(&row.container);
                }
                RenderWidgets::Composite(w)
            }
            RendererKind::Action => {
                let w = ActionWidgets::new("执行");
                let btn_row = GtkBox::new(Orientation::Horizontal, 8);
                btn_row.set_halign(Align::Center);
                btn_row.set_margin_top(8);
                btn_row.append(&w.spinner);
                btn_row.append(&w.button);
                value_box.append(&btn_row);
                value_box.append(&w.status);
                RenderWidgets::Action(w)
            }
        };

        let state_label = Label::new(None);
        state_label.set_halign(Align::Center);
        state_label.set_hexpand(true);
        state_label.add_css_class("metric-value");
        state_label.set_visible(false);
        value_box.append(&state_label);

        if layout.fixed {
            // GtkWidget's size request is a minimum, not a maximum. Without a
            // non-propagating viewport, multiline plugin output contributes its
            // natural height and stretches every FlowBox child in that row.
            let value_viewport = gtk::ScrolledWindow::new();
            value_viewport.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
            value_viewport.set_propagate_natural_height(false);
            value_viewport.set_propagate_natural_width(false);
            value_viewport.set_min_content_height(0);
            value_viewport.set_vexpand(true);
            value_viewport.set_child(Some(&value_box));
            card.append(&value_viewport);
        } else {
            card.append(&value_box);
        }

        let footer = Label::new(None);
        footer.set_halign(Align::Center);
        footer.set_hexpand(true);
        footer.add_css_class("metric-footer");
        footer.set_wrap(true);
        footer.set_lines(2);
        footer.set_ellipsize(gtk::pango::EllipsizeMode::End);
        footer.set_valign(Align::End);
        footer.set_visible(false);
        card.append(&footer);

        let mut result = Self {
            card,
            header_description,
            header_icon,
            refresh_btn,
            render_widgets,
            value_box,
            state_label,
            footer,
            model: Some(model.clone()),
            renderer_kind: model.renderer,
        };
        result.set_model(model);
        result
    }

    pub fn set_model(&mut self, model: &CardModel) {
        let fallback = match &model.value {
            crate::model::card_model::CardValue::Text(value)
                if value.lines().count() > 4 || value.chars().count() > 48 =>
            {
                Some(value.as_str())
            }
            _ => None,
        };
        self.card
            .set_tooltip_text(model.tooltip.as_deref().or(fallback));
        self.set_renderer_visible(matches!(model.state, CardState::Normal | CardState::Cached));
        if matches!(model.state, CardState::Normal | CardState::Cached) {
            match &mut self.render_widgets {
                RenderWidgets::Text(w) => crate::rendering::text::apply_text(w, model),
                RenderWidgets::Value(w) => crate::rendering::value::apply_value(w, model),
                RenderWidgets::Progress(w) => crate::rendering::progress::apply_progress(w, model),
                RenderWidgets::Status(w) => crate::rendering::status::apply_status(w, model),
                RenderWidgets::List(w) => crate::rendering::list::apply_list(w, model),
                RenderWidgets::Composite(w) => {
                    crate::rendering::composite::apply_composite(w, model)
                }
                RenderWidgets::Action(w) => crate::rendering::action::apply_action(w, model),
            }
        } else {
            self.state_label.set_label(match model.state {
                CardState::Loading => "加载中...",
                CardState::Unavailable => "不可用",
                CardState::Error => "错误",
                CardState::Normal | CardState::Cached => "",
            });
        }

        if let Some(ref sub) = model.subtitle {
            if !sub.is_empty() {
                self.footer.set_label(sub);
                self.footer.set_visible(true);
            } else {
                self.footer.set_visible(false);
            }
        } else {
            self.footer.set_visible(false);
        }

        if model.cached {
            let label = model
                .subtitle
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(|value| format!("{value} · 缓存"))
                .unwrap_or_else(|| "缓存".into());
            self.footer.set_label(&label);
            self.footer.set_visible(true);
        }
        // Preserve static identity and layout metadata. Metric updates carry
        // only dynamic fields and intentionally leave these values empty.
        let mut stored = model.clone();
        if let Some(previous) = &self.model {
            stored.id = previous.id.clone();
            stored.title = previous.title.clone();
            stored.icon = previous.icon.clone();
            stored.columns_after = previous.columns_after;
            stored.columns = previous.columns;
        }
        self.model = Some(stored);
        self.apply_density(model);
    }

    pub fn set_compact(&mut self, compact: bool) {
        self.header_icon.set_visible(!compact);
        self.refresh_btn.set_visible(!compact);
        let description_visible = !self.header_description.text().is_empty();
        self.header_description.set_visible(description_visible);
        if compact {
            self.card.add_css_class("compact-card");
        } else {
            self.card.remove_css_class("compact-card");
        }
        let footer_visible = self.model.as_ref().is_some_and(|model| {
            model.cached
                || model
                    .subtitle
                    .as_deref()
                    .is_some_and(|subtitle| !subtitle.is_empty())
        });
        self.footer.set_visible(footer_visible);
    }

    fn apply_density(&self, model: &CardModel) {
        let (mut chars, mut lines) = value_complexity(&model.value);
        if let Some(subtitle) = model.subtitle.as_deref() {
            chars += subtitle.chars().count();
            lines += subtitle.lines().count();
        }
        self.card.remove_css_class("content-medium");
        self.card.remove_css_class("content-dense");
        if chars > 48 || lines > 4 {
            self.card.add_css_class("content-dense");
        } else if chars > 20 || lines > 2 {
            self.card.add_css_class("content-medium");
        }
    }

    fn set_renderer_visible(&self, visible: bool) {
        self.state_label.set_visible(!visible);
        match &self.render_widgets {
            RenderWidgets::Text(w) => w.value.set_visible(visible),
            RenderWidgets::Value(w) => w.container.set_visible(visible),
            RenderWidgets::Progress(w) => {
                w.value.set_visible(visible);
                w.bar.set_visible(visible);
            }
            RenderWidgets::Status(w) => w.value.set_visible(visible),
            RenderWidgets::List(w) => w.value.set_visible(visible),
            RenderWidgets::Composite(w) => {
                for row in &w.rows {
                    row.container.set_visible(visible);
                }
            }
            RenderWidgets::Action(w) => {
                w.button.set_visible(visible);
                w.status.set_visible(visible);
                if !visible {
                    w.spinner.stop();
                    w.spinner.set_visible(false);
                }
            }
        }
        self.value_box.set_visible(true);
    }

    pub fn set_refresh_pending(&self, pending: bool) {
        self.refresh_btn.set_sensitive(!pending);
        self.refresh_btn
            .set_tooltip_text(Some(if pending { "正在刷新" } else { "刷新" }));
    }

    pub fn set_action_enabled(&self, enabled: bool) {
        if let RenderWidgets::Action(widgets) = &self.render_widgets {
            widgets.set_enabled(enabled);
        }
    }

    pub fn set_action_running(&self, running: bool) {
        if let RenderWidgets::Action(widgets) = &self.render_widgets {
            widgets.set_running(running);
        }
    }

    pub fn set_value_level(&self, level: Option<&str>) {
        let value = match &self.render_widgets {
            RenderWidgets::Text(w) => &w.value,
            RenderWidgets::Value(w) => &w.value,
            RenderWidgets::Progress(w) => &w.value,
            RenderWidgets::Status(w) => &w.value,
            RenderWidgets::List(w) => &w.value,
            RenderWidgets::Composite(_) | RenderWidgets::Action(_) => return,
        };
        value.remove_css_class("metric-value-good");
        value.remove_css_class("metric-value-warning");
        value.remove_css_class("metric-value-critical");
        match level {
            Some("good") => value.add_css_class("metric-value-good"),
            Some("warning") => value.add_css_class("metric-value-warning"),
            Some("critical") | Some("error") => value.add_css_class("metric-value-critical"),
            _ => {}
        }
    }
}

fn value_complexity(value: &crate::model::card_model::CardValue) -> (usize, usize) {
    use crate::model::card_model::CardValue;
    fn text_size(value: &str) -> (usize, usize) {
        (value.chars().count(), value.lines().count().max(1))
    }
    match value {
        CardValue::Text(value) => text_size(value),
        CardValue::Number { unit, .. } => (12 + unit.as_deref().map(str::len).unwrap_or(0), 1),
        CardValue::Percentage(_) => (8, 1),
        CardValue::Status { label, .. } => text_size(label),
        CardValue::List(items) => (
            items
                .iter()
                .map(|item| item.label.chars().count() + item.value.chars().count() + 2)
                .sum(),
            items.len().max(1),
        ),
        CardValue::Composite(fields) => (
            fields
                .iter()
                .map(|field| field.label.chars().count() + field.value.chars().count() + 1)
                .sum(),
            fields.len().max(1),
        ),
        _ => (0, 1),
    }
}

fn accent_for_card(card_id: &str, renderer: &RendererKind) -> &'static str {
    match card_id {
        "cpu" | "memory" => "accent-blue",
        "battery" => "accent-green",
        "battery-temp" => "accent-orange",
        "power" => "accent-purple",
        "network-status" | "network" => "accent-teal",
        _ => match renderer {
            RendererKind::Progress => "accent-blue",
            RendererKind::Status => "accent-green",
            RendererKind::Value => "accent-purple",
            _ => "accent-teal",
        },
    }
}
