#!/bin/sh
# Hermes server installer for systemd Linux distributions (PLAN.md section 18.4).
#
#   curl -fsSL https://raw.githubusercontent.com/senko-tools/hermes/main/deploy/install.sh | sudo sh
#
# Installs or upgrades /usr/local/bin/hermes-server and the hermes.service unit, creates the
# `hermes` system user, /etc/hermes/hermes.toml (only if missing) and /var/lib/hermes.
# Re-running it upgrades the binary and keeps the config and the data.
#
# Options (as arguments, or as environment variables when piping into sh):
#   --version vX.Y.Z   HERMES_VERSION   release to install (default: latest)
#   --from DIR         HERMES_FROM      install from release files already downloaded into DIR
#                                       (hermes-server-<arch>-unknown-linux-musl.tar.gz,
#                                       hermes.service, SHA256SUMS) instead of downloading them
set -eu

REPO="senko-tools/hermes"
BIN=/usr/local/bin/hermes-server
CONF_DIR=/etc/hermes
CONF="$CONF_DIR/hermes.toml"
DATA_DIR=/var/lib/hermes
UNIT=/etc/systemd/system/hermes.service

VERSION="${HERMES_VERSION:-latest}"
FROM="${HERMES_FROM:-}"

die() {
    echo "install.sh: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift 2 ;;
        --from) [ $# -ge 2 ] || die "--from needs a directory"; FROM="$2"; shift 2 ;;
        -h|--help) sed -n '2,16p' "$0" 2>/dev/null || true; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

[ "$(id -u)" -eq 0 ] || die "run this as root (e.g. with sudo)"
command -v systemctl >/dev/null 2>&1 || die "systemd is required"
for tool in tar sha256sum install; do
    command -v "$tool" >/dev/null 2>&1 || die "'$tool' is required"
done

case "$(uname -m)" in
    x86_64|amd64) ARCH=x86_64 ;;
    aarch64|arm64) ARCH=aarch64 ;;
    *) die "unsupported architecture: $(uname -m) (x86_64 and aarch64 are supported)" ;;
esac
ASSET="hermes-server-$ARCH-unknown-linux-musl.tar.gz"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

# --- fetch the release files -------------------------------------------------------------------
if [ -n "$FROM" ]; then
    for file in "$ASSET" hermes.service SHA256SUMS; do
        [ -f "$FROM/$file" ] || die "$FROM/$file is missing"
        cp "$FROM/$file" "$TMP/$file"
    done
    echo "Installing from $FROM"
else
    command -v curl >/dev/null 2>&1 || die "'curl' is required"
    if [ "$VERSION" = "latest" ]; then
        URL="https://github.com/$REPO/releases/latest/download"
    else
        URL="https://github.com/$REPO/releases/download/$VERSION"
    fi
    echo "Downloading $ASSET ($VERSION)"
    for file in "$ASSET" hermes.service SHA256SUMS; do
        curl -fsSL --retry 3 -o "$TMP/$file" "$URL/$file" || die "could not download $URL/$file"
    done
fi

(
    cd "$TMP"
    for file in "$ASSET" hermes.service; do
        grep -E "^[0-9a-f]{64}  \\*?$file\$" SHA256SUMS > "$file.sum" || die "no checksum for $file"
        sha256sum -c --quiet "$file.sum" || die "checksum mismatch for $file"
    done
)
tar -xzf "$TMP/$ASSET" -C "$TMP" hermes-server || die "$ASSET has no hermes-server"

# --- user, directories, binary -----------------------------------------------------------------
if ! id hermes >/dev/null 2>&1; then
    useradd --system --user-group --home-dir "$DATA_DIR" --no-create-home \
        --shell /usr/sbin/nologin --comment "Hermes chat server" hermes
    echo "Created the system user 'hermes'"
fi
install -d -o hermes -g hermes -m 750 "$DATA_DIR"
install -d -o root -g hermes -m 750 "$CONF_DIR"

# Replace the binary atomically, so a running server keeps its old file until restarted.
install -o root -g root -m 755 "$TMP/hermes-server" "$BIN.new"
mv -f "$BIN.new" "$BIN"
echo "Installed $("$BIN" --version)"

if [ ! -f "$CONF" ]; then
    "$BIN" init --config "$CONF" >/dev/null
    sed -i "s|^data_dir = \"./data\"|data_dir = \"$DATA_DIR\"|" "$CONF"
    chown root:hermes "$CONF"
    chmod 640 "$CONF"
    echo "Wrote $CONF"
    NEW_CONFIG=1
else
    echo "Kept the existing $CONF"
    NEW_CONFIG=0
fi

install -o root -g root -m 644 "$TMP/hermes.service" "$UNIT"
systemctl daemon-reload

# --- next steps --------------------------------------------------------------------------------
echo
if [ "$NEW_CONFIG" = 1 ]; then
    cat <<EOF
Next steps:
  1. Edit $CONF: set instance.name and instance.public_host.
  2. Check it:       hermes-server check-config --config $CONF
  3. Start Hermes:   systemctl enable --now hermes
  4. Setup link:     journalctl -u hermes -n 50   (open the invite link to create the owner account)
EOF
elif systemctl is-active --quiet hermes; then
    echo "Upgraded. Restart to use the new version:  systemctl restart hermes"
else
    echo "Start Hermes with:  systemctl enable --now hermes"
fi
PORT="$(sed -n 's/^port *= *\([0-9]*\).*/\1/p' "$CONF" | head -n 1)"
PORT="${PORT:-7700}"
cat <<EOF

Open port $PORT for both TCP and UDP in your firewall, for example:
  ufw allow $PORT/tcp && ufw allow $PORT/udp
EOF
