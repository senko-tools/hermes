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
  - Not there yet: sharing your screen, push-to-talk, hiding to the tray (closing the window quits) and updating in place (Hermes says when a new version is out)

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
