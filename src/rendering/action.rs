use gtk::prelude::*;
use gtk::{Align, Button, Label};

use crate::model::card_model::CardModel;

pub fn apply_action(_widgets: &ActionWidgets, _model: &CardModel) {}

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
}
