# 06 – Notifications

In this chapter you set up reminders for events and tasks and learn how to
react to them.

## Adding a reminder

1. Open an event or a task for editing.
2. In the **Reminder** field, choose how long beforehand you want to be
   notified (e.g. 10 minutes, 1 hour, 1 day).
3. You can add several reminders per entry.
4. **Save**.

## How reminders appear

At the due time, Aperio shows a notification – as a system notification
and/or as a hint inside the app, depending on your settings. Optionally a
**sound** is played.

In the notification you can:

- **open** the entry,
- **dismiss** the reminder (acknowledge it),
- or **snooze** it – the reminder returns after the chosen time.

## Notification sounds

You can choose which sound a reminder plays, on several levels — each one
overrides the one above it:

1. **Global default** — Settings → Calendars → *Notification sounds*.
2. **Per calendar / per task list** — in the same settings panel, on each
   calendar or list row.
3. **Per event / per task** — in the event or task dialog (while editing an
   existing entry).
4. **Per reminder** — directly on a single reminder row.

Every level offers the same choices:

- **System default** — your operating system's notification sound.
- **Silent** — a visual-only notification, no sound.
- **Custom sound** — import your own audio file (`.mp3`, `.ogg`, `.wav`,
  `.m4a`, `.aac`, `.flac`, up to 5 MB). Use **Test** to preview it and
  **Remove** to delete an imported sound.

Everything below the global level also offers **Use default**, which means
"inherit the level above". Imported sounds and your choices sync to your
other devices (the audio file travels with the setting), so a reminder
sounds the same everywhere.

> **Volume:** Aperio deliberately has no in-app volume slider — use your
> operating system's per-app volume mixer (Windows and macOS both have
> one).

## Notification settings

In the **settings** under **Notifications** you define:

- whether system notifications are used,
- whether and which **sound** is played (see *Notification sounds* above),
- the default snooze duration,
- the default lead time for new events.

> **Note:** For system notifications to appear, Aperio needs the
> corresponding permission from your operating system. You are asked for it
> the first time.

> **Screen-reader note:** Reminders are announced through a live region, so
> you notice them even without a visible focus change. The "Open",
> "Dismiss" and "Snooze" buttons are reachable with `Tab` and clearly
> labeled.

## Running in the background (system tray)

So reminders fire even when the window isn't open, Aperio can hide into the
**system tray** instead of quitting. Under **Settings → General**:

- **Minimize to the tray when closing** – the close button hides Aperio
  instead of quitting.
- **Minimize to the tray instead of the taskbar** – the minimize button
  tucks the window into the tray.

Click the tray icon to bring the window back; use its menu to actually quit.
Both options are off by default. If your system has no tray (e.g. GNOME
without an AppIndicator extension), the toggles are disabled and the window
behaves normally.

## Summary

You can create reminders, set their sounds and respond to them with dismiss
or snooze. Next up is search.
