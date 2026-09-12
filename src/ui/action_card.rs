use gtk::prelude::*;
use gtk::{Align, Box as GtkBox, Button, Image, Label, Orientation, Spinner};
use std::rc::Rc;

type ActionResolver = dyn Fn(&str) -> Option<(bool, String, String)>;
type ActionClick = dyn Fn(&str) -> bool;
type DialogCallback = dyn Fn();

pub struct ActionCardCallbacks {
    resolve: Rc<ActionResolver>,
    on_click: Rc<ActionClick>,
    on_dialog_open: Rc<DialogCallback>,
    on_dialog_response: Rc<DialogCallback>,
}

impl ActionCardCallbacks {
    pub fn new(
        resolve: impl Fn(&str) -> Option<(bool, String, String)> + 'static,
        on_click: impl Fn(&str) -> bool + 'static,
        on_dialog_open: impl Fn() + 'static,
        on_dialog_response: impl Fn() + 'static,
    ) -> Self {
        Self {
            resolve: Rc::new(resolve),
            on_click: Rc::new(on_click),
            on_dialog_open: Rc::new(on_dialog_open),
            on_dialog_response: Rc::new(on_dialog_response),
        }
    }
}

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
        callbacks: ActionCardCallbacks,
    ) -> Self {
        let initial_confirm = (callbacks.resolve)(action_id).is_some_and(|(confirm, _, _)| confirm);
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

        if initial_confirm {
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
        let resolve = callbacks.resolve;
        let on_click = callbacks.on_click;
        let on_dialog_open = callbacks.on_dialog_open;
        let on_dialog_response = callbacks.on_dialog_response;
        let btn = Button::with_label("执行");
        btn.set_valign(Align::Center);
        btn.add_css_class("pill");
        btn.add_css_class("suggested-action");
        btn.add_css_class("action-run-btn");
        btn.update_property(&[gtk::accessible::Property::Label("执行操作")]);
        let running_spinner = spinner.clone();
        btn.connect_clicked(move |button| {
            let Some((confirm, confirm_title, confirm_detail)) = resolve(&aid) else {
                // A hot reload may remove an action while its control remains
                // visible. Never execute the old command silently.
                return;
            };
            if !confirm {
                if on_click(&aid) {
                    set_running(button, &running_spinner, true);
                }
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
            on_dialog_open();
            let aid = aid.clone();
            let resolve = resolve.clone();
            let on_click = on_click.clone();
            let on_dialog_response = on_dialog_response.clone();
            let button = button.clone();
            let running_spinner = running_spinner.clone();
            glib::MainContext::default().spawn_local(async move {
                let response = dialog.choose_future(Some(&window)).await;
                on_dialog_response();
                if response == Ok(1) && resolve(&aid).is_some() && on_click(&aid) {
                    set_running(&button, &running_spinner, true);
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
        set_running(&self.button, &self.spinner, running);
    }
}

fn set_running(button: &Button, spinner: &Spinner, running: bool) {
    spinner.set_visible(running);
    if running {
        spinner.start();
    } else {
        spinner.stop();
    }
    button.set_sensitive(!running);
}
