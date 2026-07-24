use gtk::prelude::*;
use gtk::{Align, Button, Label};

use crate::model::card_model::{CardModel, CardValue};

pub fn apply_action(widgets: &ActionWidgets, model: &CardModel) {
    let status = match &model.value {
        CardValue::Text(value) if !value.trim().is_empty() => Some(value.as_str()),
        _ => None,
    };
    widgets.status.set_label(status.unwrap_or_default());
    widgets.status.set_visible(status.is_some());
}

#[derive(Clone)]
pub struct ActionWidgets {
    pub button: Button,
    pub spinner: gtk::Spinner,
    pub status: Label,
}

impl ActionWidgets {
    pub fn new(label: &str) -> Self {
        let spinner = gtk::Spinner::new();
        spinner.set_visible(false);
        spinner.set_size_request(20, 20);
        spinner.set_valign(Align::Center);

        let button = Button::with_label(label);
        button.set_valign(Align::Center);
        button.add_css_class("pill");
        button.add_css_class("suggested-action");
        button.add_css_class("action-run-btn");
        button.set_sensitive(false);

        let status = Label::new(None);
        status.set_halign(Align::Center);
        status.add_css_class("metric-footer");
        status.set_visible(false);

        Self {
            button,
            spinner,
            status,
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.button.set_sensitive(enabled);
        self.button.set_tooltip_text(Some(if enabled {
            "执行操作"
        } else {
            "未绑定可用操作"
        }));
    }

    pub fn set_running(&self, running: bool) {
        self.spinner.set_visible(running);
        if running {
            self.spinner.start();
            self.status.set_label("执行中...");
            self.status.set_visible(true);
        } else {
            self.spinner.stop();
            self.status.set_visible(false);
        }
        self.button.set_sensitive(!running);
    }
}
