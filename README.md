# Hermes

A self-hostable, open-source chat platform for friend groups and small communities: spaces with text and
voice channels, private messages (1:1 and group) and voice calls. Written entirely in Rust, with a
single-binary server, a [gpui](https://www.gpui.rs) desktop client and a
[ratatui](https://github.com/ratatui/ratatui) terminal client.

> **Status:** 1.0. Text chat, DMs and group DMs, voice channels and calls, roles and permissions,
> search, and self-hosting with Docker, a Linux install script or a game server panel.

## Getting started

- **Use it:** download `Hermes-Setup-<version>.exe` (or the portable zip) from the
  [releases](https://github.com/senko-tools/hermes/releases), open an invite link from a friend
  or paste a server address. Windows SmartScreen warns about the unsigned installer: choose
  *More info → Run anyway*. The terminal client `hermes-tui` comes with it.
- **On Linux (beta):** download `Hermes-<version>-x86_64-linux.tar.gz`, unpack it and run
  `./install.sh`: it installs Hermes for your user (no root), with a menu entry and `hermes://`
  links. It needs glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 and later), and
  `ffmpeg` and `pipewire-alsa` from your package manager; the script says if one is missing.
  Not on Linux yet: screen sharing, push-to-talk, hiding to the tray (closing quits) and updating
  in place (the app says when a new version is out). `./install.sh --uninstall` removes it.
- **Host it:** see [Hosting a server](#hosting-a-server) below.
- **New to the app?** Right-click almost anything (channels, spaces, messages, people) for its
  actions, press `Ctrl+K` to jump anywhere, and `Ctrl+F` to search.
  The [user guide](docs/user-guide.md) tours the rest.

## Hosting a server

Pick **one** of the four ways below. Each ends with a **first-time setup link**: open it in the
Hermes app to create the owner account. After that, invite friends from the app (right-click the
space → *Invite people*). Running it, backups, a reverse proxy and troubleshooting are in
[`docs/self-hosting.md`](docs/self-hosting.md).

| Way | Best for |
|---|---|
| [1. Install script](#way-1-install-script-linux) | a Linux VPS or home server (recommended) |
| [2. Docker](#way-2-docker) | a machine that already runs Docker |
| [3. Game server panel](#way-3-game-server-panel-pterodactyl-pelican-reviactyl) | Pterodactyl, Pelican, Reviactyl and similar panels |
| [4. By hand](#way-4-by-hand-linux-or-windows) | trying it out, or Windows |

### What you need

- **A computer that is always on and reachable from the internet**: a small VPS (1 CPU and 1 GB
  of RAM are plenty for a group of friends), a home server with port forwarding, or a slot on a
  game server panel.
- **One port, open for both TCP and UDP.** The default is `7700`. TCP carries messages, UDP
  carries voice: with only TCP open, everything works except voice. Only one program can use a
  port, so two Hermes servers on one machine need two different ports.
- **Optional: a domain name** (for example `chat.example.com`) pointing at the server. With a
  domain, the install script gets a free Let's Encrypt certificate. Without one, the server uses
  its own certificate; the app asks once to trust it (people who join with an invite link are not
  asked).

### Way 1: install script (Linux)

Works on Debian 12/13, Ubuntu 22.04 or newer, and most other Linux systems with systemd.

1. **Connect to the server** with SSH. On Windows, open *Terminal* and type
   `ssh root@your-server-ip`.
2. **Run the installer:**

   ```sh
   curl -fsSL https://raw.githubusercontent.com/senko-tools/hermes/main/deploy/install.sh | sudo sh
   ```

3. **Answer its three questions:**
   - *Domain or IP*: what your friends will connect to, for example `chat.example.com`.
   - *Server name*: shown in the app, for example `Friends`.
   - *Email for Let's Encrypt* (only with a domain): where certificate expiry warnings go.

   It installs the server as a service that starts with the computer, gets the certificate
   (renewed automatically), sets up a nightly database backup and opens the port in `ufw` if you
   use it.
4. **Open the ports in your provider's firewall** if it has one in its web console (Hetzner,
   AWS, Oracle…): inbound **TCP 7700** and **UDP 7700**, plus **TCP 80** with a domain (for the
   certificate).
5. **Copy the link** from the framed *Hermes first-time setup* block it prints, and open it in
   the Hermes app.

To upgrade later, run the same command again: your data is kept. To change the domain, name or
port: `curl … | sudo sh -s -- --domain new.example.com --reconfigure` (`--name`, `--port` work
the same way; `--help` lists them all).

### Way 2: Docker

1. **Download the compose file** into a new folder:

   ```sh
   mkdir hermes && cd hermes
   curl -LO https://raw.githubusercontent.com/senko-tools/hermes/main/deploy/docker/compose.yaml
   ```

2. **Edit it** (`nano compose.yaml`): set `HERMES__INSTANCE__PUBLIC_HOST` to your domain or
   public IP and `HERMES__INSTANCE__NAME` to the server's name.
3. **Create the data folder** (the server runs as user 10001):

   ```sh
   mkdir -p data && sudo chown 10001:10001 data
   ```

4. **Start it:**

   ```sh
   docker compose up -d
   ```

5. **Open TCP 7700 and UDP 7700** in every firewall in front of the machine.
6. **Get the setup link** with `docker compose logs hermes` and open it in the Hermes app.

To upgrade: `docker compose pull && docker compose up -d`. For a real certificate, mount the
certificate files and set `HERMES__TLS__MODE: "files"`, `HERMES__TLS__CERT_FILE` and
`HERMES__TLS__KEY_FILE`; the server reloads them when they are renewed.

### Way 3: game server panel (Pterodactyl, Pelican, Reviactyl)

Steps 1 and 2 need the panel's admin area.

1. **Import the egg:** download
   [`deploy/pterodactyl/egg-hermes.json`](deploy/pterodactyl/egg-hermes.json) (it is also
   attached to every release). In the panel go to *Admin → Nests*, pick or create a nest (for
   example *Chat*) and choose *Import Egg*.
2. **Add a free port:** *Admin → Nodes →* your node *→ Allocations*. Add an IP (`0.0.0.0` is
   fine) and a port that **nothing else on that machine uses**. If another Hermes server
   already runs there (for example from the install script), it holds `7700`: pick another
   port, such as `7701`.
3. **Create the server:** *Admin → Servers → Create New*:
   - *Node* and *Default allocation*: the port from step 2.
   - *Nest* / *Egg*: **Hermes**. Leave the Docker image as it is.
   - *Memory*: 512 MB is enough. Give the disk room for uploaded files.
   - *Public host*: the domain or public IP friends connect to, **without** a port (the panel
     adds it).
   - *Instance name*: shown in the app.
   - *Registration mode*: leave `invite_only` unless you want anyone to be able to sign up.
   - *Version*: `latest`, or a release tag such as `v1.3.0`.
4. **Wait for the install** to finish (the server shows *Installing*). It downloads the server
   for your CPU and writes `hermes.toml`.
5. **Open the port for TCP and UDP** in the machine's firewall and the provider's, for example
   `sudo ufw allow 7701/tcp && sudo ufw allow 7701/udp`. The panel forwards the allocation for
   both.
6. **Start the server.** When the console says `Hermes is ready`, scroll up to the *first-time
   setup* link and open it in the Hermes app.

To upgrade, *Reinstall* the server from the panel (with *Version* on `latest` or the new tag): the
configuration and data are kept. The server uses the panel's port even though `hermes.toml` says
`7700`, so leave that file's port alone.

If the console shows *address already in use*, the port from step 2 is taken: check what uses it
with `sudo ss -tulpn | grep <port>` and pick another allocation.

**If the domain has an IPv6 address** (an AAAA record), the server must listen on IPv6 too. An
allocation on `0.0.0.0` is published on IPv4 only, so people whose computer prefers IPv6 cannot
reach it (older apps then fail voice while text still works):

1. *Admin → Nodes →* your node *→ Allocations*: add the machine's public IPv6 address with the
   same port.
2. *Admin → Servers →* the server *→ Build Configuration*: add it under *Additional
   Allocations* (keep the `0.0.0.0` one as the primary).
3. *Admin → Servers →* the server *→ Startup*: set *Bind address* to `::`.
4. Allow the port for IPv6 in the firewall too (`ufw allow` covers both when `IPV6=yes` in
   `/etc/default/ufw`), and restart the server.

*Settings → Diagnostics* in the app tests HTTPS and voice on each of the server's addresses.

### Console commands

The server reads commands typed into its console (the panel's console, or the terminal it runs
in). Type `help` for the list. `log` chooses what the console shows while the server runs:

| Command | What it does |
|---|---|
| `log` | what is shown now |
| `log connections on` | every TCP/TLS connection opened and closed, and failed handshakes |
| `log requests on` | every HTTP request: address, method, path, status, time |
| `log gateway on` | people's gateway sessions starting, resuming and ending, with their address |
| `log voice on` | joining and leaving voice channels (nothing about the audio itself) |
| `log auth on` | sign-ins, refused sign-ins, new accounts, sign-outs |
| `log all off` | hide every topic (`off` works for each one too) |
| `log level debug` | the base level: `trace`, `debug`, `info`, `warn`, `error`, or a filter like `info,hermes_server::voice=debug` |
| `log reset` | back to the config file's settings |

Changes last until the server restarts. `topics` and `level` under `[log]` in `hermes.toml` set
what shows at start (by default `gateway`, `voice` and `auth`).

### Way 4: by hand (Linux or Windows)

Good for trying Hermes out. Unlike Way 1 it does not start by itself after a reboot or renew
certificates.

1. **Download the server** from the [latest release](https://github.com/senko-tools/hermes/releases/latest):
   - Linux on a normal PC or VPS: `hermes-server-x86_64-unknown-linux-musl.tar.gz`
   - Linux on ARM (Raspberry Pi 4/5, Ampere): `hermes-server-aarch64-unknown-linux-musl.tar.gz`
   - Windows: `hermes-server-x86_64-pc-windows-msvc.zip`
2. **Unpack it into a new folder.** On Linux: `mkdir hermes && tar -xzf hermes-server-*.tar.gz -C hermes && cd hermes`.
   On Windows: right-click the zip → *Extract All*, then open *Terminal* in that folder.
3. **Write the config file:** `./hermes-server init` (Windows: `.\hermes-server.exe init`). This
   creates `hermes.toml`.
4. **Edit `hermes.toml`:** set `public_host` under `[instance]` to your domain or public IP, and
   `name` to the server's name.
5. **Open the port for TCP and UDP.**
   - Linux with ufw: `sudo ufw allow 7700/tcp && sudo ufw allow 7700/udp`
   - Windows: in *Terminal* opened as administrator:

     ```powershell
     New-NetFirewallRule -DisplayName "Hermes TCP" -Direction Inbound -Protocol TCP -LocalPort 7700 -Action Allow
     New-NetFirewallRule -DisplayName "Hermes UDP" -Direction Inbound -Protocol UDP -LocalPort 7700 -Action Allow
     ```

   - At home, also forward TCP and UDP 7700 on your router to this computer.
6. **Start it:** `./hermes-server run` (Windows: `.\hermes-server.exe run`). It keeps running
   while the window is open; `Ctrl+C` stops it.
7. **Open the setup link** it prints in the Hermes app.

The database and uploads go to the `data` folder next to `hermes.toml`. To upgrade, stop the
server, replace the program file with the new one and start it again.

The full specification and build plan is [`PLAN.md`](PLAN.md). Deviations from it are logged in
[`docs/DECISIONS.md`](docs/DECISIONS.md).

## Crates

| Crate | What it is | License |
|---|---|---|
| [`hermes-proto`](crates/hermes-proto) | Shared wire types, permissions, markup parser. No I/O. | MIT OR Apache-2.0 |
| [`hermes-server`](crates/hermes-server) | Server binary and library | AGPL-3.0-or-later |
| [`hermes-audio`](crates/hermes-audio) | Audio engine (capture, playback, Opus, jitter buffer, mixer) | GPL-3.0-or-later |
| [`hermes-client-core`](crates/hermes-client-core) | Connection, REST and gateway client, state store, voice transport | GPL-3.0-or-later |
| [`hermes-gui`](crates/hermes-gui) | Desktop client (gpui) | GPL-3.0-or-later |
| [`hermes-tui`](crates/hermes-tui) | ratatui terminal client | GPL-3.0-or-later |
| [`hermes-voicebot`](crates/hermes-voicebot) | Dev tools: headless voice client, `hermes-loadtest` | GPL-3.0-or-later |

Direct messages and voice are end-to-end encrypted; [docs/e2ee.md](docs/e2ee.md) says how, and what that does and
does not protect.

## Building

Requires Rust 1.99 or newer.

```bash
cargo build --workspace
```

Checks that must pass before a change is merged (CI runs them on Linux and Windows):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

Each crate carries its own license (see the table above). The full texts are in
[`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE), [`LICENSE-GPL-3.0`](LICENSE-GPL-3.0) and
[`LICENSE-AGPL-3.0`](LICENSE-AGPL-3.0).
