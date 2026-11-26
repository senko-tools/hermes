# Changelog

What changed in each version of Hermes, written for the people who use it. Each release takes its
notes from the section with its version number, and the desktop app shows them before it updates.

How to write it:
- Add lines under `## Unreleased` as things are made. Say what people will notice, not how it was
  done, and leave out anything that was added and taken back before the release.
- Use `### New`, `### Changed` and `### Fixed` headings, `-` lists, and indent by two spaces to
  nest a list. Keep each item on one line, however long; the app wraps it.
- Before a release, rename `## Unreleased` to `## <version>` (e.g. `## 1.4.2`). The release
  workflow stops if that section is missing.

## Unreleased

### New
- Hermes for Linux (beta): unpack Hermes-<version>-x86_64-linux.tar.gz and run ./install.sh
  - It gets a menu entry with its icon, opens hermes:// links, and can start when you sign in
  - The tray icon has Show Hermes, Mute, Deafen and Quit, and clicking a notification opens its channel
  - Sharing your screen: on X11 the share picker lists your screens and windows; on Wayland your desktop's own dialog asks what to share, and the next share reuses that choice until you pick "Choose something else"
  - Push-to-talk and the global mute key work in every app on X11; on Wayland they go through the desktop's shortcut settings (KDE, Hyprland, GNOME 47 and later), elsewhere they work while Hermes is focused, and Settings says which
  - The taskbar icon shows the unread count (KDE, Dash to Dock), the window asks for attention on new DMs and mentions, and "Go idle after" works
  - Sharing sends what your apps play too (through PipeWire), never the call itself, and the pointer shows in shares on X11
  - Voice goes straight to your sound server (PipeWire or PulseAudio), so pipewire-alsa is no longer needed
  - On desktops that leave the title bar to the app (GNOME on Wayland and others), Hermes draws its own, with minimize, maximize and close, and the window can be moved and resized
  - Close to the tray and Start minimized work as on Windows, on X11 and Wayland
  - Installs made with install.sh update themselves like on Windows (from the first Linux release on)
  - Logs are kept in ~/.local/state/hermes/logs
- Screen sharing has an upload limit (Settings, Screen sharing), 8 Mbit/s by default, so a share leaves room for games on both ends

### Changed
- The hardware encoder favours quality, for sharper shares at the same upload, and small share tiles shimmer less
- People without a profile picture show a silhouette in your theme's colours
- Messages this device cannot read yet show as a placeholder whose characters slowly change, with no name or picture; the lock beside it says why
- A share opened in its own window shows only there; its tile in the call says so, and the same button brings the share back
- Sharing your screen without a hardware video encoder uses several processor cores: about twice the frame rate at 1080p
- Your profile card, opened from your name at the bottom of the sidebar, sits as far from the window's bottom as from the sidebar

### Fixed
- A screen share shown larger than it was sent no longer looks blocky or broken up
- A channel no longer keeps its unread dot when there is nothing new in it
- Opening a screen share in its own window no longer crashes Hermes
- Watching a 1080p or larger share at 60 fps no longer drops many of its pictures while the screen moves
- Coloured text and edges in shares you watch no longer get blocky colour fringes

## 1.4.3

### New
- A "Codes" link under the time and date patterns (Settings > Chat) lists every code with what it gives right now
- Settings > Appearance > Layout: choose how long the pointer rests on a long description before it scrolls, from 0 to 2 seconds
- Settings > Diagnostics: choose how long old logs are kept, from 3 to 30 days
- A user guide (docs/user-guide.md) that tours the app's features

### Changed
- Settings descriptions too long for their line no longer hide behind an (i): resting the pointer on one scrolls it to its end, and it goes round again while the pointer stays
- Updates moved to Settings > About: "Check for updates" sits by the version number and says how the check went, with "Check for updates on start" and "Auto-update" under it
- "Animate in the background" sits under "Animate pictures" in Settings > Chat, and can only be turned on while that is on
- Voice channels no longer take text messages (servers from this version refuse them)

### Fixed
- Videos show their real shape and a sharp first picture before they are played, also ones sent without a picture
- Videos recorded with the phone held upright show and play upright, no longer squeezed into a wide box
- A channel no longer stays unread after its newest message was deleted, and voice channels never show as unread
- The screen share picker no longer cuts off the bottoms of window and program names at larger text sizes; hovering a name shows it in full
- Notifications show mentions as @name and text without its formatting, instead of raw mention codes; spoilers stay hidden

## 1.4.2

### New
- See what's new before you update: clicking the update notice lists the changes of every version since yours, each version on its own, with the Update button below
- A new updater window
  - Hermes steps aside while it updates and opens again on the new version
  - The × in its corner cancels the download, and Hermes comes back as it was
  - With automatic updates on, Hermes opens straight into the updater when a new version is out
- A new Windows installer
  - The first page asks for a normal or a portable installation
  - It follows Windows' light or dark mode
  - The licence opens inside the installer, so it needs no internet
  - Installing or uninstalling only closes the Hermes in that folder; other copies keep running
  - Uninstalling can also delete your data (it asks first, and the answer starts on No)
- Portable installations
  - Sign-ins and encryption keys can stay in the Hermes folder instead of Windows' credential store (Settings > Window & startup > Portable)
  - Updates are downloaded into the folder, so nothing is left on the computer

### Changed
- An installation made by the old installer is taken over in its folder, and Apps & features lists Hermes once
- Right-click menus of the empty space in the server list and the channel list open where you clicked
