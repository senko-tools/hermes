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
