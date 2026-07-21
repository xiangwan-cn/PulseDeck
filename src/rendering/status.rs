use gtk::prelude::*;
use gtk::{Align, Label};

use crate::model::card_model::{CardModel, CardValue, StatusLevel};

pub fn apply_status(widgets: &StatusWidgets, model: &CardModel) {
    let (label, level) = match &model.value {
        CardValue::Status { label, level } => (label.clone(), level.clone()),
        CardValue::Text(t) => (t.clone(), StatusLevel::Normal),
        _ => (String::new(), StatusLevel::Unknown),
    };

    widgets.value.set_label(&label);

    widgets.value.remove_css_class("metric-value-good");
    widgets.value.remove_css_class("metric-value-warning");
    widgets.value.remove_css_class("metric-value-critical");

    match level {
        StatusLevel::Good => widgets.value.add_css_class("metric-value-good"),
        StatusLevel::Warning => widgets.value.add_css_class("metric-value-warning"),
        StatusLevel::Critical | StatusLevel::Error => {
            widgets.value.add_css_class("metric-value-critical")
        }
        _ => {}
    }
}

pub struct StatusWidgets {
    pub value: Label,
}

impl StatusWidgets {
    pub fn new() -> Self {
        let value = Label::new(None);
        value.set_halign(Align::Center);
        value.set_hexpand(true);
        value.add_css_class("metric-value");
        value.set_ellipsize(gtk::pango::EllipsizeMode::End);

        Self { value }
    }
}
