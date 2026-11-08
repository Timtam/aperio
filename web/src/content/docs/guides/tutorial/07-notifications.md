---
title: "07 – Notifications"
---

In this chapter you set up reminders for events and tasks and learn how to
react to them.

## Adding a reminder

1. Open an event or a task for editing.
2. Under **Reminders**, **Add reminder** adds a new row.
3. In the **Type** field, choose when it comes:
   - **Before start**, for tasks **Before due**: how long beforehand, with
     **Amount** and **Unit** (minutes, hours, days). A new row starts at 15
     minutes.
   - **At start**, for tasks **When due**: exactly when the event starts or
     the task is due. A task without a time rings at 9 a.m.
   - **After start**: how long afterwards, with **Amount** and **Unit**. It is
     only there where it arrives: in calendars that keep such a reminder
     (iCloud and other CalDAV calendars, Aperio's own calendars), and for a
     reminder **Only in Aperio**. Google, Exchange and Outlook only know
     reminders before the start; a row there that has "After start" anyway,
     say from a copied event, says that it will not arrive. It rings even if
     the event is over by then. For tasks it is called **After due**; Aperio
     shows it there but does not offer it anew.
   - **At fixed time**: on a date at a time.
   - **On next app start after due**: Aperio tells you once the moment has
     passed: on the desktop the next time it starts, on the phone the next
     time you open it, once on each device. For a repeating event or task
     that is once after each occurrence; several missed occurrences give one
     reminder. A done or cancelled task stays quiet. This only works for
     entries in Aperio's own calendars and lists, since no provider stores
     this kind.
4. You can add several reminders per entry.
5. **Save**.

On an all-day event the list looks different, see
[All-day events and birthdays](#all-day-events-and-birthdays).

A reminder you set in another app for the moment of the event, such as "At
time of event" on the iPhone, shows in Aperio as **At start** and stays that
way when you save.

## Notifications on Windows

Windows shows notifications only from programs that have an entry in the
Start menu. Aperio runs without an installer, so it has no such entry until
you ask for one. Without it, Windows shows none of Aperio's reminders: no
banner, and nothing in the notification center. Aperio does not show the
reminder in its own window instead.

Aperio never creates the entry by itself. Three places offer it:

- **Settings, General, Windows notifications.** This section explains why the
  entry is needed and says where it stands. **Set up notifications** creates
  it, and **Remove Start menu entry** removes it again. The section exists
  only on Windows.
- **A hint after a hidden reminder.** When a reminder comes due and Windows
  would hide it, Aperio opens the dialog **Reminders are not showing**. **Set
  up notifications** creates the entry, and **Not now** closes the dialog.
  Aperio asks at most once per start. While its window does not have focus,
  another dialog is open, or you are typing in a field, the hint waits. After the next start it can ask
  again, also if you removed the entry or skipped the wizard step.
- **The first-launch wizard.** On a fresh install it has a step **Reminders
  on Windows** after the language (see
  [Chapter 01](/guides/tutorial/01-installation/)). **Next** moves on without
  setting up.

The entry is a shortcut named Aperio in your own Start menu folder. It needs
no administrator rights and applies only to your Windows user. Once it
exists, Aperio also appears in the Start menu, and you can start it from
there.

If you move Aperio's folder or start another copy of Aperio, the entry still
points at the old place. At start, Aperio points its entry at itself again.
It never creates a missing entry at start, and it leaves alone an entry named
Aperio that it did not make. A shortcut key you set on the entry stays.

Before you delete Aperio's folder, remove the entry under **Settings,
General, Windows notifications**. Otherwise the Start menu keeps an entry
that leads nowhere.

A development build started from a build folder (`target\debug` or
`target\release`) shows its notifications as **Windows PowerShell**. It needs
no entry, and the section offers no buttons there.

> **Screen-reader note:** The hint dialog opens with focus on its
> explanation, so you hear why before you hear the buttons. `Tab` moves to
> **Set up notifications**, then to **Not now**. After setting up, Aperio
> says "Notifications set up. Windows can now show Aperio's reminders." and
> the dialog closes.

## How reminders appear

At the due time, Aperio shows a notification of your operating system. On
Windows this needs the Start menu entry described above. Optionally a
**sound** is played.

In the notification you can:

- **open** the entry,
- **dismiss** the reminder (acknowledge it),
- or **snooze** it – the reminder returns after the chosen time.

## Per-calendar default reminders

Besides the reminders on individual entries, you can set **default reminders per
calendar** – handy for subscribed or external calendars whose entries arrive
without an alarm. Where each entry applies is your choice, and it decides
whether the entry rides on top of an event's own reminders or stands in for
them (see below). Under **Settings → Calendars**, pick a calendar and set its
default reminders there.

Each default reminder also says where it lives. **Only in Aperio** is what every
entry starts with: Aperio reminds you itself, and nothing is written into the
event – exactly how the iOS Calendar app treats its own "Default Alert Times".
**Attached to new events** instead writes that entry into every appointment you
create in the calendar and leave without reminders of your own, as the event's
own reminder. That is what makes other apps reading the same calendar remind you
too – the iOS Calendar app, or a voice assistant reading your iCloud calendar.
Only appointments you create *after* the switch get it written in; an
appointment you gave reminders of your own, or deliberately none, keeps that
choice, and an event without reminders of its own still gets the entry in
Aperio. Exchange and Outlook calendars store a single "before start" or "at
start" reminder, so only the first such entry is attached there; an entry at a
fixed time does not reach those appointments.

An attached entry is not a surprise at save time. Create an appointment in such
a calendar and the editor already shows those rows, marked **on this event**:
they are ordinary reminder rows, so you can change the lead time, add another,
or take them out again, and what you leave is what the appointment gets.
Removing every row is a choice like any other — no default slides back in
afterwards. Entries that stay **only in Aperio** are not shown there, because
they are not the appointment's: they ring on top of whatever it carries, and
the calendar's settings page is where they belong.

A default reminder can be a lead time ("before start"), come at the start
("at start"), after the start (where the calendar keeps that, or **Only in
Aperio**) or be a fixed date and time. A birthday calendar picks a day and a
time, like an all-day event.
"On next app start" is missing here on purpose: it only ever fires for a
reminder set on an entry itself, so as a calendar default it would be a setting
that saves and then stays silent.

### Apple Calendar and its default alert

An iCloud account has a default alert of its own (iPhone: **Settings → Apps →
Calendar → Default Alert Times**). The iPhone adds it to every appointment
that does not say otherwise. Appointments it makes itself say so with a silent
alarm that never rings; an appointment that came from elsewhere without one
gets the default on top of its own alerts.

Aperio writes that silent alarm into every appointment it saves to iCloud: a
new one, and every one you save. The iPhone then shows exactly the reminders
the appointment has in Aperio and adds none of its own, so an appointment
without a reminder has no alert on the iPhone either. If you want the iPhone,
or a voice assistant reading the calendar, to alert you, give the appointment a
reminder, or set one of the calendar's default reminders to **Attached to new
events**. An entry **Only in Aperio** stays in Aperio.

An appointment saved before this version may still show the default beside its
own alert, and one made on an iPhone and edited in Aperio may show an extra
alert "at time of event". Saving it once in Aperio puts that right. Aperio
writes the silent alarm only to iCloud: an iPhone on another CalDAV server adds
its default to appointments from Aperio, so set **Default Alert Times** to
*None* there.

When Apple has written its default into an appointment as an alert, it marks
that alert as its own. Aperio keeps the mark: editing an appointment made on an
iPhone — its title, its time, anything — leaves the alert alone, mark and all,
so it stays Apple's default instead of turning into an alert you appear to
have set by hand. Change the reminder here, add one beside it or take it away,
and the mark goes: it is your choice then.

An alert Aperio cannot read — Apple's "1 week before", say — does not show up
in Aperio, and a save hands it back to the server exactly as it was. One that
this app would have to respell is left to the server too, and the mark is
dropped rather than guessed.

### All-day events and birthdays

An all-day event has no start for minutes to count from. Its reminder is
therefore a **day** and a **time**. In the **Type** field you pick **On the
day**, **Days before** or **Days after** (the last two with **Number of
days**). In the **Time** field you pick **At the day-start time** or **At a time
of its own** and enter it under **Own time**. The day-start time is that of the
**Day-start trigger** (**Settings → Tasks**; 00:00 by default and with "App
start only"), the same as for your day-start reminders. 00:00 as its own time
counts as the day-start time. A new row starts as "On the day" at the
day-start time.

This is also how the iPhone writes its alerts for all-day events, and how they
ring: "On day of event (9 AM)" shows as "On the day" at 09:00, "1 day before
(9 AM)" as "Days before", 1 day, at 09:00. A time on the day itself and "Days
after" lie after the event's midnight; like "After start", they are only there
where they arrive.

A calendar's default reminder applies to timed and all-day events alike. On an
all-day event it therefore counts in whole **days**, at the day-start time:
"15 minutes before" rings on the day itself, not the evening before, "1 week
before" seven days earlier. When you create an all-day event in such a
calendar, an attached default is already in this form in the editor.

The automatically generated **birthday calendars** hold only all-day events,
and their default reminders are those events' only ones. They are therefore a
day and a time, like the reminders on an all-day event, and ring exactly as
they are set, without rounding. Left as they come, they remind you **on the
day itself** at the day-start time, without you setting anything up. A birthday calendar exists because you want to be told, so
it starts switched on.

Under **Settings → Calendars** (on the phone via **Reminders** in the calendar
list) you can change that, with a day and a time as on an all-day event: say
"Days before", 7 days, at 09:00, several reminders, or remove them all — an
emptied list stays empty and the calendar goes quiet.

### Cancelled events

Cancelled events (for example a meeting series withdrawn by its organizer)
**never** trigger reminders — regardless of any setting. By default they stay
visible in the calendar (like Outlook); you can hide them under
**Settings → General** via **Show cancelled events**. When shown, a cancelled
event is dimmed with its title struck through, and screen readers announce it
with a trailing “cancelled”. Deleting a single occurrence of a recurring event
removes that occurrence entirely — it does not linger as a cancelled row.

## Reminders only you get

Every reminder you set on an appointment says where it applies, in the same row
as the time. **On this appointment** is what a reminder has always been: Aperio
writes it into the appointment, the calendar stores it, and every other client
of that calendar reminds you too – your phone's calendar app, a voice assistant
reading your account out loud, and anyone else you share the calendar with.
Exchange and Outlook calendars store a single "before start" or "at start"
reminder per appointment, so only the first attached one survives there; a
second, or one at
a fixed time, is better set to **Only in Aperio**.

**Only in Aperio** keeps it here. The appointment on the server stays untouched,
so nobody sharing the calendar sees it and no other app announces it – but your
own devices do: the reminder travels through Aperio's sync like your settings
do, and rings on your phone as well as on your desktop. That is the one for
"leave now" on a shared work calendar, or a personal nudge on an appointment
your colleagues can read.

The choice appears only where it makes a difference. A local calendar has no
other client to tell, and the calendar on your phone that Aperio only reads
stores no reminder Aperio writes — in both, reminders are Aperio's either way
and no choice is offered.

Appointment identities belong to the calendar and change underneath Aperio: a
re-sync can mint new ones, and some providers do it after every edit. A private
reminder remembers the name and start of the appointment it was set on, so it
finds it again — and Aperio keeps that note up to date as long as it can still
see the appointment, so a rename or a move does not lose it either. Only if two
appointments in the same calendar share a name and a start does Aperio leave the
reminder where it is rather than guess, and you can set it again.

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

- **System default** — your operating system's notification sound. On
  Windows this is the Windows default notification sound. On macOS, Aperio
  plays no sound for System default for now; pick a custom sound there if
  you need one.
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

The desktop settings have no page of their own for notifications. The sound
is set as described under *Notification sounds* above. Windows asks for no
permission: whether it shows Aperio's reminders depends on the Start menu
entry, see *Notifications on Windows* above. On the phone, the permission is
described under [Mobile app](/guides/mobile/).

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
