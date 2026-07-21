use gtk::prelude::*;
use gtk::{Align, FlowBox, ScrolledWindow};

use crate::core::config::{DisplayConfig, UiSection};
use crate::model::card_model::CardModel;
use crate::ui::action_card::ActionCard;
use crate::ui::metric_card::CardLayout;
use crate::ui::metric_card::MetricCard;

pub struct Page {
    pub container: ScrolledWindow,
    pub metric_flow: FlowBox,
    pub action_flow: FlowBox,
    pub metric_cards: std::collections::HashMap<String, MetricCard>,
    pub action_cards: std::collections::HashMap<String, ActionCard>,
    pub has_metrics: bool,
    pub has_actions: bool,
    card_layout: CardLayout,
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

        Self {
            container: scroll,
            metric_flow,
            action_flow,
            metric_cards: std::collections::HashMap::new(),
            action_cards: std::collections::HashMap::new(),
            has_metrics: false,
            has_actions: false,
            card_layout: CardLayout {
                width: ui.card_width,
                height: ui.card_height.max(1),
                fixed: ui.fixed_card_size,
            },
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
        flow.set_valign(Align::Fill);
        flow.set_vexpand(true);
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
        let card = MetricCard::new(model, layout);
        self.metric_flow.append(&card.card);
        self.metric_cards.insert(model.id.clone(), card);
        self.has_metrics = true;
    }

    pub fn add_action_card(
        &mut self,
        action_id: &str,
        name: &str,
        description: &str,
        icon_name: &str,
        confirm: bool,
        on_click: impl Fn(&str) + 'static,
    ) {
        let card = ActionCard::new(action_id, name, description, icon_name, confirm, on_click);
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
