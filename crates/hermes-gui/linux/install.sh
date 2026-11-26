#!/bin/sh
# Installs Hermes for the current user (no root needed), from the folder this script is in:
#
#   ./install.sh               install or update
#   ./install.sh --uninstall   remove the program, its menu entry and its icons (settings, logs and
#                              sign-ins in ~/.config/hermes stay)
#
# The program goes to ~/.local/share/hermes, the menu entry to ~/.local/share/applications (it also
# opens hermes:// links), the icons to ~/.local/share/icons. Set HERMES_PREFIX to use another folder
# than ~/.local/share.
set -eu

APP_ID=tools.senko.Hermes
here="$(cd "$(dirname "$0")" && pwd)"
data="${HERMES_PREFIX:-${XDG_DATA_HOME:-$HOME/.local/share}}"
app_dir="$data/hermes"
desktop_file="$data/applications/$APP_ID.desktop"
icons="$data/icons/hicolor"
config="${XDG_CONFIG_HOME:-$HOME/.config}"

refresh() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$data/applications" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$icons" 2>/dev/null || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -rf "$app_dir"
    rm -f "$desktop_file" "$config/autostart/$APP_ID.desktop"
    for f in "$icons"/*/apps/"$APP_ID".png; do rm -f "$f"; done
    refresh
    echo "Hermes is uninstalled. Your settings and sign-ins are still in $config/hermes."
    exit 0
fi

[ -f "$here/hermes" ] || { echo "hermes is not next to this script" >&2; exit 1; }

# The program. A running copy keeps working: the new file replaces the old one's name only.
mkdir -p "$app_dir"
cp "$here/hermes" "$app_dir/hermes.new"
chmod 755 "$app_dir/hermes.new"
mv -f "$app_dir/hermes.new" "$app_dir/hermes"
[ -f "$here/LICENSE-GPL-3.0" ] && cp "$here/LICENSE-GPL-3.0" "$app_dir/"

# The icons.
for png in "$here"/icons/icon-*.png; do
    size="${png##*/icon-}"
    size="${size%.png}"
    mkdir -p "$icons/${size}x${size}/apps"
    cp "$png" "$icons/${size}x${size}/apps/$APP_ID.png"
done

# The menu entry. Exec quoting (desktop entry spec): the path in double quotes, with ", `, $ and \
# escaped by a backslash that is itself written twice.
exec_path="$(printf '%s' "$app_dir/hermes" | sed -e 's/\\/\\\\\\\\/g' -e 's/["`$]/\\\\&/g' -e 's/%/%%/g')"
mkdir -p "$data/applications"
cat > "$desktop_file" <<EOF
[Desktop Entry]
Type=Application
Name=Hermes
GenericName=Chat
Comment=Chat, voice and screen sharing on your own server
Exec="$exec_path" %u
Icon=$APP_ID
Terminal=false
Categories=Network;Chat;InstantMessaging;
MimeType=x-scheme-handler/hermes;
StartupWMClass=$APP_ID
StartupNotify=true
EOF

# hermes:// links open Hermes.
command -v xdg-mime >/dev/null 2>&1 && xdg-mime default "$APP_ID.desktop" x-scheme-handler/hermes || true

# Starting at sign-in (when it is on) runs the program from its new place.
if [ -f "$config/autostart/$APP_ID.desktop" ]; then
    sed -i "s|^Exec=.*|Exec=\"$exec_path\"|" "$config/autostart/$APP_ID.desktop"
fi
refresh

echo "Hermes is installed in $app_dir and is in your app menu."

# What Hermes needs from the system, said once here rather than found out later.
# Libraries the system lacks stop Hermes from starting at all (libpipewire-0.3 on systems
# without PipeWire, for example).
if command -v ldd >/dev/null 2>&1; then
    libs=$(ldd "$app_dir/hermes" 2>/dev/null | awk '/not found/ {print $1}' | tr '\n' ' ')
    if [ -n "$libs" ]; then
        echo
        echo "Hermes cannot start yet: the system lacks $libs"
        echo "  Install the packages that provide them (for libpipewire-0.3.so.0: libpipewire-0.3-0"
        echo "  on Debian and Ubuntu, pipewire-libs on Fedora, libpipewire on Arch)."
    fi
fi
missing=""
command -v ffmpeg >/dev/null 2>&1 || missing="$missing ffmpeg"
if [ -n "$missing" ]; then
    echo
    echo "Also install these for everything to work:$missing"
    echo "  ffmpeg plays videos and sound files in the chat."
fi
