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

## License

GPL-3.0-or-later.
