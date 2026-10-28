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

- **UI toolkit:** `gtk4-rs` + `libadwaita`, built natively — **LOCKED (revised 2026-10-23).**
  The dev machine was upgraded from jammy to **Ubuntu 24.04 (noble): GTK 4.14.5 /
  libadwaita 1.5.0**, i.e. the ship target itself. Bindings are therefore **modern**:
  `gtk4` 0.8 and `libadwaita` 0.6 with the `v1_5` feature. Dev machine == CI == ship target,
  so there is no binding compromise and no container.

  *Superseded:* the earlier jammy-era decisions to pin bindings down to GTK 4.6 and to defer
  libadwaita were both driven purely by the old OS and no longer apply; the Docker-24.04
  alternative is moot for the same reason.
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

2. **Auto-paste: shipped on X11 (revised 2026-10-27); copy-only elsewhere.**
   Originally deferred as P1 on the assumption that synthetic input needed the Wayland
   RemoteDesktop portal. On X11 it needs no portal at all: XTEST is available, so `Enter`
   now copies *and* pastes. The popup records the window manager's active window, asks for it
   back via `_NET_ACTIVE_WINDOW`, waits until both the WM reports it active and raw input
   focus has left the popup, then synthesizes `Ctrl+V`.

   Hard-won implementation notes — each of these caused a silent failure:
   - The clipboard claim is asynchronous. Poll until the selection owner changes before
     pasting, or the target asks for content we do not own yet and gets nothing.
   - `GetInputFocus` returns unnamed internal child windows for many toolkits. Use
     `_NET_ACTIVE_WINDOW` for the toplevel the window manager actually manages.
   - XTEST events carry a server-side delay; dropping the X connection while they are
     pending discards them. Round-trip before letting the connection go.

   Known limits: terminals use `Ctrl+Shift+V`, so auto-paste does nothing there (the clip is
   still on the clipboard). If focus does not return cleanly the paste is skipped rather than
   risking the wrong window. On Wayland this degrades to copy-only, as originally planned.

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

### M2 — Popup + fuzzy search + paste (the core loop) — **COMPLETE (2026-10-27)**
*Goal: the primary interaction works end-to-end for text.*
- `Super+V` → compact chrome-free popup **at the mouse pointer**, search focused (P0 #4).
  Bound through a GNOME custom shortcut by `scripts/install-desktop.sh`; the
  `GlobalShortcuts` portal remains the eventual Wayland-friendly route.
- Fuzzy incremental search, keyboard nav, `Enter` (P0 #5); `Esc` dismiss (P0 #6).
- `Enter` copies **and pastes** into the window you came from (see §5.2.2).
- The open popup live-refreshes as the daemon captures new clips, so it is never a stale
  snapshot.
- No D-Bus IPC was needed: the popup reads the same SQLite history the daemon writes, and
  owns the clipboard itself. Dropped from scope rather than built.
- **Achieved:** `Super+V`, type, `Enter` — the clip lands in the target application.
  Verified end to end against a real window manager, 3/3 runs.
- **The list must stay virtualised (revised 2026-10-28).** The popup first took ~900 ms to
  appear. The cost was not startup, the database or the linker — measured at 30 ms, 0.5 ms
  and 0.3 ms respectively — but one `AdwActionRow` per clip, at **~8.8 ms each**, laid out
  inside `window.present()`:

  | rows | 0 | 1 | 10 | 45 | 91 |
  | --- | --- | --- | --- | --- | --- |
  | GTK layout | 34 ms | 72 ms | 114 ms | 363 ms | **836 ms** |

  Linear in history size, so the 500-clip cap would have cost roughly four seconds, and it
  was re-paid on every keystroke in the search box. Fixed by moving to a GTK4 `ListView`,
  which builds widgets only for the visible rows and recycles them. Cost now scales with
  window size, not history size. Anything that reintroduces one widget per clip reintroduces
  this bug — and it hides well, because it is invisible until the history fills up.
- Guessing was expensive here, twice over: a plausible accessibility-bus error and the GL
  renderer were both wrong, and both were eliminated in one run each by varying a single
  factor. `COPYCAT_TRACE=1` exists so the next such question starts with a measurement.

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

**Platform reality & risks (updated 2026-10-23)**
- **Dev machine is Ubuntu 24.04.5 (noble), GNOME Shell 46, running an X11 session**
  (upgraded from jammy on 2026-10-23). GTK 4.14.5 / libadwaita 1.5.0 — the ship target itself,
  so the GUI is built natively against modern bindings (see §5.1). The earlier jammy-era
  constraints (GTK 4.6 / libadwaita 1.1, container workarounds) no longer apply.
- **Backend order is X11-first.** The dev session is X11, so the X11/XFixes capture path is
  built and validated first (pure-Rust `x11rb`, no system `-dev` libs). The Wayland backend
  follows and is still unimplemented.
- **GNOME + Wayland clipboard-watch remains the biggest open risk for the P0 daemon.**
  GNOME/Mutter never implemented `wlr-data-control`; background clipboard watching relies on
  `ext-data-control-v1`, which landed only in Mutter 47+ (GNOME 47+). **Confirmed: this machine
  runs GNOME 46, so it is unavailable here** — as is the case for Ubuntu 24.04 LTS generally.
  The Wayland backend must detect `ext-data-control` at runtime and, where absent, fall back
  (revisit: a minimal GNOME Shell helper, or document Wayland support as GNOME 47+/KDE/wlroots).
  Not blocking today because the dev session is X11.

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
