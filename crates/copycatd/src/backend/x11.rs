//! X11 clipboard capture via the XFIXES extension.
//!
//! We create a tiny unmapped window, ask XFIXES to notify us whenever the
//! `CLIPBOARD` selection owner changes, and on each change request the content
//! as UTF-8 text. New text is written to the [`Store`], which handles duplicate
//! collapsing and the history cap.
//!
//! Large transfers (the `INCR` protocol) and non-text targets are skipped for
//! now; images/rich text/files arrive in M3.

use crate::capture;
use copycat_core::{NewClip, Store};
use std::error::Error;
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{self, ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, SelectionNotifyEvent,
    WindowClass,
};
use x11rb::protocol::Event;
use x11rb::COPY_DEPTH_FROM_PARENT;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Run the X11 capture loop, storing new clipboard text into `store`.
///
/// Blocks until the connection drops or an error occurs. Returns `Ok(())` only
/// if the loop ends cleanly (e.g. the X server goes away).
pub fn run(store: &Store) -> Result<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    // XFIXES must be negotiated before its requests are usable.
    xfixes::query_version(&conn, 5, 0)?.reply()?;

    let atoms = Atoms::intern(&conn)?;

    // A minimal, never-mapped window to own selection requests/replies.
    let window = conn.generate_id()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?;

    conn.xfixes_select_selection_input(
        window,
        atoms.clipboard,
        SelectionEventMask::SET_SELECTION_OWNER
            | SelectionEventMask::SELECTION_WINDOW_DESTROY
            | SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )?;
    conn.flush()?;

    eprintln!("copycatd: watching CLIPBOARD (X11/XFIXES). Ctrl+C to stop.");

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::XfixesSelectionNotify(ev) if ev.selection == atoms.clipboard => {
                if ev.owner == x11rb::NONE {
                    continue; // selection was cleared
                }
                // Ask the new owner to hand us the content as UTF-8.
                conn.convert_selection(
                    window,
                    atoms.clipboard,
                    atoms.utf8_string,
                    atoms.dest_property,
                    ev.timestamp,
                )?;
                conn.flush()?;
            }
            Event::SelectionNotify(ev) => {
                if let Some(text) = read_text(&conn, &atoms, ev)? {
                    if capture::should_store_text(&text) {
                        let clip = NewClip::text(&text);
                        match store.insert(&clip) {
                            Ok(_) => eprintln!(
                                "copycatd: captured {} chars: {:?}",
                                text.chars().count(),
                                clip.preview
                            ),
                            Err(e) => eprintln!("copycatd: store error: {e}"),
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Read the converted selection out of our destination property.
///
/// Returns `None` when the conversion failed (non-text target), the property is
/// empty, or the data uses the `INCR` chunked protocol (not yet supported).
fn read_text(
    conn: &impl Connection,
    atoms: &Atoms,
    ev: SelectionNotifyEvent,
) -> Result<Option<String>> {
    if ev.property == x11rb::NONE {
        return Ok(None); // owner could not provide UTF-8
    }

    let reply = conn
        .get_property(
            true, // delete the property once read
            ev.requestor,
            ev.property,
            AtomEnum::ANY,
            0,
            u32::MAX,
        )?
        .reply()?;

    if reply.type_ == atoms.incr {
        // Large transfer; supporting INCR is future work.
        return Ok(None);
    }
    if reply.value.is_empty() {
        return Ok(None);
    }

    Ok(Some(String::from_utf8_lossy(&reply.value).into_owned()))
}

/// Interned atoms the backend needs.
struct Atoms {
    clipboard: Atom,
    utf8_string: Atom,
    dest_property: Atom,
    incr: Atom,
}

impl Atoms {
    fn intern(conn: &impl Connection) -> Result<Self> {
        Ok(Atoms {
            clipboard: intern(conn, b"CLIPBOARD")?,
            utf8_string: intern(conn, b"UTF8_STRING")?,
            dest_property: intern(conn, b"COPYCAT_CLIP")?,
            incr: intern(conn, b"INCR")?,
        })
    }
}

fn intern(conn: &impl Connection, name: &[u8]) -> Result<Atom> {
    Ok(conn.intern_atom(false, name)?.reply()?.atom)
}
