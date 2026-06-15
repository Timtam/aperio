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

## Reporting a bug

1. In Settings → Logs, set the level to **Debug**.
2. Reproduce the problem.
3. **Export** the log (or copy it) and attach it to your report, together with
   what you did and what you expected.
