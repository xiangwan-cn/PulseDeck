use gtk::prelude::*;
use gtk::{Align, Label};

use crate::core::text::{
    bounded_joined_lines, bounded_text, MAX_UI_COLLECTION_ITEMS, MAX_UI_TEXT_BYTES,
};
use crate::model::card_model::{CardModel, CardValue};

pub fn apply_list(widgets: &ListWidgets, model: &CardModel) {
    let items = match &model.value {
        CardValue::List(items) => items,
        _ => return,
    };

    let text = bounded_joined_lines(
        items
            .iter()
            .take(MAX_UI_COLLECTION_ITEMS.saturating_add(1))
            .map(|item| {
                format!(
                    "{}: {}",
                    bounded_text(&item.label, 16 * 1024),
                    bounded_text(&item.value, 16 * 1024)
                )
            }),
        MAX_UI_COLLECTION_ITEMS,
        MAX_UI_TEXT_BYTES,
    );

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
