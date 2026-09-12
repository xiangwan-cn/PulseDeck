use gtk::prelude::*;
use gtk::{Align, Label};

use crate::core::text::{bounded_text, MAX_UI_TEXT_BYTES};
use crate::model::card_model::{CardModel, CardValue};

pub fn apply_text(widgets: &TextWidgets, model: &CardModel) {
    let val_str = match &model.value {
        CardValue::Text(t) => bounded_text(t, MAX_UI_TEXT_BYTES),
        CardValue::Number {
            value,
            unit,
            decimals,
        } => crate::rendering::format::number(*value, unit.as_deref(), *decimals),
        CardValue::Percentage(p) => crate::rendering::format::percentage(*p),
        CardValue::Status { label, .. } => bounded_text(label, MAX_UI_TEXT_BYTES),
        _ => String::new(),
    };

    widgets.value.set_label(&val_str);
    apply_cached(widgets, model.cached);
}

fn apply_cached(widgets: &TextWidgets, cached: bool) {
    if cached {
        widgets.value.add_css_class("metric-value-stale");
    } else {
        widgets.value.remove_css_class("metric-value-stale");
    }
}

pub struct TextWidgets {
    pub value: Label,
}

impl TextWidgets {
    pub fn new() -> Self {
        let value = Label::new(None);
        value.set_halign(Align::Start);
        value.add_css_class("metric-value");

        value.set_wrap(true);
        value.set_lines(4);
        value.set_ellipsize(gtk::pango::EllipsizeMode::End);
        Self { value }
    }
}
