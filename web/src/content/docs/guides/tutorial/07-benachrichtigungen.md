---
title: "07 – Notifications"
---

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

## Per-calendar default reminders

Besides the reminders on individual entries, you can set **default reminders per
calendar**. They apply to events in that calendar that carry no reminder of
their own – handy for subscribed or external calendars whose entries arrive
without an alarm. Under **Settings → Calendars**, pick a calendar and set its
default reminders there.

### All-day events and birthdays

All-day events have no time of day, so a reminder isn't fired "one hour before"
(i.e. the previous evening) but **at the day-carryover time** – the same time as
your day-start reminders. A lead time counts in whole **days**: "1 week before"
fires seven days earlier at the day-carryover time.

The same applies to the automatically generated **birthday calendars**. By
default they fire no reminder; under **Settings → Calendars** (on the phone via
**Reminders** in the calendar list) you can give them a default reminder, e.g.
"1 week before".

### Cancelled events

Cancelled events (for example a meeting series withdrawn by its organizer)
**never** trigger reminders — regardless of any setting. By default they stay
visible in the calendar (like Outlook); you can hide them under
**Settings → General** via **Show cancelled events**. When shown, a cancelled
event is dimmed with its title struck through, and screen readers announce it
with a trailing “cancelled”. Deleting a single occurrence of a recurring event
removes that occurrence entirely — it does not linger as a cancelled row.

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

## Launch at login

So reminders fire again after a restart without you opening Aperio by hand,
turn on **"Launch Aperio when I sign in"** under **Settings → General →
Startup**. Aperio then starts automatically once you log in to this computer.
A second option, **"Start minimized in the system tray"** (on by default when
a tray is available), controls whether the autostart launch opens a window or
starts tucked in the tray — click the tray icon to bring it up. The settings
apply to this device only; clear the checkbox to turn autostart off again.

## Summary

You can create reminders, set their sounds and respond to them with dismiss
or snooze. Next up is search.
