use gtk::prelude::*;
use gtk::{Align, FlowBox, ScrolledWindow};

use crate::core::config::{DisplayConfig, UiSection};
use crate::model::card_model::CardModel;
use crate::plugins::{CardPresentation, CardPresentationHandle};
use crate::ui::action_card::ActionCard;
use crate::ui::metric_card::CardLayout;
use crate::ui::metric_card::MetricCard;

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
    normal_columns: u32,
    compact_grid: bool,
    featured_area: gtk::Grid,
    featured_side: FlowBox,
    featured_companions: Vec<(gtk::Widget, i32)>,
    fullscreen_area: gtk::Box,
    fullscreen_content: gtk::Box,
    fullscreen_request: std::rc::Rc<std::cell::RefCell<Option<CardPresentationHandle>>>,
}

struct PluginCardEntry {
    widget: gtk::Widget,
    normal_width: Option<i32>,
    normal_height: i32,
    fixed: bool,
    flow_position: i32,
    presentation: CardPresentation,
    request: CardPresentationHandle,
}

impl Page {
    pub fn new(page_id: &str, ui: &UiSection) -> Self {
        let flow_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        flow_box.set_margin_top(4);
        flow_box.set_margin_bottom(4);
        flow_box.set_valign(Align::Fill);
        flow_box.set_vexpand(true);

        let columns = if page_id == "settings" {
            1
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
        flow_box.append(&metric_flow);

        let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
        sep.set_visible(false);
        flow_box.append(&sep);

        let action_flow = Self::create_flow(3);
        // An empty action section must not consume half of the vertical space on
        // metric-only pages; otherwise the third metric row becomes scrollable.
        action_flow.set_vexpand(false);
        flow_box.append(&action_flow);

        let scroll = ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_kinetic_scrolling(true);
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
            normal_columns: columns,
            compact_grid: false,
            featured_area,
            featured_side,
            featured_companions: Vec::new(),
            fullscreen_area,
            fullscreen_content,
            fullscreen_request,
        }
    }

    fn create_flow(columns: u32) -> FlowBox {
        let flow = FlowBox::new();
        flow.set_homogeneous(true);
        flow.set_min_children_per_line(columns);
        flow.set_max_children_per_line(columns);
        flow.set_row_spacing(6);
        flow.set_column_spacing(6);
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_margin_start(4);
        flow.set_margin_end(4);
        flow.set_valign(Align::Start);
        flow.set_vexpand(false);
        flow
    }

    pub fn add_metric_card(&mut self, model: &CardModel, display: Option<&DisplayConfig>) {
        let layout = CardLayout {
            width: display
                .and_then(|d| d.card_width)
                .or(self.card_layout.width),
            height: display
                .and_then(|d| d.card_height)
                .unwrap_or(self.card_layout.height)
                .max(1),
            fixed: display
                .and_then(|d| d.fixed_size)
                .unwrap_or(self.card_layout.fixed),
        };
        let mut card = MetricCard::new(model, layout);
        card.set_compact(self.compact_grid);
        self.metric_flow.append(&card.card);
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
        let height = display
            .and_then(|value| value.card_height)
            .unwrap_or(self.card_layout.height)
            .max(1);
        let fixed = display
            .and_then(|value| value.fixed_size)
            .unwrap_or(self.card_layout.fixed);
        if fixed {
            widget.set_size_request(width.unwrap_or(-1), height);
        } else if let Some(width) = width {
            widget.set_size_request(width, -1);
        }
        if self.compact_grid {
            widget.add_css_class("compact-card");
        }
        self.metric_flow.append(widget);
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
                if entry.fixed {
                    entry
                        .widget
                        .set_size_request(entry.normal_width.unwrap_or(-1), entry.normal_height);
                } else {
                    entry
                        .widget
                        .set_size_request(entry.normal_width.unwrap_or(-1), -1);
                }
                entry.widget.set_hexpand(false);
                entry.widget.set_vexpand(false);
                self.metric_flow.insert(&entry.widget, entry.flow_position);
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
                let columns = if self.compact_grid {
                    6
                } else {
                    self.normal_columns
                }
                .max(3);
                let side_columns = columns - 2;
                self.featured_side.set_min_children_per_line(side_columns);
                self.featured_side.set_max_children_per_line(side_columns);
                self.featured_area.remove(&self.featured_side);
                self.featured_area
                    .attach(&self.featured_side, 2, 0, side_columns as i32, rows);
                let companions = flow_widgets(&self.metric_flow)
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
                    self.featured_companions.push((widget, position));
                }
                entry.widget.set_size_request(
                    -1,
                    entry.normal_height.saturating_mul(rows) + 6 * (rows - 1),
                );
                entry.widget.set_hexpand(true);
                entry.widget.set_vexpand(false);
                self.featured_area.attach(&entry.widget, 0, 0, 2, rows);
                self.featured_area.set_visible(true);
                self.fullscreen_area.set_visible(false);
                self.fullscreen_request.borrow_mut().take();
            }
            CardPresentation::Fullscreen => {
                entry.widget.set_size_request(-1, -1);
                entry.widget.set_hexpand(true);
                entry.widget.set_vexpand(true);
                self.fullscreen_content.append(&entry.widget);
                self.fullscreen_request.replace(Some(entry.request.clone()));
                self.fullscreen_area.set_visible(true);
            }
        }
        entry.presentation = presentation;
        self.plugin_cards.insert(card_id.to_string(), entry);
    }

    pub fn set_compact_grid(&mut self, compact: bool) {
        if self.compact_grid == compact {
            return;
        }
        self.compact_grid = compact;
        let columns = if compact { 6 } else { self.normal_columns };
        self.metric_flow.set_min_children_per_line(columns);
        self.metric_flow.set_max_children_per_line(columns);
        self.action_flow.set_min_children_per_line(columns);
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
        }
        self.featured_area.set_visible(false);
    }

    pub fn add_action_card(
        &mut self,
        action_id: &str,
        name: &str,
        description: &str,
        icon_name: &str,
        confirm: bool,
        confirm_title: &str,
        confirm_detail: &str,
        on_click: impl Fn(&str) + 'static,
    ) {
        let card = ActionCard::new(
            action_id,
            name,
            description,
            icon_name,
            confirm,
            confirm_title,
            confirm_detail,
            on_click,
        );
        if self.compact_grid {
            card.card.add_css_class("compact-card");
        }
        self.action_flow.append(&card.card);
        self.action_cards.insert(action_id.to_string(), card);
        self.has_actions = true;
    }

    pub fn flow_insert(&self, widget: &impl IsA<gtk::Widget>) {
        self.metric_flow.append(widget);
    }

    pub fn get_metric_card(&mut self, card_id: &str) -> Option<&mut MetricCard> {
        self.metric_cards.get_mut(card_id)
    }

    pub fn get_action_card(&mut self, card_id: &str) -> Option<&mut ActionCard> {
        self.action_cards.get_mut(card_id)
    }
}

fn flow_position(widget: &gtk::Widget) -> Option<i32> {
    widget
        .parent()
        .and_downcast::<gtk::FlowBoxChild>()
        .map(|child| child.index())
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
