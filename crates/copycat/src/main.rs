//! copycat — the GTK4 clipboard manager app.
//!
//! M0 scaffold: opens a minimal window so the app builds and runs. libadwaita
//! styling and the fuzzy-search history popup (M2) replace this placeholder.

use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, Label};

const APP_ID: &str = "dev.thedeceptio.Copycat";

fn main() {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    let _ = app.run();
}

fn build_ui(app: &Application) {
    let label = Label::new(Some("copycat — M0 scaffold\nHistory popup arrives in M2."));

    let window = ApplicationWindow::builder()
        .application(app)
        .title("copycat")
        .default_width(480)
        .default_height(320)
        .build();
    window.set_child(Some(&label));
    window.present();
}
