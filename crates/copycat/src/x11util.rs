//! X11-specific helpers the popup needs and GTK4 cannot provide.
//!
//! Three things live here:
//!
//! * **Placing the popup.** GTK4 deliberately removed window positioning, so to
//!   open next to the pointer we move our own toplevel through X11 directly.
//! * **Pasting for you.** After putting a clip on the clipboard we hand focus
//!   back to the window you came from and synthesize `Ctrl+V` via XTEST.
//! * **Knowing when the clipboard is really ours.** Claiming the selection is
//!   asynchronous; pasting before it completes gets the target nothing.
//!
//! All of this is X11-only. On Wayland neither positioning nor synthetic input
//! is permitted this way, so every entry point degrades to a no-op and the
//! popup falls back to "copy, you paste" — see SPEC §5.2.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, InputFocus, Window,
    KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// X11 keysym for the `v` key.
const KEYSYM_V: u32 = 0x0076;
/// X11 keysym for the left Control modifier.
const KEYSYM_CONTROL_L: u32 = 0xffe3;

/// Margin kept between the popup and the screen edge, in pixels.
const SCREEN_MARGIN: i32 = 8;

/// "Pager" source indication for `_NET_ACTIVE_WINDOW`, which asks the window
/// manager to honour the request rather than applying focus-stealing
/// prevention as it would for a normal application.
const SOURCE_PAGER: u32 = 2;

/// A short-lived X11 connection. Returns `None` off X11 (e.g. on Wayland).
fn connect() -> Option<(RustConnection, usize)> {
    x11rb::connect(None).ok()
}

fn intern(conn: &impl Connection, name: &[u8]) -> Option<u32> {
    Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
}

/// The window that currently holds input focus, i.e. where the user was typing
/// before the popup appeared.
pub fn focused_window() -> Option<Window> {
    let (conn, _) = connect()?;
    let focus = conn.get_input_focus().ok()?.reply().ok()?.focus;
    // PointerRoot (1) and None are not windows we can hand focus back to.
    if focus == x11rb::NONE || focus == 1 {
        return None;
    }
    Some(focus)
}

/// Whoever currently owns the CLIPBOARD selection, if anyone.
///
/// Used to tell when our own clipboard claim has actually landed: the owner
/// changing away from whoever held it before means the X server has processed
/// our request and a paste will now find our content.
pub fn clipboard_owner() -> Option<Window> {
    let (conn, _) = connect()?;
    let clipboard = intern(&conn, b"CLIPBOARD")?;
    let owner = conn
        .get_selection_owner(clipboard)
        .ok()?
        .reply()
        .ok()?
        .owner;
    (owner != x11rb::NONE).then_some(owner)
}

/// Move `xid` so a `width` x `height` popup sits at the pointer, nudged back
/// on-screen if that would overflow.
pub fn place_at_pointer(xid: u32, width: i32, height: i32) -> Option<()> {
    let (conn, screen_num) = connect()?;
    let screen = conn.setup().roots.get(screen_num)?;
    let (screen_w, screen_h) = (
        screen.width_in_pixels as i32,
        screen.height_in_pixels as i32,
    );

    let pointer = conn.query_pointer(screen.root).ok()?.reply().ok()?;
    let (x, y) = clamp_to_screen(
        pointer.root_x as i32,
        pointer.root_y as i32,
        width,
        height,
        screen_w,
        screen_h,
    );

    conn.configure_window(xid, &ConfigureWindowAux::new().x(x).y(y))
        .ok()?;
    conn.flush().ok()?;
    Some(())
}

/// Keep a `width` x `height` popup anchored near (`x`, `y`) fully on screen.
///
/// Pure, so the edge cases are unit-tested without a display.
pub fn clamp_to_screen(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    screen_w: i32,
    screen_h: i32,
) -> (i32, i32) {
    let max_x = (screen_w - width - SCREEN_MARGIN).max(SCREEN_MARGIN);
    let max_y = (screen_h - height - SCREEN_MARGIN).max(SCREEN_MARGIN);
    (x.clamp(SCREEN_MARGIN, max_x), y.clamp(SCREEN_MARGIN, max_y))
}

/// Hand the keyboard back to `target`.
///
/// Asks the window manager first via `_NET_ACTIVE_WINDOW` — the cooperative
/// route a compositing WM like Mutter respects — then sets input focus
/// directly as a fallback for anything that ignores the message.
pub fn focus_window(target: Window) -> Option<()> {
    let (conn, screen_num) = connect()?;
    let root = conn.setup().roots.get(screen_num)?.root;

    if let Some(active) = intern(&conn, b"_NET_ACTIVE_WINDOW") {
        let event = ClientMessageEvent::new(
            32,
            target,
            active,
            [SOURCE_PAGER, x11rb::CURRENT_TIME, 0, 0, 0],
        );
        let _ = conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
            event,
        );
    }

    let _ = conn.set_input_focus(InputFocus::PARENT, target, x11rb::CURRENT_TIME);
    conn.flush().ok()?;
    Some(())
}

/// Synthesize `Ctrl+V` into whatever currently holds focus.
///
/// Returns `None` if XTEST or the keymap lookup is unavailable, in which case
/// the clip is still on the clipboard for the user to paste themselves.
pub fn send_paste() -> Option<()> {
    let (conn, screen_num) = connect()?;
    let root = conn.setup().roots.get(screen_num)?.root;

    let ctrl = keycode_for(&conn, KEYSYM_CONTROL_L)?;
    let v = keycode_for(&conn, KEYSYM_V)?;

    for (event, keycode) in [
        (KEY_PRESS_EVENT, ctrl),
        (KEY_PRESS_EVENT, v),
        (KEY_RELEASE_EVENT, v),
        (KEY_RELEASE_EVENT, ctrl),
    ] {
        conn.xtest_fake_input(event, keycode, 0, root, 0, 0, 0)
            .ok()?;
    }
    conn.flush().ok()?;
    Some(())
}

/// Find the keycode that produces `keysym` on the current layout.
fn keycode_for(conn: &impl Connection, keysym: u32) -> Option<u8> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let mapping = conn
        .get_keyboard_mapping(min, max - min + 1)
        .ok()?
        .reply()
        .ok()?;
    let per_keycode = mapping.keysyms_per_keycode as usize;
    if per_keycode == 0 {
        return None;
    }
    mapping
        .keysyms
        .chunks(per_keycode)
        .position(|syms| syms.contains(&keysym))
        .map(|index| min + index as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: i32 = 400;
    const H: i32 = 300;
    const SW: i32 = 1920;
    const SH: i32 = 1080;

    #[test]
    fn pointer_well_inside_the_screen_is_used_as_is() {
        assert_eq!(clamp_to_screen(500, 400, W, H, SW, SH), (500, 400));
    }

    #[test]
    fn popup_is_pulled_back_from_the_right_and_bottom_edges() {
        let (x, y) = clamp_to_screen(SW - 10, SH - 10, W, H, SW, SH);
        assert_eq!(x, SW - W - SCREEN_MARGIN);
        assert_eq!(y, SH - H - SCREEN_MARGIN);
        assert!(x + W <= SW && y + H <= SH, "popup stays on screen");
    }

    #[test]
    fn popup_is_pushed_off_the_top_left_corner() {
        assert_eq!(
            clamp_to_screen(0, 0, W, H, SW, SH),
            (SCREEN_MARGIN, SCREEN_MARGIN)
        );
    }

    #[test]
    fn screen_smaller_than_the_popup_still_yields_a_sane_position() {
        // max_x/max_y would go negative; the margin floor must win.
        assert_eq!(
            clamp_to_screen(50, 50, 800, 600, 400, 300),
            (SCREEN_MARGIN, SCREEN_MARGIN)
        );
    }
}
