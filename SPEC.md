# copycat — MVP Feature Spec

**A Wayland-first clipboard manager for Ubuntu: native look, fast recall, private by default.**

Status: draft · Last updated: 2026-09-29

---

## 1. Product summary

copycat keeps a searchable history of what you copy and lets you paste any past item
back in two keystrokes. It targets the gap found in market research: **CopyQ's capability
with GPaste's native feel, minus the Wayland fragility and GNOME-upgrade breakage of both.**

- **Form factor:** standalone GTK4/libadwaita app + background daemon (survives GNOME
  upgrades, unlike shell extensions).
- **Primary interaction:** global hotkey → fuzzy-search popup → Enter to paste (keyboard-first,
  search-first — not a tray-dropdown as the *main* path). A system tray icon is provided as a
  secondary access point (see P1 #16).
- **Platform target:** Ubuntu 24.04+ on **Wayland first**, X11 as a supported fallback.

---

## 2. Goals & non-goals

**MVP goals**
- Capture clipboard history reliably on Wayland *and* X11.
- Instant recall via hotkey + fuzzy search.
- Support the content types people actually lose: text, rich text, images, file paths.
- Pinning and privacy defaults that build trust from day one.

**Explicit non-goals for MVP** (parked for later)
- Scripting/automation engine (CopyQ-style commands).
- Cloud/device sync.
- Multiple tabs/categories/tagging.
- Snippet templates / macros.
- KDE/other-desktop polish (should *work*, but GNOME is the design target).

---

## 3. MVP feature set

### P0 — must ship

| # | Feature | Detail |
|---|---|---|
| 1 | **Clipboard capture daemon** | Watches the clipboard, stores each new copy. Wayland via `wlr-data-control`; X11 fallback via XFixes. Runs as a user systemd service, autostarts on login. |
| 2 | **History store** | Local, persistent across reboots. Ring buffer with configurable cap (default 500 items). |
| 3 | **Multi-format capture** | Plain text, rich text (HTML), images (PNG), and file/URI lists. Store the richest available representation; paste back in the right format. |
| 4 | **Global hotkey → search popup** | Default `Super+V`. Opens a centered popup with the search box focused. |
| 5 | **Fuzzy incremental search** | Type-to-filter across history; ranked by relevance + recency. Arrow keys to navigate, Enter to paste. |
| 6 | **Paste-back** | Selecting an item puts it on the clipboard and (where possible) auto-pastes into the focused app. Fallback: copy + user pastes. |
| 7 | **Pinning / favorites** | Pin items so they never expire and sort to the top. |
| 8 | **Delete & clear** | Delete a single item; clear all history (one-click, with confirm). |
| 9 | **Privacy defaults** | Detect & skip password-manager clipboards (concealed-type mimetypes / `x-kde-passwordManagerHint`); exclude-list by app; optional auto-expiry of unpinned items (default off, configurable). |
| 10 | **Native UI** | GTK4 + libadwaita. Light/dark following system theme. |

### P1 — should ship if time allows

| # | Feature | Detail |
|---|---|---|
| 11 | Duplicate collapsing | Re-copying an existing item bumps it to top instead of duplicating. |
| 12 | Item preview | Larger preview pane for images / long text in the popup. |
| 13 | Content-type filter | Quick toggle: text / images / files. |
| 14 | Encrypted-at-rest option | Encrypt the local store (opt-in). |
| 15 | Basic preferences UI | History size, hotkey, autostart, exclude-list, expiry. |
| 16 | **System tray icon + menu** | Secondary access point to the popup. Shows the app is running; menu with: open history popup, recent clips (quick-pick shortlist), clear history, preferences, quit. Popup/hotkey remains the primary path. |

### P2 — post-MVP backlog
Scripting, sync, tabs/tags, snippet templates, OCR on images, KDE-native integration.

---

## 4. Key UX flows

**Capture (invisible):** user copies anything → daemon records it. No interaction.

**Recall & paste:**
1. `Super+V` → popup opens, cursor in search box.
2. User types a few chars → list filters live.
3. `↑/↓` to pick, `Enter` → popup closes, content pastes into the previously-focused window.
4. `Esc` → dismiss, nothing changes.

**Pin:** select an item → `Ctrl+P` (or pin icon) → marked, sorts to top, exempt from expiry.

**Privacy:** copying from a recognized password manager is silently ignored; a first-run note explains this.

---

## 5. Technical decisions

### 5.1 Implementation language — **LOCKED: Rust**

Chosen over Vala and Python. Rationale: the project's hardest, highest-risk parts are a
**Wayland protocol binding** (`wlr-data-control`) and a **reliable always-on daemon** with a
small footprint — exactly where Rust is strongest (`wayland-client`/`wayland-protocols`,
compile-time safety, no GC, ~5–15 MB resident). Python's clean path forces shelling out to
`wl-clipboard` (fragile, weak on images/multi-format); Vala is idiomatic for GNOME but has a
small contributor pool and more manual Wayland interop. Rust's one real cost — slower to write —
is a one-time expense against a long-lived daemon.

- **UI toolkit:** `gtk4-rs` + `libadwaita` bindings.
  - **M2 dev toolchain — LOCKED: native host, bindings pinned to the host GTK.** The dev
    machine is jammy (GTK 4.6 / libadwaita 1.1), so the Rust bindings are pinned down to match
    (older `gtk4`/`libadwaita` crate versions) and the GUI is built/run natively — no container.
    The ship target stays 24.04+ (GTK 4.14 / libadwaita 1.5); older bindings are forward-
    compatible, so the same code builds on the newer target and in CI. Trade-off accepted:
    dated bindings now in exchange for the simplest native loop; modern-binding uplift is
    deferred. (Docker-24.04 was the considered alternative; rejected for setup overhead.)
- **Wayland clipboard:** `wayland-client` + protocol bindings generated from `wlr-data-control` XML.
- **X11 fallback:** XFixes clipboard notifications.
- **Storage:** SQLite (`rusqlite`) for metadata + text; image blobs on disk referenced by hash (`sha2`).
- **Fuzzy search:** `fuzzy-matcher` (or `nucleo`).

### 5.2 Distribution & permissions — **LOCKED**

1. **Distribution: `.deb` + PPA for MVP; Flatpak deferred to P2.**
   A clipboard manager fights the Flatpak sandbox by design — `wlr-data-control` (P0 #1)
   reads *all* clipboard activity without focus, exactly what sandboxing forbids, forcing
   broad socket holes that defeat the point. A `.deb` gives the daemon unobstructed access to
   the compositor socket, matches Ubuntu-user expectations, and keeps sandbox debugging off
   the critical path. Revisit Flatpak once the portal story matures.

2. **Auto-paste: copy-only for MVP; true auto-paste → P1 opt-in.**
   Selecting an item puts it on the clipboard; the user presses `Ctrl+V`. True auto-paste
   requires faking a keystroke, which on Wayland is only sanctioned via the RemoteDesktop
   portal (synthetic input) or a bundled tool like `ydotool` — extra permission prompts and a
   fragile dependency not worth blocking MVP.

3. **Global hotkey: `org.freedesktop.portal.GlobalShortcuts` portal (primary), GNOME
   custom-shortcut fallback (documented).**
   Portal is packaging-agnostic and future-proof; the custom-shortcut fallback (bind a key in
   GNOME Settings to launch/toggle the popup) covers environments the portal doesn't yet reach.

---

## 6. Milestone breakdown

Build order is a **walking skeleton first**: get an end-to-end "copy something → find it →
paste it back" loop working on text as early as possible (M1–M2), then widen. Each milestone is
independently demoable.

### M0 — Project scaffold
*Goal: it builds and runs, nothing else.*
- Cargo **workspace**: `copycatd` (daemon) + `copycat` (GTK app) crates, shared `copycat-core` lib.
- Dependencies wired: `gtk4`, `libadwaita`, `wayland-client`, `rusqlite`, fuzzy matcher.
- `copycatd` starts, logs, exits cleanly; `copycat` opens an empty libadwaita window.
- **Demo:** both binaries run. **Exit criteria:** CI builds both crates.

### M1 — Capture text to storage (daemon)
*Goal: the daemon silently records what you copy.*
- `wlr-data-control` capture loop on Wayland; **XFixes fallback** on X11 (P0 #1).
- SQLite schema + write path; ring buffer cap (default 500) (P0 #2).
- Duplicate collapsing so re-copying bumps instead of duplicating (P1 #11, pulled early — it
  keeps the store clean for everything downstream).
- **Demo:** copy text around the desktop, then `sqlite3` shows the history. **Exit criteria:**
  100% of text copies captured in a GNOME-Wayland session, survives daemon restart.

### M2 — Popup + fuzzy search + copy-back (the core loop)
*Goal: the primary interaction works end-to-end for text.*
- `GlobalShortcuts` portal hotkey (`Super+V`) → centered libadwaita popup, search focused (P0 #4).
- Daemon↔app IPC (D-Bus) so the popup reads history and requests actions.
- Fuzzy incremental search, keyboard nav, `Enter` (P0 #5); copy-back to clipboard, `Esc` dismiss (P0 #6).
- GNOME custom-shortcut fallback documented.
- **Demo:** `Super+V`, type, Enter, `Ctrl+V` pastes the old clip. **Exit criteria:** popup <150 ms,
  live results; the walking skeleton is complete.

### M3 — Multi-format (images, rich text, files)
*Goal: stop losing non-text content.*
- Capture HTML/rich text, PNG images (blobs on disk by hash), file/URI lists (P0 #3).
- Paste back in the correct format; image thumbnails + item preview in the popup (P1 #12).
- Content-type filter toggle: text / images / files (P1 #13).
- **Demo:** copy an image and a file, recall and paste both. **Exit criteria:** each type round-trips.

### M4 — Privacy, pinning, management
*Goal: the trust + curation features.*
- Skip password-manager clipboards (concealed mimetypes); per-app exclude-list (P0 #9).
- Optional auto-expiry of unpinned items (default off) (P0 #9).
- Pinning: pin/sort-to-top/expiry-exempt (P0 #7); delete one / clear all with confirm (P0 #8).
- **Demo:** copy from a password manager (ignored), pin a snippet, clear the rest.
  **Exit criteria:** password clips never stored; pins survive expiry + clear-unpinned.

### M5 — Polish, packaging, ship
*Goal: installable and self-managing.*
- Light/dark theme following system (P0 #10); first-run note explaining privacy behavior.
- Preferences UI: history size, hotkey, autostart, exclude-list, expiry (P1 #15).
- System tray icon + menu via AppIndicator (P1 #16).
- systemd user service + autostart on login; **`.deb` + PPA** packaging.
- **Demo:** `apt install`, log in, it just works. **Exit criteria:** all §7 success criteria met.

**Platform reality & risks (discovered 2026-09-29)**
- **Dev machine is Ubuntu 22.04 (jammy), GNOME 42, running an X11 session.** Its GTK4 (4.6)
  and libadwaita (1.1) are too old for modern Rust bindings; libadwaita work needs a newer
  runtime (24.04+ target, or a distrobox/Flatpak SDK for local GUI dev). This affects M2, not M1.
- **Backend order flipped to X11-first.** Because the dev session is X11, the X11/XFixes
  capture path is built and validated first (pure-Rust `x11rb`, no system `-dev` libs). The
  Wayland backend follows.
- **GNOME + Wayland clipboard-watch is a real risk for the P0 daemon.** GNOME/Mutter did not
  implement `wlr-data-control`; background clipboard watching relies on the newer
  `ext-data-control-v1`, which only landed in Mutter 47+ (GNOME 47+). Ubuntu 24.04 LTS ships
  GNOME 46, so the target may lack it — the Wayland backend must detect `ext-data-control`
  availability and, where absent, fall back (revisit: a minimal GNOME Shell helper, or document
  Wayland support as GNOME 47+/KDE/wlroots for now). To be resolved before/at the Wayland backend.

**Sequencing notes**
- M1→M2 is the critical path; everything else layers on the store + IPC they establish.
- P1 items are folded into the milestone where they're cheapest (11→M1, 12/13→M3, 15/16→M5),
  droppable under time pressure without breaking a milestone.
- Encrypted-at-rest (P1 #14) is intentionally left for post-MVP; it touches the storage layer
  and shouldn't destabilize M1.

---

## 7. Success criteria for MVP

- Captures 100% of copies in the default GNOME-Wayland session, no missed clips.
- Search popup opens in <150 ms and returns results as you type.
- Survives a GNOME point-release upgrade without reinstall.
- Password-manager clips never appear in history.
- Zero data leaves the machine.
