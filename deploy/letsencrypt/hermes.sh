#!/bin/sh
# certbot deploy hook (PLAN.md section 18.8 step 4): copies the certificate where the `hermes`
# user can read it and makes the server reload it. Install as
# /etc/letsencrypt/renewal-hooks/deploy/hermes.sh (mode 755) and run it once by hand after
# install.sh has created the `hermes` user. Set DOMAIN for a domain other than hermes.senko.tools.
set -e
DOMAIN="${DOMAIN:-hermes.senko.tools}"
LIVE="/etc/letsencrypt/live/$DOMAIN"

# certbot runs every deploy hook for every renewed certificate; only react to ours.
if [ -n "${RENEWED_LINEAGE:-}" ] && [ "$RENEWED_LINEAGE" != "$LIVE" ]; then
    exit 0
fi

install -d -o root -g hermes -m 750 /etc/hermes/tls
install -o root -g hermes -m 640 "$LIVE/fullchain.pem" /etc/hermes/tls/fullchain.pem
install -o root -g hermes -m 640 "$LIVE/privkey.pem" /etc/hermes/tls/privkey.pem
systemctl reload hermes 2>/dev/null || true
