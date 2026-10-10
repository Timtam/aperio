---
title: "Troubleshooting & Logs"
---

When something misbehaves, Aperio's logs are the fastest way to find out why.
Aperio keeps a rolling log file on your device — even in normal (release)
builds — so you can export it and send it along with a bug report.

## The Logs settings

Open **Settings → Logs** (German: *Protokolle*). There you can:

- **Set the detail level.** *Normal* is the default and right for everyday
  use. Switch to *Debug* or *Trace* only while you reproduce a problem — they
  record much more, which makes the log noisier. The choice is remembered on
  this device and is **not** synced to your other devices.
- **View the recent log** — the latest lines of the current log file, with a
  **Refresh** button.
- **Export the log to a file** — pick where to save it, then attach it to your
  report.
- **Copy the log to the clipboard** — handy for pasting into an issue or chat.
- **Clear logs** — removes the stored log files (the current session keeps
  logging).

## Privacy

The export is meant to be shared, so **Remove personal data** is on by
default: it replaces e-mail addresses and access tokens with placeholders
before the log leaves your device. Aperio never logs your passwords, sync
passphrase, or account tokens in the first place — those live only in your
operating system's keychain. Leave the redaction option on unless support
explicitly asks for an unredacted log.

## Where the logs live

The log files are stored under your data directory, in a `logs/` folder
(`aperio.log.<date>`). Settings → Logs shows the exact path with a **Copy
path** button. Files older than 14 days are removed automatically.

## An account stops updating

If a connected account can no longer be refreshed — most commonly because its
password or app password was changed or revoked — Aperio keeps showing the
last data it has and warns you instead of failing silently:

- **Desktop:** the account in the sidebar carries a warning, and a polite
  screen-reader announcement points you at **Settings → Accounts**. There the
  affected account lists each failing calendar or list, the provider's error,
  and when the last successful update happened. If the errors look like a
  login problem, a **Re-enter password** button opens the reconnect flow
  directly.
- **Mobile:** the sync button in the header turns into a warning (its label
  says some accounts are not updating), the details are on the **Sync**
  screen, and the affected account on the accounts screen gets a
  **Reconnect** button to re-enter the password or redo the provider sign-in.

The sentence that ends a refresh names what it could not update: "External
data updated, except: Work and Home." (on the phone after every refresh, on
the desktop after one you started). If the refresh could not read anything at
all, it says that nothing could be updated, and the warning with its cause
follows; a first network hiccup that is not confirmed yet only says that the
refresh ended. The accounts it names are not announced a second time, unless their
cause grows more severe, and their warnings appear together with the
sentence.

When the operating system withholds an account's data — the phone's own
calendars without access — the account shows one line instead of one row per
calendar ("Aperio may not read the calendars"), and on the phone a button that
opens the accounts, where **Allow access…** is. A withheld grant comes before
a login problem, and a login problem before anything else, wherever Aperio
names only one of them. An account that is already failing is announced again
only when its cause grows more severe.

A brief, one-off connection hiccup does not raise the warning: a network
failure is only shown once it recurs, so a cold start on a not-yet-ready
network never flashes a false alarm. A login problem — which never fixes
itself — shows straight away, and a manual refresh always reports its
result at once, also in the first seconds after the app starts. The warning
clears by itself as soon as an update succeeds again.

## This device stops updating after a move to a new phone

Moving to a new iPhone brings Aperio's data along, the **This device**
account included, but not the permission to read the phone's own calendars
and reminders: iOS asks for that on each phone anew. Until it is given,
Aperio keeps showing what it last read, and the account does not update.

When the account exists and iOS has never asked on this phone, Aperio asks at
start, once the app is unlocked and the first screen has loaded. Allow full
access to the calendars and to the reminders; Aperio then updates the account
straight away and says so.

While access is missing, **This device** under Settings → Accounts carries a
badge that names what is missing (**No access**, **Calendars without access**
or **Reminders without access**) and the action **Allow access…**. If iOS has
not asked yet, it asks now. If you declined, iOS does not ask again: the
action then says what is missing and opens Aperio's page in the Settings app.
Set **Calendars** and **Reminders** to **Full Access** there (up to iOS 16:
turn them on). "Add events only" is not enough: Aperio has to read your
calendars. When you come back to Aperio, it updates the account and says so,
or says what access is still missing. If a restriction such as Screen Time
forbids access, Aperio says that too; only that restriction can change it.

On Android the same holds for the calendars. When the account exists and
Aperio has never asked on this phone — a new phone included — it asks once at
start. After that the action asks while Android still asks, and otherwise
leads to Aperio's permissions in the Android settings.

There is one **This device** account per phone. Choosing **This device**
again under Add account does not add a second one: while access is missing it
leads to the same **Allow access…**, and otherwise it says the account is
already added and moves to it. If **This device** shows twice (added twice by
an older version), every device calendar appears twice; delete one of the
two.

## A task's time shifted, once

Tasks with a **time of day** on a **CalDAV** account (iCloud Reminders,
Nextcloud, Radicale, Tasks.org) used to be stored by Aperio as a UTC time, even
though the time is a local wall clock. Aperio never noticed, because it made the
same mistake in reverse when reading — but in every other program the task sat
at the wrong hour, off by your time-zone offset.

From this version the time is written as what it is. Tasks that **Aperio itself**
created with a time therefore shift **once**, by exactly that offset — a 09:00
task reads as 11:00 in central Europe. Correct it once and it stays put. Tasks
created in another program were wrong before and are right now.

Two more things are fixed with it: a task whose time carries a **time zone** (how
Thunderbird, Tasks.org and Nextcloud write it) did not merely lose its time here,
it lost its **whole day** and sat undated in the backlog. And a task from
**Microsoft To Do**, created by someone in their own time zone, could show up a
day early.

## A recurring appointment is an hour off after the clocks change

Saving a recurring appointment in Aperio's editor used to drop its time zone.
That affected every series with a time zone that was edited as a whole in
Aperio since June 2026, including series created elsewhere, for example on an
iPhone, in Outlook or in Google Calendar, and on local calendars as much as on
iCloud, Google, Microsoft 365 or Exchange. An appointment turned into a series
in the editor got no time zone at all.

Without its zone, a series keeps its time in UTC. After the clocks change it
shows an hour early or late, here and in every other program that reads the
calendar. In the calendar views you can see it already when you look past the
next change.

From this version the editor keeps the zone, and an appointment that becomes a
series gets your device's time zone. If your device reports UTC, or a time zone
Aperio does not know, the series gets no time zone.

A series whose time zone is UTC under another name, such as "Etc/UTC" or "GMT",
counts as a series without a time zone; its times do not change. So does a time
zone Aperio does not know, such as the name of a Windows time zone or a bare
offset such as "+05:30". A series stored with such an offset now repeats in
UTC, and near midnight its appointments can fall on other days than before. All
of these series repeat in UTC, in the views and in the reminders alike, and are
an hour off after the clocks change, as described above.

A series that has already lost its zone is not changed on its own: to Aperio it
looks the same as a series that is meant to run in UTC. To fix an affected
series, create it again, or set its time zone again in the program it came
from.

## An Exchange appointment moved after it became a series

If you turned an appointment on an Exchange account into a series in Aperio,
changed the time zone of a stored Exchange series, or gave an all-day Exchange
series a time of day, the appointment usually moved by an hour or two
afterwards, and near midnight to another day. An appointment created in Aperio
for Monday at 00:30, for example, then stood on Sundays at 23:30.

When its time zone changes, Exchange keeps the stored time of day and reads it
in the new zone. From this version Aperio sends the time zone first and the
start and end after it, so the appointment stays where it was.

What you can do: move a series that has already moved back to its place once,
by hand.

## The day of an all-day Exchange appointment cannot be changed

Aperio then says: "Exchange stores this appointment in a time zone Aperio
cannot read, so its day cannot be changed here; its title and the rest can.
Nothing was changed."

Exchange places an all-day appointment on the midnights of the time zone it
stores it in. So that the appointment stays one day long, Aperio writes a new
day as midnight in exactly that time zone. Usually Aperio knows it: a Windows
time zone such as "W. Europe Standard Time" or a name such as "Europe/Berlin".
A custom time zone, such as "Customized Time Zone" on appointments from other
programs, Aperio can read when Exchange sends its rules along. When Exchange
sends only a name Aperio does not know, without its rules, Aperio does not
guess: the appointment would otherwise stretch over two days in Outlook.

What you can do: change the day in Outlook. Everything else, such as the
title, the place or the reminder, you can still change in Aperio, as long as
you do not move the day with it.

A similar message is: "Aperio could not read this appointment's current state
from the server, so it did not change it." To know the time zone,
Aperio reads the appointment from the server before saving. When that fails,
for instance because the server is not answering right now, Aperio sends
nothing. Try again in a moment.

## Reporting a bug

1. In Settings → Logs, set the level to **Debug**.
2. Reproduce the problem.
3. **Export** the log (or copy it) and attach it to your report, together with
   what you did and what you expected.
