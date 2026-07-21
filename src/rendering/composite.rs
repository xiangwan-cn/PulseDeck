use gtk::prelude::*;
use gtk::{Align, Box as GtkBox, Label, Orientation};

use crate::model::card_model::{CardModel, CardValue};

pub struct CompositeRow {
    pub container: GtkBox,
    label: Label,
    value: Label,
}

impl CompositeRow {
    fn new() -> Self {
        let container = GtkBox::new(Orientation::Horizontal, 8);
        let label = Label::new(None);
        label.set_halign(Align::Start);
        label.add_css_class("metric-header-sub");
        container.append(&label);

        let value = Label::new(None);
        value.set_halign(Align::Start);
        value.set_hexpand(true);
        value.set_ellipsize(gtk::pango::EllipsizeMode::End);
        container.append(&value);

        container.set_visible(false);
        Self {
            container,
            label,
            value,
        }
    }

    fn set_visible(&self, visible: bool) {
        self.container.set_visible(visible);
    }

    fn set_labels(&self, label: &str, value: &str) {
        self.label.set_label(label);
        self.value.set_label(value);
    }
}

pub fn apply_composite(widgets: &CompositeWidgets, model: &CardModel) {
    let fields = match &model.value {
        CardValue::Composite(fields) => fields.clone(),
        _ => return,
    };

    for (i, row) in widgets.rows.iter().enumerate() {
        if let Some(field) = fields.get(i) {
            row.set_visible(true);
            row.set_labels(&field.label, &field.value);
        } else {
            row.set_visible(false);
        }
    }
}

pub struct CompositeWidgets {
    pub rows: Vec<CompositeRow>,
}

impl CompositeWidgets {
    pub fn new(max_fields: usize) -> Self {
        let rows: Vec<_> = (0..max_fields).map(|_| CompositeRow::new()).collect();

        Self { rows }
    }
}
