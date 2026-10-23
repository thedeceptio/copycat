//! copycat — the GTK4 / libadwaita clipboard-history popup.
//!
//! Launched on a hotkey (bind `Super+V` to `copycat` in GNOME Settings until the
//! portal hotkey lands). It loads the history written by `copycatd`, lets you
//! fuzzy-search it, and on Enter copies the chosen clip back to the clipboard.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use copycat_core::model::one_line;
use copycat_core::{default_db_path, rank, Clip, Store};
use gtk::{gdk, glib};
use gtk::{
    Box as GtkBox, EventControllerKey, ListBox, Orientation, PropagationPhase, ScrolledWindow,
    SearchEntry, Stack,
};

const APP_ID: &str = "dev.thedeceptio.Copycat";
/// How many recent clips to load into the popup.
const HISTORY_LIMIT: usize = 500;

/// Grace period between putting a clip on the clipboard and quitting.
///
/// Taking ownership of the X11 CLIPBOARD selection is an asynchronous request.
/// Quitting in the same breath can kill the process before that request is even
/// flushed to the display server, so the clip never lands. Waiting a beat lets
/// ownership settle and the desktop's clipboard manager mirror the content.
const CLIPBOARD_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);

/// How often the open popup checks for clips captured since it was opened.
///
/// The daemon writes history independently, so a popup left open would
/// otherwise show a frozen snapshot from whenever it launched.
const REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(700);

/// Names of the [`Stack`] pages, so we can swap list vs. empty state.
const PAGE_LIST: &str = "list";
const PAGE_EMPTY: &str = "empty";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        // GTK applications are single-instance: launching copycat again (e.g.
        // pressing the hotkey) activates the running process rather than
        // starting a new one. Raise the popup we already have instead of
        // stacking up a fresh window every time. The refresh timer keeps its
        // contents current, so the re-raised window is never stale.
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        build_ui(app);
    });
    app.run()
}

fn build_ui(app: &adw::Application) {
    let store: Option<Rc<Store>> = open_store().map(Rc::new);
    let all: Rc<RefCell<Vec<Clip>>> = Rc::new(RefCell::new(read_history(store.as_deref())));
    let ranked: Rc<RefCell<Vec<Clip>>> = Rc::new(RefCell::new(all.borrow().clone()));

    let entry = SearchEntry::new();
    entry.set_placeholder_text(Some("Search clipboard history…"));

    let list = ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("boxed-list");

    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();

    let empty = adw::StatusPage::builder()
        .icon_name("edit-paste-symbolic")
        .title("No clips")
        .description("Copy something and it will show up here.")
        .vexpand(true)
        .build();

    let stack = Stack::new();
    stack.add_named(&scroller, Some(PAGE_LIST));
    stack.add_named(&empty, Some(PAGE_EMPTY));

    let body = GtkBox::new(Orientation::Vertical, 12);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&entry);
    body.append(&stack);

    let toolbar = adw::ToolbarView::builder().content(&body).build();
    toolbar.add_top_bar(&adw::HeaderBar::new());

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("copycat")
        .default_width(620)
        .default_height(460)
        .build();
    window.set_content(Some(&toolbar));

    populate(&list, &stack, &ranked.borrow());

    // Re-rank as the user types.
    entry.connect_search_changed(
        glib::clone!(@strong list, @strong stack, @strong all, @strong ranked => move |e| {
            let results = rank(&e.text(), all.borrow().clone());
            populate(&list, &stack, &results);
            *ranked.borrow_mut() = results;
        }),
    );

    // Pick up clips the daemon captured while this popup is open, so the list
    // never shows a stale snapshot.
    if let Some(store) = store.clone() {
        let revision = Rc::new(RefCell::new(store.revision().unwrap_or_default()));
        glib::timeout_add_local(
            REFRESH_INTERVAL,
            glib::clone!(
                @strong list, @strong stack, @strong all, @strong ranked,
                @strong entry, @strong revision, @weak window
                => @default-return glib::ControlFlow::Break,
                move || {
                    if let Ok(current) = store.revision() {
                        if current != *revision.borrow() {
                            *revision.borrow_mut() = current;
                            if let Ok(fresh) = store.recent(HISTORY_LIMIT) {
                                *all.borrow_mut() = fresh;
                                let results = rank(&entry.text(), all.borrow().clone());
                                populate(&list, &stack, &results);
                                *ranked.borrow_mut() = results;
                            }
                        }
                    }
                    glib::ControlFlow::Continue
                }
            ),
        );
    }

    // Capture-phase key handling so Enter/arrows/Esc work while typing.
    let keys = EventControllerKey::new();
    keys.set_propagation_phase(PropagationPhase::Capture);
    keys.connect_key_pressed(glib::clone!(
        @strong list, @strong ranked, @weak window
        => @default-return glib::Propagation::Proceed,
        move |_, key, _, _| {
            match key {
                gdk::Key::Escape => {
                    window.close();
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    copy_selected(&list, &ranked.borrow());
                    dismiss_after_copy(&window);
                    glib::Propagation::Stop
                }
                gdk::Key::Up => {
                    move_selection(&list, -1, ranked.borrow().len());
                    glib::Propagation::Stop
                }
                gdk::Key::Down => {
                    move_selection(&list, 1, ranked.borrow().len());
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        }
    ));
    window.add_controller(keys);

    // Clicking a row copies it too.
    list.connect_row_activated(
        glib::clone!(@strong ranked, @weak window => move |list, _| {
            copy_selected(list, &ranked.borrow());
            dismiss_after_copy(&window);
        }),
    );

    window.present();
    entry.grab_focus();
}

/// Open the history database, or report why we could not.
///
/// A failure is not fatal: the popup still opens, just with an empty list.
fn open_store() -> Option<Store> {
    let path = match default_db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("copycat: cannot locate history database: {e}");
            return None;
        }
    };
    match Store::open(&path) {
        Ok(store) => Some(store),
        Err(e) => {
            eprintln!("copycat: failed to open history database: {e}");
            None
        }
    }
}

/// Read the most recent clips, falling back to an empty list on error.
fn read_history(store: Option<&Store>) -> Vec<Clip> {
    let Some(store) = store else {
        return Vec::new();
    };
    store.recent(HISTORY_LIMIT).unwrap_or_else(|e| {
        eprintln!("copycat: failed to read history: {e}");
        Vec::new()
    })
}

/// Rebuild the list rows from `clips`, select the first, and show either the
/// list or the empty state.
fn populate(list: &ListBox, stack: &Stack, clips: &[Clip]) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for clip in clips {
        let row = adw::ActionRow::new();
        // Clip text is arbitrary user data, never Pango markup.
        row.set_use_markup(false);
        row.set_title(&one_line(&clip.preview));
        list.append(&row);
    }
    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
    }
    stack.set_visible_child_name(if clips.is_empty() {
        PAGE_EMPTY
    } else {
        PAGE_LIST
    });
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

/// Hide the popup immediately, but let the process live just long enough for
/// clipboard ownership to settle before quitting. See [`CLIPBOARD_SETTLE`].
fn dismiss_after_copy(window: &adw::ApplicationWindow) {
    window.set_visible(false);
    let window = window.clone();
    glib::timeout_add_local_once(CLIPBOARD_SETTLE, move || window.close());
}

/// Copy the currently selected clip's full content back to the clipboard.
fn copy_selected(list: &ListBox, ranked: &[Clip]) {
    let Some(row) = list.selected_row() else {
        eprintln!("copycat: nothing selected, clipboard left alone");
        return;
    };
    let Some(clip) = ranked.get(row.index() as usize) else {
        eprintln!(
            "copycat: selected row {} has no matching clip, clipboard left alone",
            row.index()
        );
        return;
    };
    match gdk::Display::default() {
        Some(display) => {
            display.clipboard().set_text(clip.text());
            eprintln!(
                "copycat: copied clip {} ({} chars) to the clipboard",
                clip.id,
                clip.text().chars().count()
            );
        }
        None => eprintln!("copycat: no display, cannot set the clipboard"),
    }
}
