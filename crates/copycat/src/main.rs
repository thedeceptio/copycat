//! copycat — the GTK4/libadwaita clipboard manager app.
//!
//! M0 scaffold: opens a minimal libadwaita window so the app builds and runs.
//! The fuzzy-search history popup (M2) replaces this placeholder window.

use adw::prelude::*;
use adw::{Application, ApplicationWindow};
use gtk::glib;
use gtk::{Align, Label};

const APP_ID: &str = "dev.thedeceptio.Copycat";

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &Application) {
    let label = Label::builder()
        .label("copycat — M0 scaffold\nHistory popup arrives in M2.")
        .justify(gtk::Justification::Center)
        .halign(Align::Center)
        .valign(Align::Center)
        .build();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("copycat")
        .default_width(480)
        .default_height(320)
        .content(&label)
        .build();

    window.present();
}
