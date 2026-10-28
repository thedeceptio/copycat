//! copycat — the GTK4 / libadwaita clipboard-history popup.
//!
//! Launched on a hotkey (bind `Super+V` to `copycat` in GNOME Settings until the
//! portal hotkey lands). It loads the history written by `copycatd`, lets you
//! fuzzy-search it, and on Enter copies the chosen clip back to the clipboard.

mod trace;
mod x11util;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use copycat_core::model::one_line;
use copycat_core::{default_db_path, rank, Clip, Store};
use gtk::{gdk, glib, pango};
use gtk::{
    Box as GtkBox, EventControllerKey, Label, ListItem, ListScrollFlags, ListView, Orientation,
    PropagationPhase, ScrolledWindow, SearchEntry, SignalListItemFactory, SingleSelection, Stack,
    StringList, StringObject,
};

const APP_ID: &str = "dev.thedeceptio.Copycat";
/// How many recent clips to load into the popup.
const HISTORY_LIMIT: usize = 500;

/// Read the history limit, honouring a `COPYCAT_HISTORY_LIMIT` override.
///
/// The override exists because the popup builds one widget per clip up front,
/// so the row count is a prime suspect whenever startup feels slow. Being able
/// to vary it without a rebuild turns "is it the rows?" into one measurement.
fn history_limit() -> usize {
    parse_history_limit(std::env::var("COPYCAT_HISTORY_LIMIT").ok().as_deref())
}

/// Parse a history-limit override, falling back to [`HISTORY_LIMIT`].
///
/// Anything unparseable is ignored rather than fatal: a typo in an environment
/// variable should not stop the popup opening.
fn parse_history_limit(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(HISTORY_LIMIT)
}

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

/// Popup size. Deliberately small — this is a menu, not a window you work in.
const POPUP_WIDTH: i32 = 420;
const POPUP_HEIGHT: i32 = 340;

/// How often to check whether our clipboard claim has landed.
const OWNERSHIP_POLL: std::time::Duration = std::time::Duration::from_millis(20);
/// Give up waiting for the clipboard claim after this long and paste anyway.
const OWNERSHIP_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(800);
/// How often to check whether the window manager has re-activated the window
/// the user came from.
const FOCUS_POLL: std::time::Duration = std::time::Duration::from_millis(25);
/// Give up waiting for focus to leave the popup after this long.
const FOCUS_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(700);

/// Names of the [`Stack`] pages, so we can swap list vs. empty state.
const PAGE_LIST: &str = "list";
const PAGE_EMPTY: &str = "empty";

/// GTK's "nothing is selected" sentinel for list positions.
///
/// `GTK_INVALID_LIST_POSITION` is `0xffffffff`. Treating it as an ordinary
/// index would mean indexing at four billion, so every read of
/// [`SingleSelection::selected`] has to check for it.
const NO_SELECTION: u32 = u32::MAX;

/// Padding around each row's text, in pixels. Rows are drawn by a recycled
/// [`Label`] rather than a full `AdwActionRow`, so the spacing that widget
/// provided has to be set here instead.
const ROW_PADDING: i32 = 10;

thread_local! {
    /// The window that held focus when the popup was summoned — where an
    /// auto-paste should land. Captured before we steal focus ourselves.
    static PASTE_TARGET: Cell<Option<u32>> = const { Cell::new(None) };
}

fn main() -> glib::ExitCode {
    let trace = Rc::new(trace::Trace::new());
    let app = adw::Application::builder().application_id(APP_ID).build();
    trace.mark("built AdwApplication");

    app.connect_activate(move |app| {
        trace.mark("activated (GTK init + D-Bus name registration)");

        // Remember where the user was working before our window takes focus.
        PASTE_TARGET.with(|target| target.set(x11util::active_window()));
        trace.mark("found the window to paste into");

        // GTK applications are single-instance: launching copycat again (e.g.
        // pressing the hotkey) activates the running process rather than
        // starting a new one. Raise the popup we already have instead of
        // stacking up a fresh window every time. The refresh timer keeps its
        // contents current, so the re-raised window is never stale.
        if let Some(window) = app.active_window() {
            window.present();
            trace.mark("re-presented the existing window");
            return;
        }
        build_ui(app, trace.clone());
    });
    app.run()
}

fn build_ui(app: &adw::Application, trace: Rc<trace::Trace>) {
    let store: Option<Rc<Store>> = open_store().map(Rc::new);
    trace.mark("opened the history database");
    let all: Rc<RefCell<Vec<Clip>>> = Rc::new(RefCell::new(read_history(store.as_deref())));
    trace.mark("read history");
    let ranked: Rc<RefCell<Vec<Clip>>> = Rc::new(RefCell::new(all.borrow().clone()));

    let entry = SearchEntry::new();
    entry.set_placeholder_text(Some("Search clipboard history…"));

    let (model, selection, list) = build_list();

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

    let body = GtkBox::new(Orientation::Vertical, 8);
    body.set_margin_top(8);
    body.set_margin_bottom(8);
    body.set_margin_start(8);
    body.set_margin_end(8);
    body.append(&entry);
    body.append(&stack);

    // No header bar: this is a menu that appears, gets used, and disappears.
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("copycat")
        .default_width(POPUP_WIDTH)
        .default_height(POPUP_HEIGHT)
        .resizable(false)
        .build();
    window.set_content(Some(&body));

    // GTK4 has no window-positioning API, so place it ourselves once it has an
    // X11 surface. Off X11 this does nothing and the compositor decides.
    window.connect_map(glib::clone!(@strong trace => move |window| {
        trace.mark("GTK finished realize + layout (inside present)");
        if let Some(xid) = window_xid(window) {
            x11util::place_at_pointer(xid, POPUP_WIDTH, POPUP_HEIGHT);
        }
        trace.mark("positioned at pointer — this is when you see it");
    }));

    populate(&model, &selection, &stack, &ranked.borrow());
    trace.mark("filled the list model");

    // Re-rank as the user types.
    entry.connect_search_changed(
        glib::clone!(@strong model, @strong selection, @strong stack, @strong all, @strong ranked => move |e| {
            let results = rank(&e.text(), all.borrow().clone());
            populate(&model, &selection, &stack, &results);
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
                @strong model, @strong selection, @strong stack, @strong all, @strong ranked,
                @strong entry, @strong revision, @weak window
                => @default-return glib::ControlFlow::Break,
                move || {
                    if let Ok(current) = store.revision() {
                        if current != *revision.borrow() {
                            *revision.borrow_mut() = current;
                            if let Ok(fresh) = store.recent(history_limit()) {
                                *all.borrow_mut() = fresh;
                                let results = rank(&entry.text(), all.borrow().clone());
                                populate(&model, &selection, &stack, &results);
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
        @strong list, @strong selection, @strong ranked, @weak window
        => @default-return glib::Propagation::Proceed,
        move |_, key, _, _| {
            match key {
                gdk::Key::Escape => {
                    window.close();
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    if let Some(previous) = copy_selected(&selection, &ranked.borrow()) {
                        dismiss_after_copy(&window, previous);
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Up => {
                    move_selection(&list, &selection, -1);
                    glib::Propagation::Stop
                }
                gdk::Key::Down => {
                    move_selection(&list, &selection, 1);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        }
    ));
    window.add_controller(keys);

    // Clicking a row copies it too.
    list.connect_activate(
        glib::clone!(@strong selection, @strong ranked, @weak window => move |_, position| {
            selection.set_selected(position);
            if let Some(previous) = copy_selected(&selection, &ranked.borrow()) {
                dismiss_after_copy(&window, previous);
            }
        }),
    );

    trace.mark("wired up signals");
    window.present();
    entry.grab_focus();
    trace.mark("presented");
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
    store.recent(history_limit()).unwrap_or_else(|e| {
        eprintln!("copycat: failed to read history: {e}");
        Vec::new()
    })
}

/// Build the clip list as a *virtualised* view.
///
/// This is the difference between a popup that opens instantly and one that
/// does not. The obvious construction — one `AdwActionRow` per clip, appended
/// to a `ListBox` — measured at ~8.8 ms per row on the development machine, so
/// a 91-clip history spent 836 ms laying out rows before the window appeared,
/// and the 500-clip cap would have taken roughly four seconds.
///
/// A [`ListView`] instead creates widgets only for the rows actually on screen
/// (about a dozen here) and recycles them as you scroll, so startup cost stops
/// depending on how much history you have kept. The same applies to every
/// keystroke in the search box, which re-populates this model.
fn build_list() -> (StringList, SingleSelection, ListView) {
    let model = StringList::new(&[]);
    let selection = SingleSelection::new(Some(model.clone()));

    let factory = SignalListItemFactory::new();
    // `setup` runs once per *recycled widget*, not once per clip.
    factory.connect_setup(|_, item| {
        let label = Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(pango::EllipsizeMode::End);
        label.set_margin_top(ROW_PADDING);
        label.set_margin_bottom(ROW_PADDING);
        label.set_margin_start(ROW_PADDING);
        label.set_margin_end(ROW_PADDING);
        if let Some(item) = item.downcast_ref::<ListItem>() {
            item.set_child(Some(&label));
        }
    });
    // `bind` runs when a recycled widget is pointed at a different clip.
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let text = item
            .item()
            .and_downcast::<StringObject>()
            .map(|s| s.string());
        let label = item.child().and_downcast::<Label>();
        if let (Some(text), Some(label)) = (text, label) {
            // `set_text`, never `set_markup`: clip text is arbitrary user data
            // and must not be parsed as Pango markup.
            label.set_text(&text);
        }
    });

    let list = ListView::new(Some(selection.clone()), Some(factory));
    // Match the old ListBox feel, where a single click chose a clip.
    list.set_single_click_activate(true);
    (model, selection, list)
}

/// Replace the visible clips, select the first, and show either the list or
/// the empty state.
fn populate(model: &StringList, selection: &SingleSelection, stack: &Stack, clips: &[Clip]) {
    let rows: Vec<String> = clips.iter().map(|clip| one_line(&clip.preview)).collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    // One splice rather than a remove/append per row, so the view recomputes
    // its layout once instead of once per clip.
    model.splice(0, model.n_items(), &rows);

    if !clips.is_empty() {
        selection.set_selected(0);
    }
    stack.set_visible_child_name(if clips.is_empty() {
        PAGE_EMPTY
    } else {
        PAGE_LIST
    });
}

/// Move the selection by `delta`, clamped to the list, keeping it in view.
fn move_selection(list: &ListView, selection: &SingleSelection, delta: i32) {
    let count = selection.n_items();
    if count == 0 {
        return;
    }
    let current = match selection.selected() {
        NO_SELECTION => 0,
        index => index,
    };
    let next = next_index(current, delta, count);
    selection.set_selected(next);
    // Virtualised rows are only built when visible, so moving the selection
    // off-screen has to be paired with scrolling it back into view.
    list.scroll_to(next, ListScrollFlags::FOCUS, None);
}

/// Where the selection lands after moving `delta` from `current` in a list of
/// `count` items, clamped to the ends.
///
/// Pure, so the saturating arithmetic is tested without a display. `count` is
/// assumed non-zero; callers return early on an empty list.
fn next_index(current: u32, delta: i32, count: u32) -> u32 {
    let last = count.saturating_sub(1);
    let next = current as i64 + delta as i64;
    next.clamp(0, last as i64) as u32
}

/// The popup's X11 window id, if we are running on X11.
fn window_xid(window: &adw::ApplicationWindow) -> Option<u32> {
    use gdk4_x11::X11Surface;
    window
        .surface()?
        .downcast::<X11Surface>()
        .ok()
        .map(|surface| surface.xid() as u32)
}

/// Hide the popup, wait until the clipboard really is ours, hand focus back to
/// where the user came from, paste for them, and only then quit.
///
/// The ordering is the whole point. Claiming the X11 selection is asynchronous:
/// pasting too early makes the target app ask for a clipboard we do not own yet
/// and receive nothing. So rather than guessing a delay we watch for the
/// selection owner to change away from `previous_owner`, which is the moment
/// our content became available. We also stay alive past the keystroke, since
/// the target reads the clipboard *from us*.
///
/// If any step is unavailable the clip is still on the clipboard to paste by
/// hand, which is the pre-existing behaviour.
fn dismiss_after_copy(window: &adw::ApplicationWindow, previous_owner: Option<u32>) {
    window.set_visible(false);
    let target = PASTE_TARGET.with(|target| target.get());
    let window = window.clone();
    let waited = Rc::new(Cell::new(std::time::Duration::ZERO));

    glib::timeout_add_local(OWNERSHIP_POLL, move || {
        let ours = x11util::clipboard_owner() != previous_owner;
        let elapsed = waited.get() + OWNERSHIP_POLL;
        waited.set(elapsed);

        if !ours && elapsed < OWNERSHIP_TIMEOUT {
            return glib::ControlFlow::Continue;
        }
        if !ours {
            eprintln!("copycat: clipboard ownership did not settle; pasting anyway");
        }

        paste_and_quit(&window, target);
        glib::ControlFlow::Break
    });
}

/// Wait for the window manager to hand focus back, then synthesize the paste
/// and close once the target has had a chance to read the clipboard from us.
///
/// We deliberately do not force focus ourselves. Hiding the popup already makes
/// Mutter re-activate whatever the user was working in, and intervening only
/// risks pointing at an internal child window and breaking the restoration the
/// window manager performed correctly. So we watch `_NET_ACTIVE_WINDOW` until
/// it is something other than us, and type into that.
fn paste_and_quit(window: &adw::ApplicationWindow, target: Option<u32>) {
    let close_later = {
        let window = window.clone();
        move || {
            let window = window.clone();
            glib::timeout_add_local_once(CLIPBOARD_SETTLE, move || window.close());
        }
    };

    let Some(target) = target else {
        eprintln!("copycat: no window to paste into; clip is on the clipboard");
        close_later();
        return;
    };

    // Ask for the window back explicitly rather than hoping focus drifts home.
    x11util::activate_window(target);

    let ours = window_xid(window);
    let waited = Rc::new(Cell::new(std::time::Duration::ZERO));

    glib::timeout_add_local(FOCUS_POLL, move || {
        let elapsed = waited.get() + FOCUS_POLL;
        waited.set(elapsed);

        // Both conditions matter: the window manager must consider the target
        // active, *and* raw input focus must actually have left our popup.
        // _NET_ACTIVE_WINDOW alone updates slightly early, and a keystroke sent
        // in that gap goes nowhere.
        let active_is_target = x11util::active_window() == Some(target);
        let focus_left_us = match (x11util::input_focus(), ours) {
            (Some(focus), Some(ours)) => focus != ours,
            _ => true,
        };

        if !(active_is_target && focus_left_us) {
            if elapsed < FOCUS_TIMEOUT {
                return glib::ControlFlow::Continue;
            }
            eprintln!(
                "copycat: {} never regained focus; skipping auto-paste, \
                 clip is on the clipboard",
                x11util::window_label(target)
            );
            close_later();
            return glib::ControlFlow::Break;
        }

        match x11util::send_paste() {
            Some(()) => eprintln!("copycat: pasted into {}", x11util::window_label(target)),
            None => eprintln!("copycat: could not synthesize paste; clip is on the clipboard"),
        }
        close_later();
        glib::ControlFlow::Break
    });
}

/// Copy the currently selected clip's full content back to the clipboard.
///
/// Returns whoever owned the clipboard beforehand, so the caller can tell when
/// our own claim has taken effect. `None` means nothing was copied.
fn copy_selected(selection: &SingleSelection, ranked: &[Clip]) -> Option<Option<u32>> {
    let index = match selection.selected() {
        NO_SELECTION => {
            eprintln!("copycat: nothing selected, clipboard left alone");
            return None;
        }
        index => index,
    };
    let clip = ranked.get(index as usize).or_else(|| {
        eprintln!("copycat: selected row {index} has no matching clip, clipboard left alone");
        None
    })?;
    let display = gdk::Display::default().or_else(|| {
        eprintln!("copycat: no display, cannot set the clipboard");
        None
    })?;

    let previous_owner = x11util::clipboard_owner();
    display.clipboard().set_text(clip.text());
    eprintln!(
        "copycat: copied clip {} ({} chars) to the clipboard",
        clip.id,
        clip.text().chars().count()
    );
    Some(previous_owner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_override_uses_the_built_in_limit() {
        assert_eq!(parse_history_limit(None), HISTORY_LIMIT);
    }

    #[test]
    fn an_override_is_honoured() {
        assert_eq!(parse_history_limit(Some("25")), 25);
        assert_eq!(parse_history_limit(Some("  40 ")), 40);
    }

    #[test]
    fn zero_is_a_legitimate_override() {
        // Useful for isolating row-building cost from everything else.
        assert_eq!(parse_history_limit(Some("0")), 0);
    }

    #[test]
    fn a_nonsense_override_falls_back_instead_of_failing() {
        for bad in ["", "lots", "-5", "3.5"] {
            assert_eq!(
                parse_history_limit(Some(bad)),
                HISTORY_LIMIT,
                "{bad:?} should fall back"
            );
        }
    }

    #[test]
    fn selection_moves_by_one_in_each_direction() {
        assert_eq!(next_index(3, 1, 10), 4);
        assert_eq!(next_index(3, -1, 10), 2);
    }

    #[test]
    fn selection_stops_at_the_ends_rather_than_wrapping() {
        assert_eq!(next_index(0, -1, 10), 0, "up from the top stays put");
        assert_eq!(next_index(9, 1, 10), 9, "down from the bottom stays put");
    }

    #[test]
    fn a_single_item_list_has_nowhere_to_go() {
        assert_eq!(next_index(0, 1, 1), 0);
        assert_eq!(next_index(0, -1, 1), 0);
    }

    #[test]
    fn a_stale_index_past_the_end_is_pulled_back_into_range() {
        // The list shrinks as you type, so the old selection can be too big.
        assert_eq!(next_index(500, 1, 10), 9);
    }

    #[test]
    fn large_jumps_do_not_overflow() {
        // u32 index plus i32 delta must not wrap around in either direction.
        assert_eq!(next_index(0, i32::MIN, 10), 0);
        assert_eq!(next_index(u32::MAX - 1, i32::MAX, 10), 9);
    }
}
