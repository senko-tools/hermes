#!/bin/sh
# Hermes server installer for systemd Linux distributions (PLAN.md section 18.4).
#
#   curl -fsSL https://raw.githubusercontent.com/senko-tools/hermes/main/deploy/install.sh | sudo sh
#
# or, with the files of a release downloaded into one folder (needed while the repository is
# private, because the server cannot download from GitHub then):
#
#   sudo sh ./install.sh          # uses the release files next to it
#
# Does everything: downloads the newest release (pre-releases included), installs
# /usr/local/bin/hermes-server and hermes.service, creates the `hermes` user, /etc/hermes and
# /var/lib/hermes, writes the config, gets a Let's Encrypt certificate for a domain (or uses a
# self-signed one), installs the certificate renewal hook and a nightly backup, opens the port in
# ufw, checks the config, starts the server and prints the owner invite link on the first start.
#
# A first install asks for the domain (and an email for Let's Encrypt) when run in a terminal.
# Re-running it upgrades the binary, keeps the config and the data, and repairs the service,
# hooks and file permissions.
#
# Options (as arguments; with `curl … | sudo sh` pass them after `sh -s --`):
#   --domain NAME      domain or public IP clients use (instance.public_host)
#   --email ADDRESS    email for Let's Encrypt expiry notices (optional)
#   --name NAME        instance name, also the first space's name (default: Hermes)
#   --port N           TCP + UDP port (default: 7700)
#   --self-signed      do not use Let's Encrypt, even for a domain
#   --reconfigure      apply --domain/--name/--port/TLS to an existing config too
#   --version vX.Y.Z   release to install (default: the newest, pre-releases included)
#   --from DIR         install from release files already in DIR (default: the script's folder
#                      when the files are there): hermes-server-<arch>-unknown-linux-musl.tar.gz,
#                      hermes.service, SHA256SUMS
#   --yes              never ask; use the defaults for anything not given
#   --no-start         install and configure, but do not (re)start the server
set -eu

REPO="senko-tools/hermes"
BIN=/usr/local/bin/hermes-server
CONF_DIR=/etc/hermes
CONF="$CONF_DIR/hermes.toml"
TLS_DIR="$CONF_DIR/tls"
DATA_DIR=/var/lib/hermes
UNIT=/etc/systemd/system/hermes.service
HOOK=/etc/letsencrypt/renewal-hooks/deploy/hermes.sh
BACKUP=/etc/cron.daily/hermes-backup

VERSION="${HERMES_VERSION:-latest}"
FROM="${HERMES_FROM:-}"
DOMAIN=""
EMAIL=""
NAME=""
PORT=""
SELF_SIGNED=0
RECONFIGURE=0
ASSUME_YES=0
START=1

die() {
    echo "install.sh: $*" >&2
    exit 1
}
say() { printf '\n==> %s\n' "$*"; }
warn() { printf 'WARNING: %s\n' "$*" >&2; }

while [ $# -gt 0 ]; do
    case "$1" in
        --domain) [ $# -ge 2 ] || die "--domain needs a value"; DOMAIN="$2"; shift 2 ;;
        --email) [ $# -ge 2 ] || die "--email needs a value"; EMAIL="$2"; shift 2 ;;
        --name) [ $# -ge 2 ] || die "--name needs a value"; NAME="$2"; shift 2 ;;
        --port) [ $# -ge 2 ] || die "--port needs a value"; PORT="$2"; shift 2 ;;
        --self-signed) SELF_SIGNED=1; shift ;;
        --reconfigure) RECONFIGURE=1; shift ;;
        --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift 2 ;;
        --from) [ $# -ge 2 ] || die "--from needs a directory"; FROM="$2"; shift 2 ;;
        --yes|-y) ASSUME_YES=1; shift ;;
        --no-start) START=0; shift ;;
        -h|--help) sed -n '2,37p' "$0" 2>/dev/null || true; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
done
case "$PORT" in
    ''|*[!0-9]*) [ -z "$PORT" ] || die "--port must be a number" ;;
esac

[ "$(id -u)" -eq 0 ] || die "run this as root (e.g. with sudo)"
command -v systemctl >/dev/null 2>&1 || die "systemd is required"

case "$(uname -m)" in
    x86_64|amd64) ARCH=x86_64 ;;
    aarch64|arm64) ARCH=aarch64 ;;
    *) die "unsupported architecture: $(uname -m) (x86_64 and aarch64 are supported)" ;;
esac
ASSET="hermes-server-$ARCH-unknown-linux-musl.tar.gz"

# Run from a folder with the release files (a downloaded release): install those.
if [ -z "$FROM" ] && [ -f "$0" ] && [ -f "$(dirname "$0")/$ASSET" ]; then
    FROM="$(dirname "$0")"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

# --- helpers -----------------------------------------------------------------------------------

# Questions go to the terminal even when the script itself comes from a pipe.
can_ask() {
    [ "$ASSUME_YES" = 0 ] && [ -t 1 ] && (: </dev/tty) 2>/dev/null
}

# ask "Question" "default" → the answer (or the default)
ask() {
    printf '%s' "$1" >/dev/tty
    [ -n "$2" ] && printf ' [%s]' "$2" >/dev/tty
    printf ': ' >/dev/tty
    read -r answer </dev/tty || answer=""
    [ -n "$answer" ] || answer="$2"
    printf '%s' "$answer"
}

apt_install() {
    command -v apt-get >/dev/null 2>&1 || return 1
    DEBIAN_FRONTEND=noninteractive apt-get install -y -q "$@" >/dev/null 2>&1 && return 0
    DEBIAN_FRONTEND=noninteractive apt-get update -q >/dev/null 2>&1 || true
    DEBIAN_FRONTEND=noninteractive apt-get install -y -q "$@" >/dev/null 2>&1
}

# The Debian/Ubuntu package with a tool.
package_of() {
    case "$1" in
        ss) echo iproute2 ;;
        sha256sum|install) echo coreutils ;;
        *) echo "$1" ;;
    esac
}

need_tools() {
    missing=""
    for tool in "$@"; do
        command -v "$tool" >/dev/null 2>&1 || missing="$missing $(package_of "$tool")"
    done
    [ -z "$missing" ] && return 0
    echo "Installing:$missing"
    # shellcheck disable=SC2086
    apt_install $missing ca-certificates || die "please install:$missing"
}

# A TOML string.
toml_str() {
    printf '"%s"' "$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g')"
}

# set_key section key value: replaces `key = …` inside `[section]` of the config.
set_key() {
    value="$(printf '%s' "$3" | sed 's/[\\&|]/\\&/g')"
    sed -i "/^\[$1\]/,/^\[/ s|^$2 *=.*|$2 = $value|" "$CONF"
    grep -q "^$2 = " "$CONF" || warn "could not set $1.$2 in $CONF; please set it by hand"
}

# get_key section key: the raw value (quotes stripped).
get_key() {
    sed -n "/^\[$1\]/,/^\[/ s|^$2 *= *\"\{0,1\}\([^\"#]*\)\"\{0,1\}.*|\1|p" "$CONF" | head -n 1 \
        | sed 's/ *$//'
}

is_ip() {
    case "$1" in
        *:*) return 0 ;;
        *[!0-9.]*) return 1 ;;
        *) return 0 ;;
    esac
}

port_80_user() {
    ss -ltnpH 'sport = :80' 2>/dev/null | sed -n 's/.*users:(("\([^"]*\)".*/\1/p' | head -n 1
}

# --- 1. the release files ----------------------------------------------------------------------
need_tools curl tar sha256sum install sed ss

if [ -n "$FROM" ]; then
    for file in "$ASSET" hermes.service SHA256SUMS; do
        [ -f "$FROM/$file" ] || die "$FROM/$file is missing"
        cp "$FROM/$file" "$TMP/$file"
    done
    say "Installing from $FROM"
else
    if [ "$VERSION" = "latest" ]; then
        # The newest release, pre-releases included (GitHub's /releases/latest skips those).
        VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=1" \
            | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)" || true
        [ -n "$VERSION" ] || die "could not find the newest release of $REPO. While the repository is private, download a release's files into one folder, copy it here and run install.sh from that folder."
    fi
    URL="https://github.com/$REPO/releases/download/$VERSION"
    say "Downloading Hermes $VERSION"
    for file in "$ASSET" hermes.service SHA256SUMS; do
        curl -fsSL --retry 3 -o "$TMP/$file" "$URL/$file"             || die "could not download $URL/$file (private repository? see --help)"
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

# --- 2. user, directories, binary, service -----------------------------------------------------
say "Installing the server"
if ! id hermes >/dev/null 2>&1; then
    useradd --system --user-group --home-dir "$DATA_DIR" --no-create-home \
        --shell /usr/sbin/nologin --comment "Hermes chat server" hermes
    echo "Created the system user 'hermes'"
fi
install -d -o hermes -g hermes -m 750 "$DATA_DIR"
install -d -o root -g hermes -m 750 "$CONF_DIR"
# Data written by an earlier run as root, or copied by hand, must belong to `hermes`.
chown -R hermes:hermes "$DATA_DIR"

# Replace the binary atomically, so a running server keeps its old file until restarted.
install -o root -g root -m 755 "$TMP/hermes-server" "$BIN.new"
mv -f "$BIN.new" "$BIN"
echo "Installed $("$BIN" --version)"

install -o root -g root -m 644 "$TMP/hermes.service" "$UNIT"
systemctl daemon-reload

# --- 3. configuration --------------------------------------------------------------------------
FRESH=0
[ -f "$CONF" ] || FRESH=1

if [ "$FRESH" = 1 ] || [ "$RECONFIGURE" = 1 ]; then
    say "Configuring"
    if [ -z "$DOMAIN" ] && [ "$FRESH" = 0 ]; then
        DOMAIN="$(get_key instance public_host)"
    fi
    if [ -z "$DOMAIN" ]; then
        guess="$(hostname -I 2>/dev/null | awk '{print $1}')"
        if can_ask; then
            echo "Clients connect to a domain (recommended, e.g. chat.example.com, with its DNS"
            echo "A/AAAA records pointing at this server) or to this server's public IP."
            DOMAIN="$(ask "Domain or public IP of this server" "$guess")"
        else
            DOMAIN="$guess"
            warn "no --domain given; using $DOMAIN (self-signed certificate)"
        fi
    fi
    [ -n "$DOMAIN" ] || die "no domain or IP given (use --domain)"
    if [ -z "$NAME" ] && [ "$FRESH" = 1 ] && can_ask; then
        NAME="$(ask "Name of this server (also the first space's name)" "Hermes")"
    fi
    if ! is_ip "$DOMAIN" && [ "$SELF_SIGNED" = 0 ] && [ -z "$EMAIL" ] \
        && [ ! -f "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" ] && can_ask; then
        EMAIL="$(ask "Email for Let's Encrypt expiry notices (empty: none)" "")"
    fi

    if [ "$FRESH" = 1 ]; then
        "$BIN" init --config "$CONF" >/dev/null
    fi
    set_key instance public_host "$(toml_str "$DOMAIN")"
    [ -z "$NAME" ] || set_key instance name "$(toml_str "$NAME")"
    [ -z "$PORT" ] || set_key network port "$PORT"
    set_key storage data_dir "$(toml_str "$DATA_DIR")"
    if [ "$FRESH" = 1 ]; then
        # Dual stack when this server has a public IPv6 address.
        if ip -6 addr show scope global 2>/dev/null | grep -q inet6; then
            set_key network bind '"::"'
        fi
    fi
    WANT_TLS=1
else
    say "Keeping the existing $CONF (use --reconfigure to change it)"
    DOMAIN="$(get_key instance public_host)"
    WANT_TLS=0
    # An existing Let's Encrypt setup is refreshed (hook, certificate copy) on every run.
    [ "$(get_key tls mode)" = "files" ] && [ -f "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" ] \
        && WANT_TLS=1
fi
chown root:hermes "$CONF"
chmod 640 "$CONF"
PORT="$(get_key network port)"
PORT="${PORT:-7700}"

# --- 4. certificate ----------------------------------------------------------------------------
write_hook() {
    install -d -m 755 "$(dirname "$HOOK")"
    cat > "$HOOK" <<EOF
#!/bin/sh
# certbot deploy hook written by Hermes' install.sh: copies the certificate for $DOMAIN where
# the hermes user can read it and makes the server reload it.
set -e
LIVE="/etc/letsencrypt/live/$DOMAIN"
# certbot runs every deploy hook for every renewed certificate; only react to ours.
if [ -n "\${RENEWED_LINEAGE:-}" ] && [ "\$RENEWED_LINEAGE" != "\$LIVE" ]; then
    exit 0
fi
install -d -o root -g hermes -m 750 $TLS_DIR
install -o root -g hermes -m 640 "\$LIVE/fullchain.pem" $TLS_DIR/fullchain.pem
install -o root -g hermes -m 640 "\$LIVE/privkey.pem" $TLS_DIR/privkey.pem
systemctl reload hermes 2>/dev/null || true
EOF
    chmod 755 "$HOOK"
}

get_certificate() {
    live="/etc/letsencrypt/live/$DOMAIN"
    if [ -f "$live/fullchain.pem" ]; then
        echo "Using the existing Let's Encrypt certificate for $DOMAIN"
        return 0
    fi
    need_tools certbot
    if [ -n "$EMAIL" ]; then
        account="-m $EMAIL"
    else
        account="--register-unsafely-without-email"
    fi
    user80="$(port_80_user)"
    case "$user80" in
        "")
            echo "Getting a certificate for $DOMAIN (certbot, standalone on port 80)"
            method="--standalone" ;;
        nginx)
            echo "Getting a certificate for $DOMAIN (certbot, through nginx)"
            dpkg -s python3-certbot-nginx >/dev/null 2>&1 || apt_install python3-certbot-nginx \
                || { warn "the certbot nginx plugin is missing"; return 1; }
            method="--nginx" ;;
        apache2|httpd)
            echo "Getting a certificate for $DOMAIN (certbot, through Apache)"
            dpkg -s python3-certbot-apache >/dev/null 2>&1 || apt_install python3-certbot-apache \
                || { warn "the certbot apache plugin is missing"; return 1; }
            method="--apache" ;;
        *)
            warn "port 80 is used by '$user80', so certbot cannot answer the challenge"
            return 1 ;;
    esac
    # shellcheck disable=SC2086
    certbot certonly $method -d "$DOMAIN" --agree-tos --non-interactive $account
}

if [ "$WANT_TLS" = 1 ]; then
    if ! is_ip "$DOMAIN" && [ "$SELF_SIGNED" = 0 ]; then
        say "Certificate"
        if get_certificate; then
            write_hook
            RENEWED_LINEAGE="" "$HOOK"
            set_key tls mode '"files"'
            set_key tls cert_file "$(toml_str "$TLS_DIR/fullchain.pem")"
            set_key tls key_file "$(toml_str "$TLS_DIR/privkey.pem")"
            echo "TLS: Let's Encrypt (renewed automatically by certbot's timer)"
        else
            warn "no Let's Encrypt certificate; using a self-signed one for now."
            warn "Check that $DOMAIN points at this server and TCP port 80 is open, then"
            warn "re-run this script with --reconfigure."
            set_key tls mode '"self_signed"'
        fi
    else
        set_key tls mode '"self_signed"'
        echo "TLS: self-signed (clients confirm its fingerprint once)"
    fi
fi

# --- 5. nightly backup -------------------------------------------------------------------------
cat > "$BACKUP" <<'EOF'
#!/bin/sh
# Nightly Hermes backup written by install.sh. Keeps 14 days in /var/backups/hermes.
set -e
d=/var/backups/hermes
install -d -o hermes -g hermes -m 750 "$d"
runuser -u hermes -- /usr/local/bin/hermes-server backup --config /etc/hermes/hermes.toml "$d/hermes-$(date +%F).db"
if [ -d /var/lib/hermes/files ]; then
    tar -C /var/lib/hermes -czf "$d/files-$(date +%F).tar.gz" files
fi
find "$d" -type f -mtime +14 -delete
EOF
chmod 755 "$BACKUP"

# --- 6. firewall -------------------------------------------------------------------------------
if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q "Status: active"; then
    ufw allow "$PORT/tcp" >/dev/null
    ufw allow "$PORT/udp" >/dev/null
    echo "ufw: allowed $PORT/tcp and $PORT/udp"
fi

# --- 7. check and start ------------------------------------------------------------------------
say "Checking the configuration"
runuser -u hermes -- "$BIN" check-config --config "$CONF" >"$TMP/check" 2>&1 || {
    cat "$TMP/check" >&2
    die "the configuration has errors (see above); fix $CONF and re-run"
}
echo "OK"

if [ "$START" = 0 ]; then
    say "Installed. Start Hermes with:  systemctl enable --now hermes"
    exit 0
fi

say "Starting Hermes"
since="$(date '+%Y-%m-%d %H:%M:%S')"
systemctl enable hermes >/dev/null 2>&1
systemctl restart hermes
ready=0
i=0
while [ $i -lt 30 ]; do
    if journalctl -u hermes --since "$since" -o cat --no-pager 2>/dev/null | grep -q "Hermes is ready"; then
        ready=1
        break
    fi
    sleep 1
    i=$((i + 1))
done
if [ "$ready" = 0 ]; then
    journalctl -u hermes --since "$since" -n 30 -o cat --no-pager >&2 || true
    die "Hermes did not start (log above; more with: journalctl -u hermes -n 100)"
fi
"$BIN" --version
link="$(journalctl -u hermes --since "$since" -o cat --no-pager | grep -o 'https://[^ ]*/invite/[^ ]*' | tail -n 1)"

cat <<EOF

Hermes is running: https://$DOMAIN:$PORT
EOF
if [ -n "$link" ]; then
    cat <<EOF

  FIRST-TIME SETUP: open this link in the Hermes client (paste it into "Add instance")
  to create the OWNER account. Single use, valid for 24 hours:

    $link
EOF
fi
cat <<EOF

Also open TCP and UDP port $PORT in your hosting provider's firewall, if it has one
(e.g. the Hetzner Cloud Firewall). Voice needs the UDP port.

  Logs:        journalctl -u hermes -f
  Config:      $CONF (then: systemctl restart hermes)
  New invite:  runuser -u hermes -- $BIN invite create --config $CONF
  Upgrade:     re-run this script
EOF
