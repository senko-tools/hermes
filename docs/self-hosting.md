# Hosting your own Hermes server

Installing the server is explained step by step in the
[README](../README.md#hosting-a-server): an install script, Docker, a game server panel, or by
hand. This page covers what comes after.

- [Optional: behind Caddy or nginx](#optional-behind-caddy-or-nginx)
- [Inviting people](#inviting-people)
- [Day-to-day administration](#day-to-day-administration)
- [Backups and upgrades](#backups-and-upgrades)
- [Restoring a backup](#restoring-a-backup)
- [Settings worth knowing](#settings-worth-knowing)
- [Troubleshooting](#troubleshooting)
- [Limiting disk use](#limiting-disk-use)

---

## Optional: behind Caddy or nginx

Only for people who already run a web server with a certificate for the domain. Hermes then
speaks plain HTTP on localhost and the web server does TLS. **Voice still needs UDP 7700 open
directly to Hermes**: web servers cannot forward it.

In `/etc/hermes/hermes.toml`:

```toml
[instance]
public_host = "chat.example.com"
public_port = 443          # what clients connect to (the web server)

[network]
bind = "127.0.0.1"
port = 7700
trusted_proxies = ["127.0.0.1", "::1"]   # so rate limits see the real client address

[tls]
mode = "off"
```

**Caddy** (`/etc/caddy/Caddyfile`):

```
chat.example.com {
    reverse_proxy 127.0.0.1:7700
}
```

**nginx** (inside the `server { listen 443 ssl; … }` block of the domain):

```nginx
client_max_body_size 30m;            # at least storage.max_upload_mb

location / {
    proxy_pass http://127.0.0.1:7700;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto https;
    # The real-time connection (WebSocket) at /api/v1/gateway:
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_read_timeout 1h;
}
```

The voice part of the app uses the same port number over UDP (`network.port`, 7700 here), so
open **UDP 7700** in the firewall.

## Inviting people

- In the app: right-click the space → **Invite people (copy link)**, or open **Space settings →
  Invites** to choose how long the link works and how many people can use it.
- From the server: `hermes-server invite create --space <space id> --uses 5 --expires 7d`
  (the space ID is in the space's right-click menu, *Copy space ID*).

With `registration.mode = "invite_only"` (the default) nobody can create an account without an
invite link, so random visitors cannot sign up.

## Day-to-day administration

Everything about a space — roles, channel permissions, kicking and banning, invites, the audit
log — is in the app: right-click the space → **Space settings**.

Account problems are fixed on the server (with the install script, prefix these with
`sudo -u hermes` and add `--config /etc/hermes/hermes.toml`; with Docker, prefix them with
`docker compose exec hermes /hermes-server`):

| Command | What it does |
|---|---|
| `hermes-server user list` | lists every account |
| `hermes-server user reset-password alice` | prints a new password for `alice` and signs her out everywhere |
| `hermes-server user set-admin alice true` | makes `alice` an instance admin (`false` takes it away) |
| `hermes-server user disable alice` / `enable alice` | blocks or unblocks an account |
| `hermes-server user set-tagless alice#1234 true` | lets the account go without a `#1234` tag (`false` gives it one again) |
| `hermes-server invite create` | a new invite link (also a new owner link if nobody has registered yet) |
| `hermes-server check-config` | checks the config file and prints what is used |
| `hermes-server version` | the installed version |

These commands are safe while the server runs; it notices the changes within 10 seconds.

## Backups and upgrades

- **Backups:** the install script makes a copy of the database every night in
  `/var/backups/hermes` (the last 14 days). By hand: `hermes-server backup /path/to/backup.db` (safe while the
  server runs). Uploaded files live in `<data dir>/files`; copy that folder too.
  Docker: `docker compose exec hermes /hermes-server backup /data/backup.db`.
- **Upgrades:** re-run the install script, `docker compose pull && docker compose up -d`, or
  reinstall the panel server. The database is upgraded automatically on start. Clients and
  servers of different versions work together as long as they speak the same protocol version;
  the app tells you when one of them needs an update.

## Restoring a backup

Stop the server, then run `hermes-server restore <file>`: it checks the file, keeps the current database beside it
and puts the backup in place. Copy `<data_dir>/files` back as well. The nightly job keeps 14 days of the database, and
daily copies of the files that hard-link what has not changed, so they take little room.

## Settings worth knowing

- `network.trusted_proxies` takes addresses or ranges, such as `10.0.0.0/8`.
- `[privacy]`: `message_retention_days`, `deleted_message_retention_days` and `audit_retention_days` delete for good
  what is older (0 keeps it). They run once an hour.
- `[limits]`: `session_idle_days` and `max_sessions_per_user` end unused sessions, `max_connections` and
  `max_connections_per_ip` cap sockets, `actions_per_minute` limits edits, reactions and the like per person, and
  `max_mentions_per_message` also works on encrypted messages.
- Instance admins can change who may register in the app (Settings, Instance). That wins over the config file.
- Someone who lost their authenticator: `hermes-server user disable2fa <name>`.

Automatic moderation of message text cannot exist where channels are end-to-end encrypted, because the server cannot
read them. Reports carry what the reporter chooses to share.

## Troubleshooting

| Problem | Fix |
|---|---|
| The app says it cannot reach the server | Check that TCP 7700 is open in every firewall (the server's own and the provider's), and that `public_host` is right: `hermes-server check-config`. |
| Text works but voice does not connect | UDP 7700 is blocked. Open **UDP** 7700 in every firewall; with a reverse proxy, UDP must go straight to Hermes. Some school or office networks block UDP entirely. |
| "The certificate changed" warning | The server's own certificate was replaced (for example the data folder was deleted). If you did that, choose *Trust the new certificate*; otherwise do not continue. |
| The setup link expired | `hermes-server invite create` prints a new one while nobody has registered yet. |
| Let's Encrypt failed during install | Port 80 must be reachable from the internet and the domain must point at this server. Re-run the installer; it falls back to a self-signed certificate meanwhile. |
| *address already in use* on start | Another program (often another Hermes server) uses the port. `sudo ss -tulpn \| grep 7700` shows which; stop it or give this server another port. |
| Something else | `journalctl -u hermes -n 100` (or `docker compose logs hermes`) shows the last log lines; in the app, *Settings → About → Copy debug info* helps when asking for help. |

## Limiting disk use

Message text is tiny next to files, so the limits are for attachments. In `[storage]`:

- `max_total_storage_gb`: everything uploaded together. Uploads are refused past it. 0 = no limit.
- `max_user_storage_mb`: what one person's uploads may take up. 0 = no limit.
- `limit_includes_messages`: `true` makes the database (the text of messages) count towards the total. Off by default,
  so text ignores the limit; a message's length is bounded by `limits.max_message_length` anyway.

Clients compress files before encrypting them when that saves at least 5 %. The server cannot do it for them: what it
stores is ciphertext. `privacy.require_channel_encryption` (on by default) refuses channel messages that are not
end-to-end encrypted; update clients together with the server.
