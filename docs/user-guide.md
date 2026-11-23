# Using Hermes

A short tour of the desktop client, so that features in menus and the *More* parts of settings are
easy to find. For hosting a server, see [self-hosting.md](self-hosting.md) and the
[README](../README.md#hosting-a-server).

## Three things to know first

- **Right-click almost anything:** servers, channels, messages, people, voice users, invites.
  Every action lives in those menus. Less common message actions (quote, save, forward, pin,
  copy link) are under **More** in a message's menu.
- **`Ctrl+K` opens the command bar.** It jumps to any channel, DM, person, server, theme or
  action, such as *Mentions*, *Saved messages*, *Leave voice* and *Add an account*.
- **`Ctrl+F` searches.** The search box takes filters (see [Search](#search)).

## Getting in

- Open an invite link (`hermes://…` or the `https://…/invite/…` link a friend sends), or paste a
  server address into **Add an account**.
- You can be signed in to several servers (instances) at once. Each one has its own spaces, DMs
  and friends.
- **Portable copy:** the portable zip keeps settings, logs and keys in its own folder. See
  *Settings → Window & startup → Portable*.

## Servers, channels and DMs

- The rail on the left lists your servers. *Move up* and *Move down* in a server's menu reorder
  them.
- **Text channels** are for chat. Clicking a **voice channel** joins it.
- Drag channels to reorder them or move them into categories. You need *Manage channels* for
  this.
- **DMs and group DMs** are listed with your friends under the Friends icon on the rail. Someone
  who doesn't share a server with you may arrive as a *message request*, depending on your
  privacy settings.
- **Notifications** per server or channel are in its menu: *Notify: all / only @mentions /
  nothing*, or *Mute* for a while.
- **Unread:** `Alt+Shift+Down` / `Alt+Shift+Up` go to the next or previous unread channel.
  `Escape` marks the open channel as read. *Mark unread from here* (in a message's menu) moves
  the read mark back.

## Messages

- **Formatting:** `**bold**`, `*italic*`, `` `code` ``, fenced code blocks with a language
  (highlighted, with a copy button), `> quotes`, `||spoilers||` and links. The eye button next to
  the message box shows a formatted preview.
- **Files:** drag them in, paste them, or use the attach button. Pictures, GIFs, videos and sound
  files play in the chat (videos and sounds need the FFmpeg that comes with the installer).
- **Reply, react, edit, delete:** from the message's menu, or from the buttons shown on hover.
- **Select** (in a message's menu) picks several messages to delete at once: `Ctrl+click` adds
  one, `Shift+click` a range.
- **Saved messages** are a private bookmark list: *More → Save message*, then *Saved messages*
  in the command bar.
- **Pinned messages** belong to the channel: *Pinned messages* in the channel's menu.
- **Jump to a date:** the calendar button in the channel header.
- **Personal touches:** in a person's menu, *Name them for me…* and *Personal note…* are seen
  only by you.

## Search

Search all your servers, or one server or channel, with `Ctrl+F` or the search button. Filters
go into the search text:

| Filter | Example |
|---|---|
| `from:` | `from:alice` |
| `in:` | `in:general` (when searching one server) |
| `has:` | `has:image`, `has:file`, `has:link` |
| `before:` `after:` `during:` | `after:2026-11-01` |

The *Mentions* page (in the command bar) lists every message that mentions you.

## Voice, calls and screen sharing

- Click a voice channel to join it. The panel at the bottom has mute, deafen, screen share and
  leave. `Ctrl+Shift+M` and `Ctrl+Shift+D` toggle mute and deafen.
- **Push-to-talk** and its key are in *Settings → Voice & Audio*. That tab also has noise
  suppression, echo cancellation, automatic gain and the input sensitivity meter.
- **Per-person volume:** right-click someone in voice → *Volume…* or *Mute for me*.
- **Calls:** *Join voice* in a DM's or group DM's menu starts a call; the others are rung.
- **Screen sharing:** the share button opens a picker of screens and windows, with a switch for
  sharing their sound (when the server passes it on). Others click your share to watch it, and can pop it out into its own window.
  Quality and frame rate are in *Settings → Screen sharing*.

## Your profile and status

- Click your name at the bottom left for status (Online, Idle, Do not disturb, Invisible) and
  your profile. The profile card is edited in place: avatar, banner, display name and bio.
- *Settings → Account* has your password, two-factor authentication, the devices you are
  signed in on, and *Your data* (export, delete account). *Privacy & safety* has who may DM you
  and blocked people.

## Encryption

DMs and channels marked encrypted are end-to-end encrypted: the server stores only ciphertext.
New devices and new members get keys as described in [e2ee.md](e2ee.md). Server owners choose
per server whether new members can read earlier messages (*Server settings → Encryption*).

## Settings worth knowing

- **Settings search:** type in the box at the top of Settings. It also finds settings in folded
  *More …* parts and opens them.
- **Appearance:** themes, UI scale, text size, sidebar side and avatar shape. `Ctrl+=`, `Ctrl+-`
  and `Ctrl+0` zoom.
- **Chat:** time and date formats (the *Codes* link lists every code), motion, animated pictures,
  picture size and downloads.
- **Keybinds:** every shortcut above can be changed.
- **Diagnostics:** tests your connection to each server, your microphone and speakers. *Developer
  mode* adds *Copy … ID* to menus. *Keep old logs for* sets how long old logs stay.
- **About:** version, *Check for updates*, logs and debug info for bug reports.

## Panes

`Ctrl+\` splits the chat to the right, and `Ctrl+Shift+\` splits it downwards. `Ctrl+Tab` moves
between panes, `Ctrl+Shift+F` maximizes one and `Ctrl+W` closes it. `Ctrl+B` hides the
sidebar and `Ctrl+U` hides the member list.

## When something goes wrong

- *Reconnect* and *Go offline* are in the menu of each account (instance).
- *Settings → Diagnostics* explains what works and what doesn't.
- Logs are in the `logs` folder of your config folder (*About → Logs*). A crash leaves a
  `crash-….log` there. Attach it to a bug report.
