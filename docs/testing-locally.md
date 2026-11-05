# Testing on a local network

How to run a complete Hermes setup at home, apart from the production server and from your
everyday client: **server_1 and client_1 on the PC, client_2 on a laptop** connected over the
local network (Ethernet preferred). This is the setup to use when something is slow or broken and
you need to know whether the server, the network or a client is the cause.

## 1. Build (on the PC)

Build release binaries. OpenH264 (screen sharing) needs NASM on `PATH` or its encoder is about
3.5 times slower:

```powershell
$env:Path += ";C:\Program Files\NASM"
cargo build --release -p hermes-server -p hermes-gui
```

You get `target\release\hermes-server.exe` and `target\release\hermes.exe`. Copy `hermes.exe` to
the laptop (any folder), or build it there the same way.

## 2. Find the PC's address

Run `ipconfig` on the PC and note the IPv4 address of the Ethernet adapter, for example
`192.168.1.20`. In Windows Settings → Network & internet → Ethernet, set the network profile to
**Private** (the firewall rules below only apply to private networks).

## 3. Configure the test server

Use a folder of its own, for example `A:\hermes-lan`:

```powershell
mkdir A:\hermes-lan; cd A:\hermes-lan
A:\Projects\hermes\target\release\hermes-server.exe --config hermes.toml init
```

Edit `hermes.toml`:

```toml
[instance]
name = "LAN test"
public_host = "192.168.1.20"    # the PC's address from step 2

[network]
port = 7799                     # anything but the 7700 of a real server

[registration]
mode = "open"

[storage]
data_dir = "./data"             # relative to the folder you start the server in

[log]
level = "info"                  # the server logs a screen share's numbers every 10 s
```

## 4. Open the firewall (once, PowerShell as administrator)

Voice and screen sharing use QUIC over **UDP**, the rest uses TCP, both on the same port:

```powershell
New-NetFirewallRule -DisplayName "Hermes LAN test (TCP)" -Direction Inbound -Protocol TCP -LocalPort 7799 -Action Allow -Profile Private
New-NetFirewallRule -DisplayName "Hermes LAN test (UDP)" -Direction Inbound -Protocol UDP -LocalPort 7799 -Action Allow -Profile Private
```

Remove them later with `Remove-NetFirewallRule -DisplayName "Hermes LAN test*"`.

## 5. Start the server

```powershell
cd A:\hermes-lan
A:\Projects\hermes\target\release\hermes-server.exe --config hermes.toml
```

Leave this window open: its output is the server's side of every test. On the first start it
prints a link that creates the owner account. For more accounts, in a second window:

```powershell
cd A:\hermes-lan
A:\Projects\hermes\target\release\hermes-server.exe --config hermes.toml invite create
```

## 6. Start the two clients

Each test client gets its own `--config-dir`, so your everyday client (its settings, accounts and
the server it is connected to) is never touched. Both can run at the same time as it.

On the PC (client_1):

```powershell
A:\Projects\hermes\target\release\hermes.exe --config-dir A:\hermes-lan\client1 "<link from step 5>"
```

On the laptop (client_2):

```powershell
.\hermes.exe --config-dir C:\hermes-lan\client2 "<a link from invite create>"
```

Register an account in each (for example `pc` and `laptop`). The server's certificate is
self-signed; the fingerprint in the link is trusted automatically.

To turn up the clients' logging, set `HERMES_LOG` before starting them, for example
`$env:HERMES_LOG = "info,hermes_gui::screen=debug"`. Client logs are in
`<config dir>\logs\hermes.log`.

## 7. Test a screen share

Create a space with a voice channel, join it with both clients, share from one and watch from the
other. While a share runs, every step logs one line every 10 seconds:

| Where | Log line starts with | Tells you |
|---|---|---|
| Sharer (client log) | `screen share (GPU, …)` or `(desktop duplication, OpenH264, …)` | frames captured and sent per second, capture and encode time, frames dropped by a busy encoder or refused by a full connection queue, keyframes, bitrate |
| Sharer (client log) | `screen share transport (sent)` | how long frames take to leave over QUIC |
| Server (console) | `screen share of <user>` | frames in per second, frames over the rate limit, frames queued for watchers, frames lost to a watcher's full queue, keyframe requests |
| Watcher (client log) | `screen share transport (received)` | frames arriving per second, time on the wire, frames not handed to the decoder |
| Watcher (client log) | `screen share of <user> (decoding)` | decode time, waiting time, pictures the window had no time for, arrival jitter and delay drift |
| Watcher (client log) | `screen shares drawn` | new pictures reaching the GPU per second |

## 8. Reading the results

Follow the frames from left to right; the first step whose rate falls below the target is the
problem:

- **Sharer captures or sends less than the target**, or "dropped by a busy encoder" is high: the
  sharer's capture/encode is too slow (client side, sharer).
- **"refused by a full connection queue"** on the sharer, or slow "time on the wire": the upload
  from the sharer to the server is congested.
- **Server: "over the rate limit"**: the sharer sends faster than the protocol allows.
- **Server: "lost to full queues"**: the server cannot get frames to that watcher fast enough
  (the network to the watcher, or the watcher reading too slowly).
- **Watcher receives the full rate but decodes or draws fewer**: the watcher's decoding or drawing
  (client side, watcher). A growing "delay drift" means a queue is filling up somewhere.
- Each lost frame makes the picture wait for the next keyframe; many keyframes in the logs
  usually mean frames are being lost somewhere upstream.

To narrow it down further:

- **Swap roles** (the laptop shares, the PC watches) to tell a weak machine from a code problem.
- **Take the network out**: run a second test client on the PC as well
  (`--config-dir A:\hermes-lan\client3`, with a link from `invite create`; traffic between
  programs on one PC never leaves it). If that is smooth and the laptop is not, look at the
  network or the laptop.
- **Take the clients out**: the transport benchmark pushes share-sized frames through a real
  local server with no capture or codec:

  ```powershell
  $env:HERMES_BENCH_FPS = 60; $env:HERMES_BENCH_KBPS = 25000
  cargo test -p hermes-client-core --release --test voice share_transport_benchmark -- --ignored --nocapture
  ```

  It prints frames sent, received and their delay (median, p95, p99, max).
- **Measure the codec alone**: `cargo test -p hermes-gui --release real_screen -- --ignored --nocapture`
  captures this PC's screen and prints the time of each step, on the CPU path and on the GPU path
  (1080p, the screen's size and 4K, encoding and decoding).
- **Measure a whole share on one PC**: `cargo test -p hermes-gui --release share_end_to_end -- --ignored --nocapture`
  shares this PC's screen through a real local server to a second client and prints the frames
  received, their spacing and the decode time (`HERMES_SHARE_RES`, `HERMES_SHARE_FPS`,
  `HERMES_SHARE_PREF`, `HERMES_SHARE_SECS` shape it). Frames only flow while the screen changes:
  play a video while it runs.

Task Manager → Performance → GPU shows the "Video Encode" and "Video Decode" engines: with hardware
encoding they carry the work instead of the CPU.
