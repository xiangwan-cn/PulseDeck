use gtk::prelude::*;
use gtk::{Align, Label};

use crate::model::card_model::{CardModel, CardValue};

pub fn apply_list(widgets: &ListWidgets, model: &CardModel) {
    let items = match &model.value {
        CardValue::List(items) => items.clone(),
        _ => return,
    };

    let text: String = items
        .iter()
        .map(|i| format!("{}: {}", i.label, i.value))
        .collect::<Vec<_>>()
        .join("\n");

    widgets.value.set_label(&text);
}

pub struct ListWidgets {
    pub value: Label,
}

impl ListWidgets {
    pub fn new() -> Self {
        let value = Label::new(None);
        value.set_halign(Align::Start);
        value.add_css_class("metric-value");
        value.set_lines(6);
        value.set_ellipsize(gtk::pango::EllipsizeMode::End);

        Self { value }
    }
}
