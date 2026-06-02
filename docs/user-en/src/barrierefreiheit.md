# Accessibility

Aperio was developed from the ground up to be **fully usable with a screen
reader**. This page explains the most important concepts and gives tips for
the common screen readers.

## The core principle: application mode

Aperio runs in **application mode** (`role="application"`). This means:

- You do **not** need to switch to browse (virtual) mode.
- Your **arrow keys** control the calendar and lists directly – not the
  screen reader's virtual cursor.
- Keystrokes reach Aperio directly, so all shortcuts work.

This is different from classic web pages and enables smooth, app-like work.

## Live announcements

Important status changes are announced through **live regions**, without
moving the focus, for example:

- "Event saved", "Task done",
- the view change and the focused period,
- the synchronization status,
- due reminders.

## Tips by screen reader

### NVDA

- Aperio enables focus mode automatically (application mode). If you
  accidentally end up in browse mode, `NVDA+Space` brings you back.
- Use `NVDA+Tab` to have the currently focused element announced again.

### JAWS

- Here, too, application mode ensures the arrow keys go straight to Aperio.
  If needed, force forms mode with `Enter`.
- `Insert+Tab` repeats the focus announcement.

### VoiceOver (macOS)

- With VoiceOver enabled, you navigate Aperio with the arrow keys; the
  "Quick Nav" behavior is not needed within the application area.
- `VO+F` lets you search for controls.

### Narrator (Windows)

- Scan mode is not required in Aperio; if it is active, toggle it with
  `Caps Lock+Space`.

## What you can expect

- **All** features are reachable without a mouse.
- Buttons, menus, dialogs and lists are correctly marked up and labeled.
- Visual truncation (e.g. shortened event titles in the month view) changes
  **nothing** about the announced information – you always get the full
  text.

## Feedback

If you find a place that doesn't work well with your screen reader, we'd love
a note on the [project on GitHub](https://github.com/Timtam/aperio).
Accessibility is a central goal of Aperio.
