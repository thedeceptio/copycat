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
    AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, Window,
    KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// X11 keysym for the `v` key.
const KEYSYM_V: u32 = 0x0076;
/// X11 keysym for the left Control modifier.
const KEYSYM_CONTROL_L: u32 = 0xffe3;

/// Modifiers that must not be held when we synthesize `Ctrl+V`: a stuck Super
/// (from the hotkey that summoned us) or Shift turns the combination into
/// something the target will not treat as paste. Each entry is the mask bit
/// reported by the server and the keysym to release.
const MODIFIERS_TO_CLEAR: [(u16, u32); 4] = [
    (0x0001, 0xffe1), // Shift   -> Shift_L
    (0x0004, 0xffe3), // Control -> Control_L
    (0x0008, 0xffe9), // Mod1    -> Alt_L
    (0x0040, 0xffeb), // Mod4    -> Super_L
];

/// Delay in milliseconds attached to each synthesized key event.
///
/// XTEST delivers events as fast as we queue them, which puts the whole
/// press/release sequence in a single instant. Chromium-based apps in
/// particular can miss a combination delivered that way, so the events are
/// spaced out the way a real keyboard would produce them.
const KEY_DELAY_MS: u32 = 12;

/// Margin kept between the popup and the screen edge, in pixels.
const SCREEN_MARGIN: i32 = 8;

/// A short-lived X11 connection. Returns `None` off X11 (e.g. on Wayland).
fn connect() -> Option<(RustConnection, usize)> {
    x11rb::connect(None).ok()
}

fn intern(conn: &impl Connection, name: &[u8]) -> Option<u32> {
    Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
}

/// The toplevel window the window manager considers active.
///
/// This is deliberately *not* `GetInputFocus`: that reports whichever window
/// holds focus, which for many toolkits is an internal child with no name that
/// the window manager does not manage. Acting on it means targeting the wrong
/// window. `_NET_ACTIVE_WINDOW` is the window manager's own answer to "where is
/// the user working", which is what we actually want.
pub fn active_window() -> Option<Window> {
    let (conn, screen_num) = connect()?;
    let root = conn.setup().roots.get(screen_num)?.root;
    let atom = intern(&conn, b"_NET_ACTIVE_WINDOW")?;
    let reply = conn
        .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    let window = reply.value32()?.next()?;
    (window != x11rb::NONE).then_some(window)
}

/// A human-readable label for `window`, for diagnostics only.
pub fn window_label(window: Window) -> String {
    let Some((conn, _)) = connect() else {
        return "<no display>".to_string();
    };
    let class = intern(&conn, b"WM_CLASS")
        .and_then(|atom| {
            conn.get_property(false, window, atom, AtomEnum::STRING, 0, 64)
                .ok()?
                .reply()
                .ok()
        })
        .map(|reply| {
            String::from_utf8_lossy(&reply.value)
                .split('\0')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(".")
        })
        .unwrap_or_default();
    if class.is_empty() {
        format!("{window} <unnamed>")
    } else {
        format!("{window} ({class})")
    }
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

/// Ask the window manager to activate `target`, the way `xdotool
/// windowactivate` does.
///
/// Waiting passively for the window manager to restore focus is not enough:
/// `_NET_ACTIVE_WINDOW` is updated slightly before keyboard focus actually
/// transfers, so a keystroke sent on that signal alone falls into the gap.
/// Requesting activation explicitly — and then confirming input focus has
/// really moved — is what makes the paste land.
pub fn activate_window(target: Window) -> Option<()> {
    let (conn, screen_num) = connect()?;
    let root = conn.setup().roots.get(screen_num)?.root;
    let atom = intern(&conn, b"_NET_ACTIVE_WINDOW")?;

    let event = ClientMessageEvent::new(
        32,
        target,
        atom,
        // Source "pager", so the window manager honours the request instead of
        // applying focus-stealing prevention.
        [2u32, x11rb::CURRENT_TIME, 0, 0, 0],
    );
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
        event,
    )
    .ok()?;
    conn.flush().ok()?;
    Some(())
}

/// The window holding raw input focus. Unlike [`active_window`] this may be an
/// internal child, so it is only useful for confirming focus has left us.
pub fn input_focus() -> Option<Window> {
    let (conn, _) = connect()?;
    let focus = conn.get_input_focus().ok()?.reply().ok()?.focus;
    (focus != x11rb::NONE && focus != 1).then_some(focus)
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

    // Whatever the user was holding (very likely Super, which summoned us) is
    // still part of the modifier state and would corrupt the combination.
    let held = u16::from(conn.query_pointer(root).ok()?.reply().ok()?.mask);
    if held != 0 {
        eprintln!("copycat: clearing held modifiers (mask {held:#06x}) before pasting");
    }
    for (bit, keysym) in MODIFIERS_TO_CLEAR {
        if held & bit != 0 {
            if let Some(keycode) = keycode_for(&conn, keysym) {
                let _ =
                    conn.xtest_fake_input(KEY_RELEASE_EVENT, keycode, KEY_DELAY_MS, root, 0, 0, 0);
            }
        }
    }

    for (event, keycode) in [
        (KEY_PRESS_EVENT, ctrl),
        (KEY_PRESS_EVENT, v),
        (KEY_RELEASE_EVENT, v),
        (KEY_RELEASE_EVENT, ctrl),
    ] {
        conn.xtest_fake_input(event, keycode, KEY_DELAY_MS, root, 0, 0, 0)
            .ok()?;
    }
    conn.flush().ok()?;

    // Round-trip before this connection is dropped. Each event above carries a
    // server-side delay, and tearing the connection down while they are still
    // pending loses them — which is exactly how a paste that "succeeded" could
    // never arrive. Waiting for a reply proves the server worked through them.
    conn.get_input_focus().ok()?.reply().ok()?;
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
