# Changelog

What changed in each version of Hermes, written for the people who use it. Each release takes its
notes from the section with its version number, and the desktop app shows them before it updates.

How to write it:
- Add lines under `## Unreleased` as things are made. Say what people will notice, not how it was
  done, and leave out anything that was added and taken back before the release.
- Use `### New`, `### Changed` and `### Fixed` headings, `-` lists, and indent by two spaces to
  nest a list.
- Before a release, rename `## Unreleased` to `## <version>` (e.g. `## 1.4.2`). The release
  workflow stops if that section is missing.

## Unreleased

### New
- A new installer, with a choice between a normal and a portable installation
  - A portable installation keeps its sign-ins, settings and updates in its own folder
  - The licence can be read from inside the installer
- Clicking the update notice shows what changed before you update
- A new updater window: Hermes closes while it updates and opens again on the new version

### Changed
- Right-click menus of the empty space in the server rail and the channel list open at the pointer
