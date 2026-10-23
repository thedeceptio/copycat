#!/usr/bin/env bash
#
# Wire copycat into the desktop session:
#   * link the built binaries into ~/.local/bin
#   * run copycatd automatically via a systemd user service
#   * bind Super+V to the popup
#
# Everything here is idempotent, and `--uninstall` reverses all of it.
#
# Usage:
#   scripts/install-desktop.sh [--uninstall]

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="$HOME/.local/bin"
UNIT_DIR="$HOME/.config/systemd/user"
UNIT_NAME="copycat-daemon.service"

# GNOME shortcut wiring.
MEDIA_KEYS="org.gnome.settings-daemon.plugins.media-keys"
CUSTOM_PATH="/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/copycat/"
CUSTOM_SCHEMA="${MEDIA_KEYS}.custom-keybinding:${CUSTOM_PATH}"
SHORTCUT="<Super>v"
# Super+V is GNOME's "toggle message tray" out of the box. It also ships
# Super+M for the same action, so we take V and leave M alone.
TRAY_KEY="org.gnome.shell.keybindings toggle-message-tray"
TRAY_WITHOUT_V="['<Super>m']"
TRAY_DEFAULT="['<Super>v', '<Super>m']"

say() { printf '  %s\n' "$*"; }

# Add/remove a path in a GVariant string-array, printing the new value.
# Parsing the list properly beats string surgery on quotes and brackets.
#   gvariant_list <add|remove> <current value> <path>
gvariant_list() {
    python3 -I -c '
import ast, sys
action, current, path = sys.argv[1], sys.argv[2], sys.argv[3]
text = current.strip()
if text.startswith("@as"):
    text = text[len("@as"):].strip()
try:
    items = list(ast.literal_eval(text)) if text and text != "[]" else []
except (ValueError, SyntaxError):
    items = []
if action == "add":
    if path not in items:
        items.append(path)
else:
    items = [i for i in items if i != path]
print("@as []" if not items else "[" + ", ".join("%r" % i for i in items) + "]")
' "$@"
}

# Which profile did cargo build into? Prefer release, fall back to debug.
detect_profile() {
    if [[ -x "$REPO_ROOT/target/release/copycat" && -x "$REPO_ROOT/target/release/copycatd" ]]; then
        echo "release"
    else
        echo "debug"
    fi
}

install_all() {
    local profile target
    profile="$(detect_profile)"
    target="$REPO_ROOT/target/$profile"

    if [[ ! -x "$target/copycat" || ! -x "$target/copycatd" ]]; then
        echo "error: binaries not found in $target" >&2
        echo "build them first:  cargo build --workspace" >&2
        exit 1
    fi

    echo "Linking binaries ($profile build)"
    mkdir -p "$BIN_DIR"
    ln -sfn "$target/copycat" "$BIN_DIR/copycat"
    ln -sfn "$target/copycatd" "$BIN_DIR/copycatd"
    say "$BIN_DIR/copycat -> $target/copycat"
    say "$BIN_DIR/copycatd -> $target/copycatd"

    echo "Installing the capture daemon service"
    mkdir -p "$UNIT_DIR"
    install -m 0644 "$REPO_ROOT/packaging/systemd/$UNIT_NAME" "$UNIT_DIR/$UNIT_NAME"
    # The daemon needs the display to watch the clipboard; make sure the user
    # manager knows about it before starting the service.
    systemctl --user import-environment DISPLAY XAUTHORITY WAYLAND_DISPLAY 2>/dev/null || true
    systemctl --user daemon-reload
    systemctl --user enable --now "$UNIT_NAME"
    say "enabled $UNIT_NAME (starts on login)"

    echo "Binding $SHORTCUT to the popup"
    # Free Super+V from the message tray, keeping Super+M for it.
    gsettings set $TRAY_KEY "$TRAY_WITHOUT_V"
    say "message tray now on Super+M only"

    # Register (or update) our custom keybinding, without clobbering others.
    local existing updated
    existing="$(gsettings get "$MEDIA_KEYS" custom-keybindings)"
    updated="$(gvariant_list add "$existing" "$CUSTOM_PATH")"
    gsettings set "$MEDIA_KEYS" custom-keybindings "$updated"
    gsettings set "$CUSTOM_SCHEMA" name 'copycat'
    gsettings set "$CUSTOM_SCHEMA" command "$BIN_DIR/copycat"
    gsettings set "$CUSTOM_SCHEMA" binding "$SHORTCUT"
    say "$SHORTCUT -> $BIN_DIR/copycat"

    echo
    echo "Done. Press $SHORTCUT to open copycat."
    echo "Check the daemon with:  systemctl --user status $UNIT_NAME"
}

uninstall_all() {
    echo "Removing the $SHORTCUT binding"
    local existing cleaned
    existing="$(gsettings get "$MEDIA_KEYS" custom-keybindings 2>/dev/null || echo '@as []')"
    cleaned="$(gvariant_list remove "$existing" "$CUSTOM_PATH")"
    gsettings set "$MEDIA_KEYS" custom-keybindings "$cleaned"
    gsettings reset-recursively "$CUSTOM_SCHEMA" 2>/dev/null || true
    say "restoring the message tray shortcut"
    gsettings set $TRAY_KEY "$TRAY_DEFAULT"

    echo "Disabling the capture daemon service"
    systemctl --user disable --now "$UNIT_NAME" 2>/dev/null || true
    rm -f "$UNIT_DIR/$UNIT_NAME"
    systemctl --user daemon-reload 2>/dev/null || true

    echo "Unlinking binaries"
    [[ -L "$BIN_DIR/copycat" ]] && rm -f "$BIN_DIR/copycat"
    [[ -L "$BIN_DIR/copycatd" ]] && rm -f "$BIN_DIR/copycatd"

    echo
    echo "Done. copycat is unwired; your history database was left untouched."
}

case "${1:-}" in
    --uninstall) uninstall_all ;;
    "") install_all ;;
    *)
        echo "usage: $(basename "$0") [--uninstall]" >&2
        exit 2
        ;;
esac
