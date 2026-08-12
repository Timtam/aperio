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

A brief, one-off connection hiccup does not raise the warning: a network
failure is only shown once it recurs, so a cold start on a not-yet-ready
network never flashes a false alarm. A login problem — which never fixes
itself — shows straight away, and a manual refresh always reports its
result at once. The warning clears by itself as soon as an update
succeeds again.

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

## Reporting a bug

1. In Settings → Logs, set the level to **Debug**.
2. Reproduce the problem.
3. **Export** the log (or copy it) and attach it to your report, together with
   what you did and what you expected.
