use gtk::prelude::*;
use gtk::{Align, Label, LevelBar, Orientation};

use crate::model::card_model::{CardModel, CardValue};

pub fn apply_progress(widgets: &ProgressWidgets, model: &CardModel) {
    let (frac, label) = match &model.value {
        CardValue::Percentage(p) => (*p / 100.0, crate::rendering::format::percentage(*p)),
        CardValue::Number {
            value,
            unit,
            decimals,
        } => (
            value.clamp(0.0, 100.0) / 100.0,
            crate::rendering::format::number(*value, unit.as_deref(), *decimals),
        ),
        CardValue::Text(t) => {
            if let Ok(p) = t.trim_end_matches('%').parse::<f64>() {
                (
                    p.clamp(0.0, 100.0) / 100.0,
                    crate::rendering::format::percentage(p),
                )
            } else {
                (0.0, t.clone())
            }
        }
        _ => (0.0, String::new()),
    };

    widgets.value.set_label(&label);
    widgets.bar.set_value(frac);

    widgets.value.remove_css_class("metric-value-critical");
    widgets.value.remove_css_class("metric-value-warning");
    if frac >= 0.9 {
        widgets.value.add_css_class("metric-value-critical");
    } else if frac >= 0.7 {
        widgets.value.add_css_class("metric-value-warning");
    }
}

pub struct ProgressWidgets {
    pub value: Label,
    pub bar: LevelBar,
}

impl ProgressWidgets {
    pub fn new() -> Self {
        let value = Label::new(None);
        value.set_halign(Align::Center);
        value.set_hexpand(true);
        value.add_css_class("metric-value");
        value.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let bar = LevelBar::new();
        bar.set_min_value(0.0);
        bar.set_max_value(1.0);
        bar.set_orientation(Orientation::Horizontal);
        bar.set_halign(Align::Fill);
        bar.set_valign(Align::Center);

        Self { value, bar }
    }
}
