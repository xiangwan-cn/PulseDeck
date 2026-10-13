use gtk::prelude::*;
use gtk::{Align, FlowBox, ScrolledWindow};

use crate::core::config::{DisplayConfig, UiSection};
use crate::model::card_model::CardModel;
use crate::plugins::{CardPresentation, CardPresentationHandle};
use crate::ui::action_card::{ActionCard, ActionCardCallbacks};
use crate::ui::metric_card::{CardLayout, CardStyleRegistry, MetricCard};

const GRID_ROWS: i32 = 3;
const FLOW_VERTICAL_MARGIN: i32 = 8;
const FLOW_ROW_SPACING: i32 = 6;

pub struct Page {
    pub container: gtk::Overlay,
    pub metric_flow: FlowBox,
    pub action_flow: FlowBox,
    pub metric_cards: std::collections::HashMap<String, MetricCard>,
    plugin_cards: std::collections::HashMap<String, PluginCardEntry>,
    pub action_cards: std::collections::HashMap<String, ActionCard>,
    pub has_metrics: bool,
    pub has_actions: bool,
    card_layout: CardLayout,
    available_height: i32,
    metric_min_heights: std::collections::HashMap<String, i32>,
    normal_columns: u32,
    compact_grid: bool,
    settings_page: bool,
    featured_area: gtk::Grid,
    featured_side: FlowBox,
    featured_companions: Vec<(gtk::Widget, i32)>,
    fullscreen_area: gtk::Box,
    fullscreen_content: gtk::Box,
    fullscreen_request: std::rc::Rc<std::cell::RefCell<Option<CardPresentationHandle>>>,
    card_style_registry: std::rc::Rc<std::cell::RefCell<CardStyleRegistry>>,
}

struct PluginCardEntry {
    widget: gtk::Widget,
    normal_width: Option<i32>,
    minimum_height: i32,
    normal_height: i32,
    fixed: bool,
    flow_position: i32,
    presentation: CardPresentation,
    request: CardPresentationHandle,
}

impl Page {
    pub fn new(
        page_id: &str,
        ui: &UiSection,
        card_style_registry: std::rc::Rc<std::cell::RefCell<CardStyleRegistry>>,
    ) -> Self {
        let settings_page = page_id == "settings";
        let flow_box = gtk::Box::new(gtk::Orientation::Vertical, FLOW_ROW_SPACING);
        flow_box.set_margin_top(4);
        flow_box.set_margin_bottom(4);
        flow_box.set_valign(Align::Fill);
        flow_box.set_vexpand(true);

        let columns = if settings_page {
            ui.card_columns.clamp(1, 3)
        } else {
            ui.card_columns.max(1)
        };
        let featured_area = gtk::Grid::new();
        featured_area.set_column_homogeneous(true);
        featured_area.set_column_spacing(6);
        featured_area.set_margin_start(4);
        featured_area.set_margin_end(4);
        featured_area.set_visible(false);
        let featured_side = Self::create_flow(1);
        featured_side.set_margin_start(0);
        featured_side.set_margin_end(0);
        featured_area.attach(&featured_side, 2, 0, 1, 3);
        flow_box.append(&featured_area);

        let metric_flow = Self::create_flow(columns);
        metric_flow.set_visible(false);
        if settings_page {
            metric_flow.set_min_children_per_line(1);
        }
        flow_box.append(&metric_flow);

        let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
        sep.set_visible(false);
        flow_box.append(&sep);

        let action_flow = Self::create_flow(columns);
        action_flow.set_visible(false);
        if settings_page {
            action_flow.set_min_children_per_line(1);
        }
        // An empty action section must not consume half of the vertical space on
        // metric-only pages; otherwise the third metric row becomes scrollable.
        action_flow.set_vexpand(false);
        flow_box.append(&action_flow);

        let scroll = ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_kinetic_scrolling(true);
        if settings_page {
            scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            scroll.set_propagate_natural_width(false);
            flow_box.set_hexpand(true);
            metric_flow.set_hexpand(true);
        }
        scroll.set_child(Some(&flow_box));

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&scroll));

        let fullscreen_area = gtk::Box::new(gtk::Orientation::Vertical, 6);
        fullscreen_area.set_hexpand(true);
        fullscreen_area.set_vexpand(true);
        fullscreen_area.set_halign(Align::Fill);
        fullscreen_area.set_valign(Align::Fill);
        fullscreen_area.set_margin_top(4);
        fullscreen_area.set_margin_bottom(4);
        fullscreen_area.set_margin_start(4);
        fullscreen_area.set_margin_end(4);
        fullscreen_area.add_css_class("card-fullscreen-layer");
        fullscreen_area.set_visible(false);

        let close = gtk::Button::from_icon_name("view-restore-symbolic");
        close.set_tooltip_text(Some("退出全屏"));
        close.update_property(&[gtk::accessible::Property::Label("退出全屏")]);
        close.set_halign(Align::End);
        close.add_css_class("circular");
        close.add_css_class("card-fullscreen-close");
        fullscreen_area.append(&close);

        let fullscreen_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        fullscreen_content.set_hexpand(true);
        fullscreen_content.set_vexpand(true);
        fullscreen_area.append(&fullscreen_content);
        overlay.add_overlay(&fullscreen_area);

        let fullscreen_request =
            std::rc::Rc::new(std::cell::RefCell::new(None::<CardPresentationHandle>));
        let close_request = fullscreen_request.clone();
        close.connect_clicked(move |_| {
            if let Some(request) = close_request.borrow().as_ref() {
                request.request(CardPresentation::Normal);
            }
        });
        let escape = gtk::EventControllerKey::new();
        let escape_request = fullscreen_request.clone();
        escape.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                if let Some(request) = escape_request.borrow().as_ref() {
                    request.request(CardPresentation::Normal);
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        overlay.add_controller(escape);

        Self {
            container: overlay,
            metric_flow,
            action_flow,
            metric_cards: std::collections::HashMap::new(),
            plugin_cards: std::collections::HashMap::new(),
            action_cards: std::collections::HashMap::new(),
            has_metrics: false,
            has_actions: false,
            card_layout: CardLayout {
                width: ui.card_width,
                height: ui.card_height.max(1),
                fixed: ui.fixed_card_size,
            },
            available_height: 0,
            metric_min_heights: std::collections::HashMap::new(),
            normal_columns: columns,
            compact_grid: false,
            settings_page,
            featured_area,
            featured_side,
            featured_companions: Vec::new(),
            fullscreen_area,
            fullscreen_content,
            fullscreen_request,
            card_style_registry,
        }
    }

    fn create_flow(columns: u32) -> FlowBox {
        let flow = FlowBox::new();
        flow.set_homogeneous(true);
        flow.set_min_children_per_line(columns);
        flow.set_max_children_per_line(columns);
        flow.set_row_spacing(FLOW_ROW_SPACING as u32);
        flow.set_column_spacing(6);
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_margin_start(4);
        flow.set_margin_end(4);
        flow.add_css_class("pulsedeck-flow");
        flow.set_valign(Align::Start);
        flow.set_vexpand(false);
        flow
    }

    pub fn add_metric_card(&mut self, model: &CardModel, display: Option<&DisplayConfig>) {
        let minimum_height = display
            .and_then(|d| d.card_height)
            .unwrap_or(self.card_layout.height)
            .max(1);
        let layout = CardLayout {
            width: display
                .and_then(|d| d.card_width)
                .or(self.card_layout.width),
            height: self.fitted_card_height().max(minimum_height),
            fixed: display
                .and_then(|d| d.fixed_size)
                .unwrap_or(self.card_layout.fixed),
        };
        let mut card = MetricCard::new(model, layout, display, self.card_style_registry.clone());
        card.set_compact(self.compact_grid);
        if !self.settings_page {
            card.card.set_size_request(
                card.card.width_request(),
                card_content_height(&card.card, self.fitted_card_height()).max(minimum_height),
            );
        }
        self.metric_flow.append(&card.card);
        clear_flow_child_margins(&card.card);
        self.metric_flow.set_visible(true);
        self.metric_min_heights
            .insert(model.id.clone(), minimum_height);
        self.metric_cards.insert(model.id.clone(), card);
        self.has_metrics = true;
    }

    pub fn add_plugin_card(
        &mut self,
        card_id: &str,
        widget: &gtk::Widget,
        display: Option<&DisplayConfig>,
        request: CardPresentationHandle,
    ) {
        let width = display
            .and_then(|value| value.card_width)
            .or(self.card_layout.width);
        let minimum_height = display
            .and_then(|value| value.card_height)
            .unwrap_or(self.card_layout.height)
            .max(1);
        let height = self.fitted_card_height().max(minimum_height);
        let fixed = display
            .and_then(|value| value.fixed_size)
            .unwrap_or(self.card_layout.fixed);
        if self.compact_grid {
            widget.add_css_class("compact-card");
        }
        let requested_height =
            card_request_height(widget, height, self.settings_page).max(minimum_height);
        widget.set_size_request(width.unwrap_or(-1), requested_height);
        self.metric_flow.append(widget);
        clear_flow_child_margins(widget);
        self.metric_flow.set_visible(true);
        let flow_position = self.metric_flow.child_at_index(0).map_or(0, |_| {
            self.metric_flow
                .observe_children()
                .n_items()
                .saturating_sub(1) as i32
        });
        self.plugin_cards.insert(
            card_id.to_string(),
            PluginCardEntry {
                widget: widget.clone(),
                normal_width: width,
                minimum_height,
                normal_height: height,
                fixed,
                flow_position,
                presentation: CardPresentation::Normal,
                request,
            },
        );
        self.has_metrics = true;
    }

    pub fn set_plugin_card_presentation(&mut self, card_id: &str, presentation: CardPresentation) {
        let Some(mut entry) = self.plugin_cards.remove(card_id) else {
            return;
        };
        if entry.presentation == presentation {
            self.plugin_cards.insert(card_id.to_string(), entry);
            return;
        }

        if matches!(
            entry.presentation,
            CardPresentation::Quad | CardPresentation::Expanded
        ) {
            self.restore_featured_companions();
        }
        if let Some(position) = flow_position(&entry.widget) {
            entry.flow_position = position;
        }
        detach_widget(&entry.widget);

        match presentation {
            CardPresentation::Normal => {
                entry.widget.set_valign(Align::Fill);
                entry.widget.set_margin_top(0);
                entry.widget.set_margin_bottom(0);
                entry.widget.set_margin_start(0);
                entry.widget.set_margin_end(0);
                if entry.fixed {
                    let height =
                        card_request_height(&entry.widget, entry.normal_height, self.settings_page)
                            .max(entry.minimum_height);
                    entry
                        .widget
                        .set_size_request(entry.normal_width.unwrap_or(-1), height);
                } else {
                    entry
                        .widget
                        .set_size_request(entry.normal_width.unwrap_or(-1), -1);
                }
                entry.widget.set_hexpand(false);
                entry.widget.set_vexpand(false);
                self.metric_flow.insert(&entry.widget, entry.flow_position);
                clear_flow_child_margins(&entry.widget);
                self.featured_area.set_visible(false);
                self.fullscreen_area.set_visible(false);
                self.fullscreen_request.borrow_mut().take();
            }
            CardPresentation::Quad | CardPresentation::Expanded => {
                let rows = if presentation == CardPresentation::Quad {
                    2
                } else {
                    3
                };
                let span_columns = 2;
                let columns = if self.compact_grid {
                    6
                } else {
                    self.normal_columns
                }
                .max(span_columns);
                let side_columns = columns.saturating_sub(span_columns);
                if self.featured_side.parent().is_some() {
                    self.featured_area.remove(&self.featured_side);
                }
                let companions = if side_columns > 0 {
                    self.featured_side.set_min_children_per_line(side_columns);
                    self.featured_side.set_max_children_per_line(side_columns);
                    self.featured_side.set_visible(true);
                    self.featured_area.attach(
                        &self.featured_side,
                        span_columns as i32,
                        0,
                        side_columns as i32,
                        rows,
                    );
                    flow_widgets(&self.metric_flow)
                } else {
                    self.featured_side.set_visible(false);
                    Vec::new()
                }
                .into_iter()
                .take((rows as u32 * side_columns) as usize)
                .map(|(widget, position)| {
                    let original_position = if position >= entry.flow_position {
                        position + 1
                    } else {
                        position
                    };
                    (widget, original_position)
                })
                .collect::<Vec<_>>();
                for (widget, position) in companions {
                    detach_widget(&widget);
                    self.featured_side.append(&widget);
                    clear_flow_child_margins(&widget);
                    self.featured_companions.push((widget, position));
                }
                let height = featured_card_request_height(
                    &entry,
                    &self.featured_side,
                    rows,
                    self.settings_page,
                );
                entry.widget.set_size_request(-1, height);
                entry.widget.set_hexpand(true);
                entry.widget.set_vexpand(false);
                apply_featured_card_insets(&entry.widget, &self.featured_side);
                entry.widget.set_valign(Align::Fill);
                entry.widget.set_halign(Align::Fill);
                self.featured_area
                    .attach(&entry.widget, 0, 0, span_columns as i32, rows);
                self.featured_area.set_visible(true);
                self.fullscreen_area.set_visible(false);
                self.fullscreen_request.borrow_mut().take();
            }
            CardPresentation::Fullscreen => {
                entry.widget.set_margin_top(0);
                entry.widget.set_margin_bottom(0);
                entry.widget.set_margin_start(0);
                entry.widget.set_margin_end(0);
                entry.widget.set_size_request(-1, -1);
                entry.widget.set_hexpand(true);
                entry.widget.set_vexpand(true);
                entry.widget.set_valign(Align::Fill);
                self.fullscreen_content.append(&entry.widget);
                self.fullscreen_request.replace(Some(entry.request.clone()));
                self.fullscreen_area.set_visible(true);
            }
        }
        self.sync_flow_visibility();
        entry.presentation = presentation;
        self.plugin_cards.insert(card_id.to_string(), entry);
    }

    pub fn set_compact_grid(&mut self, compact: bool) {
        if self.compact_grid == compact {
            return;
        }
        self.compact_grid = compact;
        let columns = if self.settings_page {
            self.normal_columns
        } else if compact {
            6
        } else {
            self.normal_columns
        };
        self.metric_flow
            .set_min_children_per_line(if self.settings_page { 1 } else { columns });
        self.metric_flow.set_max_children_per_line(columns);
        self.action_flow
            .set_min_children_per_line(if self.settings_page { 1 } else { columns });
        self.action_flow.set_max_children_per_line(columns);
        for card in self.metric_cards.values_mut() {
            card.set_compact(compact);
        }
        for entry in self.plugin_cards.values() {
            if compact {
                entry.widget.add_css_class("compact-card");
            } else {
                entry.widget.remove_css_class("compact-card");
            }
        }
        for card in self.action_cards.values() {
            if compact {
                card.card.add_css_class("compact-card");
            } else {
                card.card.remove_css_class("compact-card");
            }
        }
        self.apply_card_heights();
        let featured = self
            .plugin_cards
            .iter()
            .find(|(_, entry)| {
                matches!(
                    entry.presentation,
                    CardPresentation::Quad | CardPresentation::Expanded
                )
            })
            .map(|(id, entry)| (id.clone(), entry.presentation));
        if let Some((id, presentation)) = featured {
            self.set_plugin_card_presentation(&id, CardPresentation::Normal);
            self.set_plugin_card_presentation(&id, presentation);
        }
    }

    fn restore_featured_companions(&mut self) {
        let mut companions = std::mem::take(&mut self.featured_companions);
        companions.sort_by_key(|(_, position)| *position);
        for (widget, position) in companions {
            detach_widget(&widget);
            self.metric_flow.insert(&widget, position);
            clear_flow_child_margins(&widget);
        }
        self.featured_area.set_visible(false);
        self.sync_flow_visibility();
    }

    fn sync_flow_visibility(&self) {
        self.metric_flow
            .set_visible(self.metric_flow.observe_children().n_items() > 0);
        self.action_flow
            .set_visible(self.action_flow.observe_children().n_items() > 0);
    }

    pub fn set_available_height(&mut self, height: i32) {
        let height = height.max(0);
        if self.available_height == height {
            return;
        }
        self.available_height = height;
        self.apply_card_heights();
    }

    fn fitted_card_height(&self) -> i32 {
        if self.settings_page {
            return self.card_layout.height;
        }
        adaptive_card_height(self.available_height, self.card_layout.height)
    }

    fn apply_card_heights(&mut self) {
        let fitted = self.fitted_card_height();
        for (id, card) in &self.metric_cards {
            let content_height = if self.settings_page {
                fitted
            } else {
                card_content_height(&card.card, fitted)
            };
            let height = content_height.max(
                self.metric_min_heights
                    .get(id)
                    .copied()
                    .unwrap_or(self.card_layout.height),
            );
            card.card
                .set_size_request(card.card.width_request(), height);
        }
        for card in self.action_cards.values() {
            let height = if self.settings_page {
                fitted
            } else {
                card_content_height(&card.card, fitted)
            };
            card.card.set_size_request(-1, height);
        }
        for entry in self.plugin_cards.values_mut() {
            entry.normal_height = fitted.max(entry.minimum_height);
            if matches!(
                entry.presentation,
                CardPresentation::Quad | CardPresentation::Expanded
            ) {
                apply_featured_card_insets(&entry.widget, &self.featured_side);
            }
            let normal_height =
                card_request_height(&entry.widget, entry.normal_height, self.settings_page)
                    .max(entry.minimum_height);
            match entry.presentation {
                CardPresentation::Normal => entry
                    .widget
                    .set_size_request(entry.normal_width.unwrap_or(-1), normal_height),
                CardPresentation::Quad => entry.widget.set_size_request(
                    -1,
                    featured_card_request_height(entry, &self.featured_side, 2, self.settings_page),
                ),
                CardPresentation::Expanded => entry.widget.set_size_request(
                    -1,
                    featured_card_request_height(entry, &self.featured_side, 3, self.settings_page),
                ),
                CardPresentation::Fullscreen => {}
            }
        }
    }

    pub fn add_action_card(
        &mut self,
        action_id: &str,
        name: &str,
        description: &str,
        icon_name: &str,
        callbacks: ActionCardCallbacks,
    ) {
        let card = ActionCard::new(action_id, name, description, icon_name, callbacks);
        card.card.set_size_request(-1, self.fitted_card_height());
        if self.compact_grid {
            card.card.add_css_class("compact-card");
        }
        self.action_flow.append(&card.card);
        clear_flow_child_margins(&card.card);
        self.action_flow.set_visible(true);
        self.action_cards.insert(action_id.to_string(), card);
        self.has_actions = true;
    }

    pub fn flow_insert(&self, widget: &impl IsA<gtk::Widget>) {
        self.metric_flow.append(widget);
        clear_flow_child_margins(widget);
        self.metric_flow.set_visible(true);
    }

    pub fn get_metric_card(&mut self, card_id: &str) -> Option<&mut MetricCard> {
        self.metric_cards.get_mut(card_id)
    }

    pub fn get_action_card(&mut self, card_id: &str) -> Option<&mut ActionCard> {
        self.action_cards.get_mut(card_id)
    }
}

fn adaptive_card_height(available_height: i32, minimum_height: i32) -> i32 {
    if available_height <= 0 {
        return minimum_height.max(1);
    }
    let spacing = FLOW_ROW_SPACING * (GRID_ROWS - 1);
    let usable = available_height
        .saturating_sub(FLOW_VERTICAL_MARGIN)
        .saturating_sub(spacing);
    (usable / GRID_ROWS).max(minimum_height.max(1))
}

// GTK adds CSS padding and borders to a widget's minimum content request.
// The grid's fitted height is an outer allocation budget, so requesting that
// whole height as content makes each of the three rows overflow its budget.
// Resolve the actual style instead of hardcoding normal/compact padding.
// GTK 4.10 deprecated style contexts without an equivalent box-inset getter.
// Keep the compatibility API scoped to these measurements.
#[allow(deprecated)]
fn card_content_height(widget: &impl IsA<gtk::Widget>, outer_height: i32) -> i32 {
    let context = widget.style_context();
    let padding = context.padding();
    let border = context.border();
    let inset = i32::from(padding.top())
        + i32::from(padding.bottom())
        + i32::from(border.top())
        + i32::from(border.bottom());
    outer_height.saturating_sub(inset).max(1)
}

fn card_request_height(
    widget: &impl IsA<gtk::Widget>,
    outer_height: i32,
    settings_page: bool,
) -> i32 {
    if settings_page {
        outer_height
    } else {
        card_content_height(widget, outer_height)
    }
}

fn featured_card_request_height(
    entry: &PluginCardEntry,
    flow: &FlowBox,
    rows: i32,
    settings_page: bool,
) -> i32 {
    let cell_height = card_request_height(&entry.widget, entry.normal_height, settings_page)
        .max(entry.minimum_height);
    let insets = featured_card_insets(flow);
    let row_spacing = flow.row_spacing() as i32 + insets[0] + insets[2];
    let outer_height = cell_height
        .saturating_mul(rows)
        .saturating_add(row_spacing.saturating_mul(rows.saturating_sub(1)));
    card_request_height(&entry.widget, outer_height, settings_page)
}

#[allow(deprecated)]
fn featured_card_insets(flow: &FlowBox) -> [i32; 4] {
    flow.child_at_index(0).map_or([0; 4], |child| {
        let context = child.style_context();
        let padding = context.padding();
        let border = context.border();
        [
            i32::from(padding.top()) + i32::from(border.top()),
            i32::from(padding.right()) + i32::from(border.right()),
            i32::from(padding.bottom()) + i32::from(border.bottom()),
            i32::from(padding.left()) + i32::from(border.left()),
        ]
    })
}

fn apply_featured_card_insets(widget: &gtk::Widget, flow: &FlowBox) {
    let [top, right, bottom, left] = featured_card_insets(flow);
    widget.set_margin_top(top);
    widget.set_margin_end(right);
    widget.set_margin_bottom(bottom);
    widget.set_margin_start(left);
}

fn flow_position(widget: &gtk::Widget) -> Option<i32> {
    widget
        .parent()
        .and_downcast::<gtk::FlowBoxChild>()
        .map(|child| child.index())
}

fn clear_flow_child_margins(widget: &impl IsA<gtk::Widget>) {
    if let Some(child) = widget.parent().and_downcast::<gtk::FlowBoxChild>() {
        child.set_margin_top(0);
        child.set_margin_bottom(0);
        child.set_margin_start(0);
        child.set_margin_end(0);
    }
}

fn flow_widgets(flow: &FlowBox) -> Vec<(gtk::Widget, i32)> {
    let mut widgets = Vec::new();
    let mut index = 0;
    while let Some(child) = flow.child_at_index(index) {
        if let Some(widget) = child.child() {
            widgets.push((widget, index));
        }
        index += 1;
    }
    widgets
}

fn detach_widget(widget: &gtk::Widget) {
    let Some(parent) = widget.parent() else {
        return;
    };
    if let Ok(child) = parent.clone().downcast::<gtk::FlowBoxChild>() {
        if let Some(flow) = child.parent().and_downcast::<FlowBox>() {
            // Removing the FlowBoxChild alone may leave `widget` parented to
            // the detached wrapper. Explicitly release it first so it can be
            // attached to the expanded grid or inserted back into the flow.
            child.set_child(gtk::Widget::NONE);
            flow.remove(&child);
            return;
        }
    }
    if let Ok(box_) = parent.clone().downcast::<gtk::Box>() {
        box_.remove(widget);
    } else if let Ok(grid) = parent.clone().downcast::<gtk::Grid>() {
        grid.remove(widget);
    } else {
        widget.unparent();
    }
}

#[cfg(test)]
mod tests {
    use super::{adaptive_card_height, FLOW_ROW_SPACING, FLOW_VERTICAL_MARGIN};

    #[test]
    fn adaptive_height_fills_three_visible_rows() {
        let height = adaptive_card_height(668, 133);
        assert_eq!(height, 216);
        assert_eq!(
            height * 3 + FLOW_VERTICAL_MARGIN + FLOW_ROW_SPACING * 2,
            668
        );
    }

    #[test]
    fn adaptive_height_preserves_configured_minimum() {
        assert_eq!(adaptive_card_height(360, 133), 133);
        assert_eq!(adaptive_card_height(0, 160), 160);
    }
}
