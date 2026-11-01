# Hosting your own Hermes server

This guide is for people who have never run a server before. Pick **one** of the three ways
below; each takes about 15 minutes. At the end you will have a link that you open in the Hermes
app to create the owner account, and invite links for your friends.

- [What you need](#what-you-need)
- [Way 1: a Linux server with the install script (recommended)](#way-1-a-linux-server-with-the-install-script)
- [Way 2: Docker](#way-2-docker)
- [Way 3: a game server panel (Pterodactyl / Pelican)](#way-3-a-game-server-panel-pterodactyl--pelican)
- [Optional: behind Caddy or nginx](#optional-behind-caddy-or-nginx)
- [Inviting people](#inviting-people)
- [Day-to-day administration](#day-to-day-administration)
- [Backups and upgrades](#backups-and-upgrades)
- [Troubleshooting](#troubleshooting)

---

## What you need

- **A computer that is always on and reachable from the internet.** A small VPS (Hetzner,
  OVH, DigitalOcean… 1 CPU and 1 GB of RAM are plenty for a group of friends), a home server
  with port forwarding, or a slot on a game server panel.
- **One port, open for both TCP and UDP.** The default is `7700`. TCP carries the app's
  messages; UDP carries voice. If only TCP is open, everything works except voice.
- **Optional but nice: a domain name** such as `chat.example.com` pointing at the server
  (an `A` record with its IPv4 address, and `AAAA` for IPv6). With a domain the install script
  gets a free Let's Encrypt certificate. Without one, the server uses its own certificate and
  the app asks once to trust it (invite links carry its fingerprint, so friends who join with an
  invite link are not asked).

## Way 1: a Linux server with the install script

Works on Debian 12/13, Ubuntu 22.04 or newer, and most other Linux systems with systemd.

1. **Connect to the server** with SSH (on Windows: open *Terminal* and type
   `ssh root@your-server-ip`).
2. **Run the installer:**

   ```sh
   curl -fsSL https://raw.githubusercontent.com/senko-tools/hermes/main/deploy/install.sh | sudo sh
   ```

   It asks three questions:
   - *Domain or IP*: what your friends will type, for example `chat.example.com`.
   - *Server name*: shown in the app, for example `Friends`.
   - *Email for Let's Encrypt* (only with a domain): where certificate expiry warnings go.

   It then installs everything: the program, a background service that starts with the
   computer, the certificate (renewed automatically), a nightly database backup, and it opens
   the port in the `ufw` firewall if you use one.
3. **Copy the link it prints** in the framed block:

   ```
   ==================================================================
    Hermes first-time setup
    Open this link in the Hermes client to create the OWNER account:
    https://chat.example.com:7700/invite/AbC123xYz9
   ==================================================================
   ```

   Open it in the Hermes app (or paste it into *Add an instance*) and create your account. You
   are now the owner of the server and of its first space.
4. **Cloud firewall:** if your provider has a firewall in its web console (Hetzner, AWS,
   Oracle…), allow inbound **TCP 7700** and **UDP 7700** there as well (and TCP 80 if you use a
   domain, for the certificate).

Useful commands afterwards:

```sh
sudo systemctl status hermes        # is it running?
sudo journalctl -u hermes -f        # live log (Ctrl+C to stop watching)
sudo systemctl restart hermes       # after editing /etc/hermes/hermes.toml
```

To change the domain, name or port later, re-run the installer with options, for example
`curl … | sudo sh -s -- --domain new.example.com --reconfigure`. Re-running it without options
upgrades to the newest version and keeps your data.

## Way 2: Docker

If the computer already runs Docker:

```sh
mkdir hermes && cd hermes
curl -LO https://raw.githubusercontent.com/senko-tools/hermes/main/deploy/docker/compose.yaml
nano compose.yaml          # set HERMES__INSTANCE__PUBLIC_HOST and HERMES__INSTANCE__NAME
mkdir -p data && sudo chown 10001:10001 data
docker compose up -d
docker compose logs hermes # shows the first-time setup link
```

Open the printed link in the Hermes app. Docker publishes TCP and UDP 7700; open both in any
firewall in front of the machine.

The image uses its own (self-signed) certificate. To use a real one, mount the certificate
files and add `HERMES__TLS__MODE: "files"`, `HERMES__TLS__CERT_FILE` and
`HERMES__TLS__KEY_FILE`; the server reloads them by itself when they are renewed.

## Way 3: a game server panel (Pterodactyl / Pelican)

1. **Panel admin:** download
   [`deploy/pterodactyl/egg-hermes.json`](../deploy/pterodactyl/egg-hermes.json) and import it in
   *Admin → Nests → Import Egg*.
2. **Create a server** with the *Hermes* egg. Give it one allocation (port); the panel forwards
   it for both TCP and UDP.
3. Fill in **Public host** (the panel node's domain or IP) and **Instance name**, then start the
   server.
4. The **console** shows the first-time setup link. Open it in the Hermes app.

To upgrade, *Reinstall* the server from the panel (set *Version* to `latest` or a tag such as
`v1.0.0`); the configuration and data are kept.

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

## Troubleshooting

| Problem | Fix |
|---|---|
| The app says it cannot reach the server | Check that TCP 7700 is open in every firewall (the server's own and the provider's), and that `public_host` is right: `hermes-server check-config`. |
| Text works but voice does not connect | UDP 7700 is blocked. Open **UDP** 7700 in every firewall; with a reverse proxy, UDP must go straight to Hermes. Some school or office networks block UDP entirely. |
| "The certificate changed" warning | The server's own certificate was replaced (for example the data folder was deleted). If you did that, choose *Trust the new certificate*; otherwise do not continue. |
| The setup link expired | `hermes-server invite create` prints a new one while nobody has registered yet. |
| Let's Encrypt failed during install | Port 80 must be reachable from the internet and the domain must point at this server. Re-run the installer; it falls back to a self-signed certificate meanwhile. |
| Something else | `journalctl -u hermes -n 100` (or `docker compose logs hermes`) shows the last log lines; in the app, *Settings → About → Copy debug info* helps when asking for help. |
