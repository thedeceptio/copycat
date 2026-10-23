//! X11-specific helpers the popup needs and GTK4 cannot provide.
//!
//! Two things live here:
//!
//! * **Placing the popup.** GTK4 deliberately removed window positioning, so to
//!   open next to the pointer we move our own toplevel through X11 directly.
//! * **Pasting for you.** After putting a clip on the clipboard we hand focus
//!   back to the window you came from and synthesize `Ctrl+V` via XTEST.
//!
//! All of this is X11-only. On Wayland neither is permitted this way, so every
//! entry point degrades to a no-op and the popup falls back to "copy, you
//! paste" — see SPEC §5.2.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConfigureWindowAux, ConnectionExt as _, InputFocus, Window, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// X11 keysym for the `v` key.
const KEYSYM_V: u32 = 0x0076;
/// X11 keysym for the left Control modifier.
const KEYSYM_CONTROL_L: u32 = 0xffe3;

/// Margin kept between the popup and the screen edge, in pixels.
const SCREEN_MARGIN: i32 = 8;

/// A short-lived X11 connection. Returns `None` off X11 (e.g. on Wayland).
fn connect() -> Option<(RustConnection, usize)> {
    x11rb::connect(None).ok()
}

/// The window that currently holds input focus, i.e. where the user was typing
/// before the popup appeared.
pub fn focused_window() -> Option<Window> {
    let (conn, _) = connect()?;
    let focus = conn.get_input_focus().ok()?.reply().ok()?.focus;
    // PointerRoot/None are not real windows we can hand focus back to.
    if focus == x11rb::NONE || focus == 1 {
        return None;
    }
    Some(focus)
}

/// Move `xid` so a `width` x `height` popup sits just below-right of the
/// pointer, nudged back on-screen if that would overflow.
pub fn place_at_pointer(xid: u32, width: i32, height: i32) -> Option<()> {
    let (conn, screen_num) = connect()?;
    let screen = conn.setup().roots.get(screen_num)?;
    let root = screen.root;
    let (screen_w, screen_h) = (
        screen.width_in_pixels as i32,
        screen.height_in_pixels as i32,
    );

    let pointer = conn.query_pointer(root).ok()?.reply().ok()?;
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

/// Give `target` focus again and synthesize `Ctrl+V` into it.
///
/// Returns `None` if anything along the way is unavailable, in which case the
/// clip is still on the clipboard for the user to paste themselves.
pub fn paste_into(target: Window) -> Option<()> {
    let (conn, screen_num) = connect()?;
    let root = conn.setup().roots.get(screen_num)?.root;

    let ctrl = keycode_for(&conn, KEYSYM_CONTROL_L)?;
    let v = keycode_for(&conn, KEYSYM_V)?;

    // Hand the keyboard back to where the user was working first; otherwise the
    // synthetic keys land on whatever has focus now.
    conn.set_input_focus(InputFocus::PARENT, target, x11rb::CURRENT_TIME)
        .ok()?;
    conn.flush().ok()?;

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
