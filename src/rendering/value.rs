use gtk::prelude::*;
use gtk::{Align, Box as GtkBox, Grid, Label, Orientation};

use crate::model::card_model::{CardModel, CardValue};

pub fn apply_value(widgets: &ValueWidgets, model: &CardModel) {
    while let Some(child) = widgets.grid.first_child() {
        widgets.grid.remove(&child);
    }
    widgets.grid.set_visible(false);
    widgets.value.set_visible(true);

    if let (CardValue::Text(text), Some(limit)) = (&model.value, model.columns_after) {
        if apply_text_grid(widgets, text, limit, model.columns.unwrap_or(2)) {
            return;
        }
    }

    let display = match &model.value {
        CardValue::Number {
            value,
            unit,
            decimals,
        } => {
            let u = unit.as_deref().unwrap_or("");
            format!("{:.*} {}", *decimals as usize, value, u)
        }
        CardValue::Percentage(p) => format!("{:.1}%", p),
        CardValue::Text(t) => t.clone(),
        _ => String::new(),
    };

    widgets.value.set_label(&display);
}

fn apply_text_grid(widgets: &ValueWidgets, text: &str, limit: usize, columns: usize) -> bool {
    let mut summary = Vec::new();
    let mut items = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        if line.contains('轮') && (line.contains('件') || line.contains("剩")) {
            summary.push(line);
        } else {
            items.push(line);
        }
    }
    if items.len() <= limit {
        return false;
    }

    widgets.value.set_visible(false);
    widgets.grid.set_visible(true);
    if !summary.is_empty() {
        let label = grid_label(&summary.join(" · "), true);
        label.set_halign(Align::Center);
        widgets.grid.attach(&label, 0, 0, columns.max(1) as i32, 1);
    }
    let columns = columns.max(1);
    // Compact fixed cards can hold eight short rows. Truncate only beyond the
    // configured grid's total capacity; the full value remains in the tooltip.
    items.truncate(columns.saturating_mul(8));
    let summary_offset = usize::from(!summary.is_empty());
    for (index, item) in items.into_iter().enumerate() {
        // Row-major order follows normal reading order. The previous
        // column-major placement made unrelated entries look like pairs.
        let column = index % columns;
        let row = index / columns + summary_offset;
        widgets.grid.attach(
            &grid_label(item, columns == 1),
            column as i32,
            row as i32,
            1,
            1,
        );
    }
    true
}

fn grid_label(text: &str, centered: bool) -> Label {
    let label = Label::new(Some(text));
    label.set_halign(if centered {
        Align::Center
    } else {
        Align::Start
    });
    label.set_hexpand(true);
    label.add_css_class("metric-value");
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(28);
    label
}

pub struct ValueWidgets {
    pub container: GtkBox,
    pub value: Label,
    pub grid: Grid,
}

impl ValueWidgets {
    pub fn new() -> Self {
        let value = Label::new(None);
        value.set_halign(Align::Center);
        value.set_hexpand(true);
        value.add_css_class("metric-value");
        value.set_ellipsize(gtk::pango::EllipsizeMode::End);
        value.set_max_width_chars(36);
        let grid = Grid::new();
        grid.set_column_spacing(8);
        grid.set_row_spacing(2);
        grid.set_column_homogeneous(true);
        grid.set_hexpand(true);
        grid.set_visible(false);
        let container = GtkBox::new(Orientation::Vertical, 0);
        container.append(&value);
        container.append(&grid);
        Self {
            container,
            value,
            grid,
        }
    }
}
