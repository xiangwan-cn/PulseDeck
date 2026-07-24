use gtk::prelude::*;
use gtk::{Align, Box as GtkBox, Button, Image, Label, Orientation, Spinner};
use std::rc::Rc;

pub struct ActionCard {
    pub card: GtkBox,
    pub button: Button,
    pub spinner: Spinner,
}

impl ActionCard {
    pub fn new(
        action_id: &str,
        name: &str,
        description: &str,
        icon_name: &str,
        confirm: bool,
        confirm_title: &str,
        confirm_detail: &str,
        on_click: impl Fn(&str) + 'static,
    ) -> Self {
        let card = GtkBox::new(Orientation::Vertical, 0);
        card.add_css_class("card");
        card.add_css_class("pulsedeck-card");
        card.add_css_class("action-card");
        card.set_valign(Align::Fill);
        card.set_hexpand(true);
        card.set_size_request(-1, 133);
        card.set_overflow(gtk::Overflow::Hidden);

        let hdr = GtkBox::new(Orientation::Horizontal, 10);
        let img = Image::from_icon_name(icon_name);
        img.set_pixel_size(28);
        img.set_valign(Align::Center);
        img.add_css_class("action-icon");
        hdr.append(&img);

        let tb = GtkBox::new(Orientation::Vertical, 1);
        tb.set_hexpand(true);
        tb.set_valign(Align::Center);

        let nl = Label::new(Some(name));
        nl.set_halign(Align::Start);
        nl.add_css_class("action-name");
        tb.append(&nl);

        let sl = Label::new(Some(description));
        sl.set_halign(Align::Start);
        sl.add_css_class("action-desc");
        tb.append(&sl);

        if confirm {
            let badge = Label::new(Some("需确认"));
            badge.set_halign(Align::Start);
            badge.add_css_class("action-confirm-badge");
            tb.append(&badge);
        }

        hdr.append(&tb);
        card.append(&hdr);

        let btn_row = GtkBox::new(Orientation::Horizontal, 8);
        btn_row.set_halign(Align::Center);
        btn_row.set_margin_top(12);

        let spinner = Spinner::new();
        spinner.set_visible(false);
        spinner.set_size_request(20, 20);
        spinner.set_valign(Align::Center);
        btn_row.append(&spinner);

        let aid = action_id.to_string();
        let on_click: Rc<dyn Fn(&str)> = Rc::new(on_click);
        let confirm_title = confirm_title.to_string();
        let confirm_detail = confirm_detail.to_string();
        let btn = Button::with_label("执行");
        btn.set_valign(Align::Center);
        btn.add_css_class("pill");
        btn.add_css_class("suggested-action");
        btn.add_css_class("action-run-btn");
        btn.connect_clicked(move |button| {
            if !confirm {
                on_click(&aid);
                return;
            }
            let Some(window) = button
                .root()
                .and_then(|root| root.downcast::<gtk::Window>().ok())
            else {
                return;
            };
            let dialog = gtk::AlertDialog::builder()
                .message(&confirm_title)
                .detail(&confirm_detail)
                .buttons(["取消", "执行"])
                .cancel_button(0)
                .default_button(1)
                .build();
            let aid = aid.clone();
            let on_click = on_click.clone();
            glib::MainContext::default().spawn_local(async move {
                if dialog.choose_future(Some(&window)).await == Ok(1) {
                    on_click(&aid);
                }
            });
        });
        btn_row.append(&btn);
        card.append(&btn_row);

        Self {
            card,
            button: btn,
            spinner,
        }
    }

    pub fn set_running(&self, running: bool) {
        self.spinner.set_visible(running);
        if running {
            self.spinner.start();
            self.button.set_sensitive(false);
        } else {
            self.spinner.stop();
            self.button.set_sensitive(true);
        }
    }
}
