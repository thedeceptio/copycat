# copycat

A Wayland-first clipboard manager for Ubuntu: native look, fast recall, private by default.

copycat keeps a searchable history of what you copy and lets you paste any past item back in
two keystrokes. It aims for *CopyQ's capability with GPaste's native feel*, without the Wayland
fragility or GNOME-upgrade breakage of shell-extension managers.

> Status: **early development** (M0 scaffold). See [`SPEC.md`](SPEC.md) for the full MVP spec
> and milestone plan.

## Architecture

A Cargo workspace of three crates:

| Crate          | Role                                                                 |
| -------------- | ------------------------------------------------------------------- |
| `copycat-core` | Data model + SQLite-backed history store (no UI/platform deps)      |
| `copycatd`     | Background daemon that captures clipboard changes                    |
| `copycat`      | GTK4 / libadwaita app: the search popup, preferences, tray icon      |

- **Language:** Rust
- **UI:** GTK4 + libadwaita
- **Clipboard:** Wayland `wlr-data-control` (X11/XFixes fallback)
- **Storage:** SQLite (bundled)
- **Distribution:** `.deb` + PPA for the MVP

## Building from source

### Prerequisites

- Rust (stable) — install via [rustup](https://rustup.rs)
- System development libraries:

```bash
sudo apt-get update
sudo apt-get install -y libgtk-4-dev libadwaita-1-dev libwayland-dev
```

### Build & test

```bash
cargo build
cargo test
```

To work on just the storage layer (no system libraries required):

```bash
cargo test -p copycat-core
```

## Running it

copycat is two processes: a **daemon** that records what you copy, and a **popup**
that lets you search and restore it. **Nothing is captured unless the daemon is
running.**

Run the daemon in a terminal:

```bash
cargo run -p copycatd
```

Then open the popup (in another terminal, or bound to a hotkey):

```bash
cargo run -p copycat
```

## Wiring it into your desktop

To stop running it by hand — daemon on login, popup on `Super+V`:

```bash
cargo build --release --workspace
scripts/install-desktop.sh
```

That script links the binaries into `~/.local/bin`, enables the `copycat-daemon`
user service, and binds `Super+V` to the popup. It prefers a release build and
falls back to a debug one, and is safe to re-run.

**One shortcut changes:** GNOME ships `Super+V` as "toggle message tray", which
also has `Super+M`. The script takes `Super+V` and leaves `Super+M` doing the
tray.

To reverse everything (shortcut, service, symlinks — the history database is
left alone):

```bash
scripts/install-desktop.sh --uninstall
```

Check the daemon any time with `systemctl --user status copycat-daemon`.

### Using it

1. Copy things normally — the daemon records them.
2. Press `Super+V` to open the popup.
3. Type to fuzzy-search, or use `Up`/`Down`.
4. `Enter` copies the clip and closes the popup; `Esc` dismisses it.
5. The clip is pasted into the window you came from. (In terminals use `Ctrl+Shift+V`,
   since `Ctrl+V` is not paste there.)

## License

GPL-3.0-or-later.
