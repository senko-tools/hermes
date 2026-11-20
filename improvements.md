### 1. Improve text selection accuracy

Text selection currently relies too heavily on a character's outermost visible pixels, making the first and last characters difficult to select accurately.

Treat each character's hitbox as two regions divided by its absolute horizontal midpoint:
- **Left:** Everything left of the midpoint.
- **Right:** Everything right of the midpoint.

Use the midpoint as the selection boundary. If the user starts a selection on one side and drags across the midpoint, the character should be included. Apply this consistently in both selection directions.

The goal is to make text selection more predictable and eliminate accidental inclusion or exclusion of boundary characters.

### 2. Animate images when unfocused

Add a persistent setting controlling whether animated images (GIFs and other supported formats) continue playing when the application loses focus.

- **Enabled:** Animations continue playing in the background.
- **Disabled:** Animations pause when unfocused and resume when focused.

Apply consistently across the application and reuse the existing settings architecture.

### 3. Improve vertical channel reordering

Rework how channels are moved vertically. Currently, moving a channel further down requires manually moving all the channels below it upwards, which is cumbersome.

Find a more intuitive and user-friendly approach to channel reordering.

- Make it easy to move channels to any position, regardless of their current position.
- Provide clear visual feedback during dragging, showing where the channel will be placed.
- Make the interaction smooth, intuitive, and visually consistent with the existing UI.

Avoid unnecessary interactions or awkward workarounds.

### 4. Improve application startup performance

Application startup currently has two noticeable delays:

**Window initialization:**
- The application initially displays a transparent window for several seconds.
- Eliminate this by ensuring the window is rendered properly as soon as it appears.
- Minimize startup time so the application feels almost instant.

**Server connection and initialization:**
- After the window appears, it takes several seconds for servers and their associated content to load.
- Optimize connection, initialization, and data loading to display server content as quickly as possible.
- Identify and eliminate unnecessary delays, bottlenecks, and blocking operations.

Prioritize perceived performance as well as actual startup time. Avoid hiding delays behind loading animations rather than addressing their causes.

### 5. Improve search organization

The search interface currently displays channels, DMs, settings, and keyboard shortcuts together without any categorization, making it cluttered and difficult to navigate.

- Group results and actions into clear, logical categories.
- Make each category visually distinct and easy to scan.
- Improve spacing and hierarchy to reduce visual clutter.

Keep the interface compact and intuitive without sacrificing functionality.

### 6. Improve message date separators

Center date separators within the chat window, with horizontal lines extending from either side of the date.

For example: `───── November 20, 2026 ─────`

- Keep the lines within the chat's content area; they should not touch the window edges.
- Center the date consistently regardless of message width.
- Match the existing UI styling.

### 7. Improve settings UI and layout

Many settings currently look like basic radio buttons with little visual distinction between options. Examples include date formats and user statuses (Online, Idle, Do Not Disturb, Invisible).

- Redesign these controls to be visually appealing and immediately recognizable without relying solely on text labels.
- Use appropriate icons, colors, and visual states to distinguish options.
- Improve the placement, spacing, and overall layout of settings.
- Ensure controls remain intuitive and accessible.

Apply these improvements consistently across the settings interface.

### 8. Improve profile popup and editing experience

The current profile editing experience is slow, unresponsive, and visually disconnected from the normal profile popup. Remove the separate edit profile window entirely and integrate editing directly into the existing profile popup.

**Expected behavior:**
- Keep the same overall layout and appearance as the normal profile popup, but enlarge it to provide more room for avatar and banner editing.
- Make editable elements directly interactive. For example, clicking the username should allow it to be edited.
- Use small pencil icons to indicate editable elements.
- Add a subtle hover effect to editable elements, such as a slight color change.
- Allow users to change their status (Online, Idle, Do Not Disturb, Invisible) directly from the profile popup.
- Color-code status indicators using the following theme configuration variables:
  - Online: `presence.online`
  - Idle: `presence.idle`
  - Do Not Disturb: `presence.dnd`
  - Offline: `presence.offline`
- Read these colors from the active theme configuration. If a variable is missing, fall back to the dark theme's default value.
- Avoid unnecessary explanatory text, labels, and instructions. The interface should be self-explanatory.

**Profile scope:**
- Remove per-server profiles entirely from the client-side implementation.
- Keep only the global user profile for now.
- Per-server profiles will be implemented separately in the future. Do not build or retain client-side functionality for them.

**Performance:**
- Significantly improve the opening time and responsiveness of the profile popup.
- Avoid unnecessary loading, blocking operations, or delays when entering editing mode.

The result should feel like a natural extension of the existing profile popup rather than a separate settings interface.

### 9. Improve settings layout and space utilization

Make the settings content fill the available space within its container instead of leaving unnecessary empty areas.

- Expand the settings content to use the available width and height appropriately.
- Ensure elements are positioned and spaced consistently.
- Keep the layout balanced and visually cohesive without stretching individual controls unnecessarily.
- Maintain appropriate margins and padding.