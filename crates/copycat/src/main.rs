//! copycat — the GTK4 clipboard-history popup.
//!
//! Launched on a hotkey (bind `Super+V` to `copycat` in GNOME Settings until the
//! portal hotkey lands). It loads the history written by `copycatd`, lets you
//! fuzzy-search it, and on Enter copies the chosen clip back to the clipboard.
//! Plain GTK4 for now; libadwaita styling is deferred (see SPEC §5.1).

use std::cell::RefCell;
use std::rc::Rc;

use copycat_core::model::one_line;
use copycat_core::{default_db_path, rank, Clip, Store};
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::{
    Application, ApplicationWindow, Box as GtkBox, EventControllerKey, Inhibit, Label, ListBox,
    Orientation, PropagationPhase, ScrolledWindow, SearchEntry,
};

const APP_ID: &str = "dev.thedeceptio.Copycat";
/// How many recent clips to load into the popup.
const HISTORY_LIMIT: usize = 500;

fn main() {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    let _ = app.run();
}

fn build_ui(app: &Application) {
    let all: Rc<Vec<Clip>> = Rc::new(load_history());
    let ranked: Rc<RefCell<Vec<Clip>>> = Rc::new(RefCell::new((*all).clone()));

    let entry = SearchEntry::new();
    entry.set_placeholder_text(Some("Search clipboard history…"));

    let list = ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);

    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();

    let layout = GtkBox::new(Orientation::Vertical, 6);
    layout.set_margin_top(8);
    layout.set_margin_bottom(8);
    layout.set_margin_start(8);
    layout.set_margin_end(8);
    layout.append(&entry);
    layout.append(&scroller);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("copycat")
        .default_width(600)
        .default_height(420)
        .build();
    window.set_child(Some(&layout));

    populate(&list, &ranked.borrow());

    // Re-rank as the user types.
    entry.connect_search_changed(
        glib::clone!(@strong list, @strong all, @strong ranked => move |e| {
            let query = e.text().to_string();
            let results = rank(&query, (*all).clone());
            populate(&list, &results);
            *ranked.borrow_mut() = results;
        }),
    );

    // Capture-phase key handling so Enter/arrows/Esc work while typing.
    let keys = EventControllerKey::new();
    keys.set_propagation_phase(PropagationPhase::Capture);
    keys.connect_key_pressed(glib::clone!(@strong list, @strong ranked, @weak window => @default-return Inhibit(false), move |_, key, _, _| {
        match key {
            gdk::Key::Escape => {
                window.close();
                Inhibit(true)
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                copy_selected(&list, &ranked.borrow());
                window.close();
                Inhibit(true)
            }
            gdk::Key::Up => {
                move_selection(&list, -1, ranked.borrow().len());
                Inhibit(true)
            }
            gdk::Key::Down => {
                move_selection(&list, 1, ranked.borrow().len());
                Inhibit(true)
            }
            _ => Inhibit(false),
        }
    }));
    window.add_controller(&keys);

    // Double-click / activate a row copies it too.
    list.connect_row_activated(
        glib::clone!(@strong ranked, @weak window => move |list, _| {
            copy_selected(list, &ranked.borrow());
            window.close();
        }),
    );

    window.present();
    entry.grab_focus();
}

/// Load recent history, returning an empty list on any error (so the popup still
/// opens rather than crashing).
fn load_history() -> Vec<Clip> {
    let path = match default_db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("copycat: cannot locate history database: {e}");
            return Vec::new();
        }
    };
    match Store::open(&path) {
        Ok(store) => store.recent(HISTORY_LIMIT).unwrap_or_else(|e| {
            eprintln!("copycat: failed to read history: {e}");
            Vec::new()
        }),
        Err(e) => {
            eprintln!("copycat: failed to open history database: {e}");
            Vec::new()
        }
    }
}

/// Rebuild the list rows from `clips` and select the first.
fn populate(list: &ListBox, clips: &[Clip]) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for clip in clips {
        let label = Label::new(Some(&one_line(&clip.preview)));
        label.set_xalign(0.0);
        label.set_margin_top(4);
        label.set_margin_bottom(4);
        label.set_margin_start(6);
        label.set_margin_end(6);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        list.append(&label);
    }
    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
    }
}

/// Move the selection by `delta`, clamped to `[0, count)`, keeping it in view.
fn move_selection(list: &ListBox, delta: i32, count: usize) {
    if count == 0 {
        return;
    }
    let current = list.selected_row().map(|r| r.index()).unwrap_or(0);
    let next = (current + delta).clamp(0, count as i32 - 1);
    if let Some(row) = list.row_at_index(next) {
        list.select_row(Some(&row));
        row.grab_focus();
    }
}

/// Copy the currently selected clip's full content back to the clipboard.
fn copy_selected(list: &ListBox, ranked: &[Clip]) {
    let Some(row) = list.selected_row() else {
        return;
    };
    let Some(clip) = ranked.get(row.index() as usize) else {
        return;
    };
    if let Some(display) = gdk::Display::default() {
        display.clipboard().set_text(&clip.content);
    }
}
