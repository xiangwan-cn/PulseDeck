use gio::prelude::FileExt;
use gtk::prelude::*;
use gtk::{Align, Box as GtkBox, Button, Image, Label, Orientation};
use regex::{Regex, RegexBuilder};

use crate::core::config::{
    CardBackgroundSvgConfig, CardColorsConfig, CardLogoSvgConfig, CardTransitionConfig,
    CardVisualStateConfig, DisplayConfig,
};
use crate::model::card_model::{CardModel, CardState, CardValue, RendererKind, StatusLevel};
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

struct VisualRule {
    config: CardVisualStateConfig,
    regex: Option<Regex>,
    css_class: String,
}

struct MatchedVisual {
    label: Option<String>,
    icon: Option<String>,
    css_class: String,
}

struct BackgroundSvgCss {
    image: String,
    position: &'static str,
    size: &'static str,
}

pub struct MetricCard {
    pub card: GtkBox,
    pub header_description: Label,
    header_icon: Image,
    logo_picture: gtk::Picture,
    compact_logo_picture: gtk::Picture,
    refresh_content: gtk::Stack,
    pub refresh_btn: Button,
    pub render_widgets: RenderWidgets,
    value_box: GtkBox,
    state_label: Label,
    pub footer: Label,
    pub model: Option<CardModel>,
    pub renderer_kind: RendererKind,
    base_icon: Option<String>,
    style_class: String,
    style_provider: Option<gtk::CssProvider>,
    customization: Option<DisplayConfig>,
    visual_rules: Vec<VisualRule>,
    active_state_class: Option<String>,
    compact: bool,
    logo_active: bool,
}

impl MetricCard {
    pub fn new(model: &CardModel, layout: CardLayout, display: Option<&DisplayConfig>) -> Self {
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
        let style_class = format!("card-theme-{:08x}", stable_hash(&model.id));
        card.add_css_class(&style_class);

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

        let refresh_icon = Image::from_icon_name("view-refresh-symbolic");

        let logo_picture = gtk::Picture::new();
        logo_picture.set_can_shrink(true);
        logo_picture.set_content_fit(gtk::ContentFit::Contain);
        logo_picture.set_can_target(false);
        logo_picture.set_focusable(false);
        logo_picture.add_css_class("card-logo-svg");

        let refresh_content = gtk::Stack::new();
        refresh_content.add_named(&refresh_icon, Some("refresh"));
        refresh_content.add_named(&logo_picture, Some("logo"));
        refresh_content.set_visible_child_name("refresh");

        let refresh_btn = Button::new();
        refresh_btn.set_child(Some(&refresh_content));
        refresh_btn.set_halign(Align::End);
        refresh_btn.set_valign(Align::Center);
        refresh_btn.add_css_class("flat");
        refresh_btn.set_tooltip_text(Some("刷新"));

        let compact_logo_picture = gtk::Picture::new();
        compact_logo_picture.set_halign(Align::End);
        compact_logo_picture.set_valign(Align::Start);
        compact_logo_picture.set_can_shrink(true);
        compact_logo_picture.set_content_fit(gtk::ContentFit::Contain);
        compact_logo_picture.set_can_target(false);
        compact_logo_picture.set_focusable(false);
        compact_logo_picture.set_visible(false);
        compact_logo_picture.add_css_class("card-logo-svg");

        // The refresh affordance is a true foreground overlay: it does not
        // participate in header measurement, so replacing its standard icon
        // with a custom logo cannot move the centered title. Compact mode hides
        // the button and shows a separate, non-interactive logo overlay instead.
        let header_overlay = gtk::Overlay::new();
        header_overlay.set_child(Some(&header));
        header_overlay.add_overlay(&refresh_btn);
        header_overlay.set_measure_overlay(&refresh_btn, false);
        header_overlay.add_overlay(&compact_logo_picture);
        header_overlay.set_measure_overlay(&compact_logo_picture, false);
        card.append(&header_overlay);

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
            logo_picture,
            compact_logo_picture,
            refresh_content,
            refresh_btn,
            render_widgets,
            value_box,
            state_label,
            footer,
            model: Some(model.clone()),
            renderer_kind: model.renderer,
            base_icon: model.icon.clone(),
            style_class,
            style_provider: None,
            customization: None,
            visual_rules: Vec::new(),
            active_state_class: None,
            compact: false,
            logo_active: false,
        };
        result.set_customization(display);
        result.set_model(model);
        result
    }

    pub fn set_model(&mut self, model: &CardModel) {
        let matched_rule = self.matching_rule(model);
        let mut display_model = model.clone();
        if let Some(rule) = &matched_rule {
            self.header_icon.set_icon_name(Some(
                rule.icon
                    .as_deref()
                    .or(self.base_icon.as_deref())
                    .unwrap_or("computer-symbolic"),
            ));
        } else {
            self.header_icon.set_icon_name(Some(
                self.base_icon.as_deref().unwrap_or("computer-symbolic"),
            ));
        }
        self.set_visual_state(
            matched_rule
                .as_ref()
                .map(|rule| rule.css_class.as_str())
                .unwrap_or_else(|| source_state_class(model.state)),
        );

        let fallback = match &display_model.value {
            crate::model::card_model::CardValue::Text(value)
                if value.lines().count() > 4 || value.chars().count() > 48 =>
            {
                Some(value.as_str())
            }
            _ => None,
        };
        self.card
            .set_tooltip_text(display_model.tooltip.as_deref().or(fallback));
        self.set_renderer_visible(matches!(
            display_model.state,
            CardState::Normal | CardState::Cached
        ));
        if matches!(display_model.state, CardState::Normal | CardState::Cached) {
            match &mut self.render_widgets {
                RenderWidgets::Text(w) => crate::rendering::text::apply_text(w, &display_model),
                RenderWidgets::Value(w) => crate::rendering::value::apply_value(w, &display_model),
                RenderWidgets::Progress(w) => {
                    crate::rendering::progress::apply_progress(w, &display_model)
                }
                RenderWidgets::Status(w) => {
                    crate::rendering::status::apply_status(w, &display_model)
                }
                RenderWidgets::List(w) => crate::rendering::list::apply_list(w, &display_model),
                RenderWidgets::Composite(w) => {
                    crate::rendering::composite::apply_composite(w, &display_model)
                }
                RenderWidgets::Action(w) => {
                    crate::rendering::action::apply_action(w, &display_model)
                }
            }
            if let Some(label) = matched_rule.as_ref().and_then(|rule| rule.label.as_deref()) {
                self.set_rendered_label(label);
                display_model.value = CardValue::Text(label.to_string());
            }
        } else {
            self.state_label.set_label(
                matched_rule
                    .as_ref()
                    .and_then(|rule| rule.label.as_deref())
                    .unwrap_or(match display_model.state {
                        CardState::Loading => "加载中...",
                        CardState::Unavailable => "不可用",
                        CardState::Error => "错误",
                        CardState::Normal | CardState::Cached => "",
                    }),
            );
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
        self.apply_density(&display_model);
    }

    /// Apply standard-card appearance and state rules. This is separate from
    /// layout so a value-only config reload can take effect without rebuilding
    /// the page hierarchy.
    pub fn set_customization(&mut self, display: Option<&DisplayConfig>) {
        if self.customization.as_ref() == display {
            return;
        }
        self.customization = display.cloned();
        self.set_logo_svg(display.and_then(|display| display.logo_svg.as_ref()));
        let Some(display) = display else {
            self.visual_rules.clear();
            if let Some(provider) = &self.style_provider {
                provider.load_from_data("");
            }
            return;
        };

        self.visual_rules = display
            .states
            .iter()
            .enumerate()
            .map(|(index, config)| VisualRule {
                regex: config.regex.as_deref().and_then(|pattern| {
                    RegexBuilder::new(pattern)
                        .case_insensitive(config.ignore_case)
                        .build()
                        .ok()
                }),
                css_class: format!("card-state-rule-{}-{}", index, css_fragment(&config.name)),
                config: config.clone(),
            })
            .collect();

        let background_svg = display.background_svg.as_ref().and_then(|background| {
            match background_svg_css(background) {
                Ok(background) => Some(background),
                Err(error) => {
                    tracing::warn!(
                        "card {} SVG background is unavailable: {error}",
                        self.model
                            .as_ref()
                            .map(|model| model.id.as_str())
                            .unwrap_or("unknown")
                    );
                    None
                }
            }
        });
        let css = appearance_css(
            &self.style_class,
            &display.colors,
            &self.visual_rules,
            display.transition.as_ref(),
            background_svg.as_ref(),
        );
        if css.is_empty() {
            if let Some(provider) = &self.style_provider {
                provider.load_from_data("");
            }
            return;
        }
        let provider = self.style_provider.get_or_insert_with(|| {
            let provider = gtk::CssProvider::new();
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
            provider
        });
        provider.load_from_data(&css);
    }

    fn set_logo_svg(&mut self, config: Option<&CardLogoSvgConfig>) {
        let Some(config) = config else {
            self.logo_picture.set_filename(None::<&std::path::Path>);
            self.compact_logo_picture
                .set_filename(None::<&std::path::Path>);
            self.refresh_content.set_visible_child_name("refresh");
            self.logo_active = false;
            self.refresh_btn.set_valign(Align::Center);
            self.update_refresh_visibility();
            return;
        };
        let path = match config.resolve_path(&crate::core::config::config_dir()) {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(
                    "card {} SVG logo is unavailable: {error}",
                    self.model
                        .as_ref()
                        .map(|model| model.id.as_str())
                        .unwrap_or("unknown")
                );
                self.logo_picture.set_filename(None::<&std::path::Path>);
                self.compact_logo_picture
                    .set_filename(None::<&std::path::Path>);
                self.refresh_content.set_visible_child_name("refresh");
                self.logo_active = false;
                self.refresh_btn.set_valign(Align::Center);
                self.update_refresh_visibility();
                return;
            }
        };
        self.logo_picture.set_filename(Some(&path));
        self.logo_picture.set_size_request(config.size, config.size);
        self.logo_picture.set_opacity(config.opacity);
        self.compact_logo_picture.set_filename(Some(&path));
        self.compact_logo_picture
            .set_size_request(config.size, config.size);
        self.compact_logo_picture.set_opacity(config.opacity);
        self.refresh_content.set_visible_child_name("logo");
        self.logo_active = true;
        self.refresh_btn.set_valign(Align::Start);
        self.update_refresh_visibility();
    }

    fn update_refresh_visibility(&self) {
        let (refresh_visible, compact_logo_visible) =
            refresh_and_logo_visibility(self.compact, self.logo_active);
        self.refresh_btn.set_visible(refresh_visible);
        self.compact_logo_picture.set_visible(compact_logo_visible);
    }

    fn matching_rule(&self, model: &CardModel) -> Option<MatchedVisual> {
        let text = value_text(&model.value);
        let number = value_number(&model.value);
        let level = value_level(&model.value);
        self.visual_rules
            .iter()
            .find(|rule| rule.matches(model.state, &text, number, level.as_ref()))
            .map(|rule| MatchedVisual {
                label: rule.config.label.clone(),
                icon: rule.config.icon.clone(),
                css_class: rule.css_class.clone(),
            })
    }

    fn set_visual_state(&mut self, class: &str) {
        if self.active_state_class.as_deref() == Some(class) {
            return;
        }
        if let Some(previous) = self.active_state_class.take() {
            self.card.remove_css_class(&previous);
        }
        self.card.add_css_class(class);
        self.active_state_class = Some(class.to_string());
    }

    fn set_rendered_label(&self, label: &str) {
        match &self.render_widgets {
            RenderWidgets::Text(w) => w.value.set_label(label),
            RenderWidgets::Value(w) => {
                w.grid.set_visible(false);
                w.value.set_visible(true);
                w.value.set_label(label);
            }
            RenderWidgets::Progress(w) => w.value.set_label(label),
            RenderWidgets::Status(w) => w.value.set_label(label),
            RenderWidgets::List(w) => w.value.set_label(label),
            RenderWidgets::Composite(_) => {}
            RenderWidgets::Action(w) => {
                w.status.set_label(label);
                w.status.set_visible(true);
            }
        }
    }

    pub fn set_compact(&mut self, compact: bool) {
        self.compact = compact;
        self.header_icon.set_visible(!compact);
        self.update_refresh_visibility();
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

impl VisualRule {
    fn matches(
        &self,
        source_state: CardState,
        text: &str,
        number: Option<f64>,
        level: Option<&StatusLevel>,
    ) -> bool {
        if self
            .config
            .source_state
            .is_some_and(|expected| expected != source_state)
        {
            return false;
        }
        if self
            .config
            .status_level
            .as_ref()
            .is_some_and(|expected| Some(expected) != level)
        {
            return false;
        }
        if self.config.min.is_some() || self.config.max.is_some() {
            let Some(value) = number else {
                return false;
            };
            if self.config.min.is_some_and(|minimum| value < minimum)
                || self.config.max.is_some_and(|maximum| value > maximum)
            {
                return false;
            }
        }

        let normalized_text;
        let comparable = if self.config.ignore_case {
            normalized_text = text.to_lowercase();
            normalized_text.as_str()
        } else {
            text
        };
        if let Some(expected) = self.config.equals.as_deref() {
            let normalized_expected;
            let expected = if self.config.ignore_case {
                normalized_expected = expected.to_lowercase();
                normalized_expected.as_str()
            } else {
                expected
            };
            if comparable != expected {
                return false;
            }
        }
        if let Some(expected) = self.config.contains.as_deref() {
            let normalized_expected;
            let expected = if self.config.ignore_case {
                normalized_expected = expected.to_lowercase();
                normalized_expected.as_str()
            } else {
                expected
            };
            if !comparable.contains(expected) {
                return false;
            }
        }
        if self.config.regex.is_some() {
            let Some(regex) = &self.regex else {
                return false;
            };
            if !regex.is_match(text) {
                return false;
            }
        }
        true
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

fn source_state_class(state: CardState) -> &'static str {
    match state {
        CardState::Normal => "card-state-normal",
        CardState::Loading => "card-state-loading",
        CardState::Unavailable => "card-state-unavailable",
        CardState::Error => "card-state-error",
        CardState::Cached => "card-state-cached",
    }
}

fn value_text(value: &CardValue) -> String {
    match value {
        CardValue::Text(value) => value.clone(),
        CardValue::Number { value, unit, .. } => unit
            .as_deref()
            .map(|unit| format!("{value}{unit}"))
            .unwrap_or_else(|| value.to_string()),
        CardValue::Percentage(value) => format!("{value}%"),
        CardValue::Status { label, .. } => label.clone(),
        CardValue::List(items) => items
            .iter()
            .map(|item| format!("{} {}", item.label, item.value))
            .collect::<Vec<_>>()
            .join("\n"),
        CardValue::Composite(fields) => fields
            .iter()
            .map(|field| format!("{} {}", field.label, field.value))
            .collect::<Vec<_>>()
            .join("\n"),
        CardValue::Empty => String::new(),
    }
}

fn value_number(value: &CardValue) -> Option<f64> {
    match value {
        CardValue::Number { value, .. } | CardValue::Percentage(value) => Some(*value),
        CardValue::Text(value) => value
            .trim()
            .trim_end_matches('%')
            .trim()
            .parse::<f64>()
            .ok(),
        _ => None,
    }
    .filter(|value| value.is_finite())
}

fn value_level(value: &CardValue) -> Option<StatusLevel> {
    match value {
        CardValue::Status { level, .. } => Some(level.clone()),
        _ => None,
    }
}

fn appearance_css(
    style_class: &str,
    base: &CardColorsConfig,
    rules: &[VisualRule],
    transition: Option<&CardTransitionConfig>,
    background_svg: Option<&BackgroundSvgCss>,
) -> String {
    let selector = format!(".{style_class}");
    let mut css = String::new();
    append_color_css(&mut css, &selector, base, background_svg, true);
    for rule in rules {
        append_color_css(
            &mut css,
            &format!("{selector}.{}", rule.css_class),
            &rule.config.colors,
            background_svg,
            false,
        );
    }
    if let Some(transition) = transition {
        let duration = transition.duration_ms.min(5_000);
        if duration > 0 {
            let easing = match transition.easing.as_str() {
                "linear" | "ease" | "ease-in" | "ease-out" | "ease-in-out" => {
                    transition.easing.as_str()
                }
                _ => "ease-out",
            };
            css.push_str(&format!(
                "{selector} {{ transition: background-color {duration}ms {easing}, border-color {duration}ms {easing}; }}\n\
                 {selector} .metric-value, {selector} .metric-header-name, {selector} .metric-header-icon, {selector} .metric-header-sub, {selector} .metric-footer {{ transition: color {duration}ms {easing}; }}\n\
                 {selector} levelbar block.filled {{ transition: background-color {duration}ms {easing}, border-color {duration}ms {easing}; }}\n"
            ));
        }
    }
    css
}

fn append_color_css(
    css: &mut String,
    selector: &str,
    colors: &CardColorsConfig,
    background_svg: Option<&BackgroundSvgCss>,
    include_svg_without_tint: bool,
) {
    if let Some(color) = colors.accent.as_deref().and_then(css_color) {
        css.push_str(&format!("{selector} {{ border-left-color: {color}; }}\n"));
    }
    append_foreground(css, &format!("{selector} .metric-value"), &colors.value);
    append_foreground(
        css,
        &format!("{selector} .metric-header-name"),
        &colors.title,
    );
    append_foreground(
        css,
        &format!("{selector} .metric-header-icon"),
        &colors.icon,
    );
    append_foreground(
        css,
        &format!("{selector} .metric-header-sub"),
        &colors.subtitle,
    );
    append_foreground(css, &format!("{selector} .metric-footer"), &colors.footer);
    if let Some(color) = colors.progress.as_deref().and_then(css_color) {
        css.push_str(&format!(
            "{selector} levelbar block.filled {{ background-color: {color}; border-color: {color}; }}\n"
        ));
    }

    append_background_css(
        css,
        selector,
        colors,
        background_svg,
        include_svg_without_tint,
    );
}

fn append_background_css(
    css: &mut String,
    selector: &str,
    colors: &CardColorsConfig,
    background_svg: Option<&BackgroundSvgCss>,
    include_svg_without_tint: bool,
) {
    let background: Vec<_> = colors
        .background
        .iter()
        .filter_map(|color| css_color(color))
        .collect();
    if background.is_empty() && !(include_svg_without_tint && background_svg.is_some()) {
        return;
    }
    let opacity = colors.background_opacity.unwrap_or(0.12).clamp(0.0, 1.0);
    let stops: Vec<_> = background
        .iter()
        .map(|color| format!("alpha({color}, {opacity:.3})"))
        .collect();

    let mut images = Vec::new();
    let mut repeats = Vec::new();
    let mut positions = Vec::new();
    let mut sizes = Vec::new();
    if let Some(background_svg) = background_svg {
        images.push(background_svg.image.clone());
        repeats.push("no-repeat");
        positions.push(background_svg.position);
        sizes.push(background_svg.size);
    }
    if stops.len() > 1 {
        images.push(format!(
            "linear-gradient(to bottom right, {})",
            stops.join(", ")
        ));
        repeats.push("no-repeat");
        positions.push("center");
        sizes.push("cover");
    }

    let mut declarations = Vec::new();
    match stops.as_slice() {
        [color] => declarations.push(format!("background-color: {color}")),
        [_, _, ..] => declarations.push("background-color: transparent".into()),
        [] => {}
    }
    if images.is_empty() {
        declarations.push("background-image: none".into());
    } else {
        declarations.push(format!("background-image: {}", images.join(", ")));
        declarations.push(format!("background-repeat: {}", repeats.join(", ")));
        declarations.push(format!("background-position: {}", positions.join(", ")));
        declarations.push(format!("background-size: {}", sizes.join(", ")));
    }
    css.push_str(&format!("{selector} {{ {}; }}\n", declarations.join("; ")));
}

fn background_svg_css(config: &CardBackgroundSvgConfig) -> Result<BackgroundSvgCss, String> {
    let path = config.resolve_path(&crate::core::config::config_dir())?;
    let uri = gio::File::for_path(path).uri().to_string();
    if uri
        .chars()
        .any(|character| character.is_control() || matches!(character, '"' | '\\'))
    {
        return Err("resolved file URI contains unsafe CSS characters".into());
    }
    Ok(BackgroundSvgCss {
        image: format!(
            "cross-fade({:.3}% url(\"{uri}\"), image(transparent))",
            config.opacity_percent()
        ),
        position: config.position.as_css(),
        size: config.fit.as_css(),
    })
}

fn append_foreground(css: &mut String, selector: &str, color: &Option<String>) {
    if let Some(color) = color.as_deref().and_then(css_color) {
        css.push_str(&format!("{selector} {{ color: {color}; }}\n"));
    }
}

/// Keep generated CSS data-only. Supporting the common CSS hex forms avoids
/// letting a configuration value terminate a declaration and inject rules.
fn css_color(value: &str) -> Option<&str> {
    let digits = value.strip_prefix('#')?;
    matches!(digits.len(), 3 | 4 | 6 | 8)
        .then_some(())
        .filter(|_| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(|_| value)
}

fn css_fragment(value: &str) -> String {
    let fragment: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    if fragment.is_empty() {
        "unnamed".into()
    } else {
        fragment
    }
}

fn stable_hash(value: &str) -> u32 {
    value.bytes().fold(2_166_136_261, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    })
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

fn refresh_and_logo_visibility(compact: bool, logo_active: bool) -> (bool, bool) {
    (!compact, compact && logo_active)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svg_css() -> BackgroundSvgCss {
        BackgroundSvgCss {
            image: "cross-fade(10.000% url(\"file:///tmp/card.svg\"), image(transparent))".into(),
            position: "right center",
            size: "contain",
        }
    }

    #[test]
    fn svg_background_is_emitted_without_replacing_the_theme_tint() {
        let mut css = String::new();
        append_background_css(
            &mut css,
            ".card-theme-test",
            &CardColorsConfig::default(),
            Some(&svg_css()),
            true,
        );
        assert!(css.contains("background-image: cross-fade(10.000%"));
        assert!(css.contains("background-position: right center"));
        assert!(css.contains("background-size: contain"));
        assert!(!css.contains("background-color:"));
    }

    #[test]
    fn state_gradient_and_svg_are_kept_as_separate_background_layers() {
        let mut css = String::new();
        let colors = CardColorsConfig {
            background: vec!["#e01b24".into(), "#9141ac".into()],
            background_opacity: Some(0.16),
            ..CardColorsConfig::default()
        };
        append_background_css(
            &mut css,
            ".card-theme-test.card-state-critical",
            &colors,
            Some(&svg_css()),
            false,
        );
        assert!(css.contains("background-color: transparent"));
        assert!(css.contains(
            "background-image: cross-fade(10.000% url(\"file:///tmp/card.svg\"), image(transparent)), linear-gradient"
        ));
        assert!(css.contains("alpha(#e01b24, 0.160), alpha(#9141ac, 0.160)"));
        assert!(css.contains("background-repeat: no-repeat, no-repeat"));
        assert!(css.contains("background-size: contain, cover"));
    }

    #[test]
    fn empty_state_colors_leave_the_base_svg_and_tint_untouched() {
        let mut css = String::new();
        append_background_css(
            &mut css,
            ".card-theme-test.card-state-normal",
            &CardColorsConfig::default(),
            Some(&svg_css()),
            false,
        );
        assert!(css.is_empty());
    }

    #[test]
    fn compact_mode_keeps_the_logo_but_not_the_refresh_affordance() {
        assert_eq!(refresh_and_logo_visibility(false, false), (true, false));
        assert_eq!(refresh_and_logo_visibility(false, true), (true, false));
        assert_eq!(refresh_and_logo_visibility(true, false), (false, false));
        assert_eq!(refresh_and_logo_visibility(true, true), (false, true));
    }
}
