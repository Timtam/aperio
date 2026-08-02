---
title: "08 – Synchronization"
---

Accounts such as Google or iCloud are synced directly with the respective
provider. **Local** calendars and task lists can additionally be
synchronized across multiple devices through your **own storage**. That's
what this chapter is about.

## How synchronization works

Aperio stores your local data as a change log and reconciles it through a
storage location that **you** choose – for example WebDAV, Dropbox, your
Google account's Drive, or another folder shared between your devices. There is no Aperio server; your
data stays with you.

## Setting up synchronization

In Aperio a storage location is an **account** like any other: you add it
under **Settings → Accounts**, and the synchronization settings only ask
which of your accounts holds the dataset.

1. Open the **settings** and go to **Accounts**.
2. Add an account for your storage location (e.g. WebDAV, SFTP, FTPS or
   Dropbox) and enter its address and credentials. Two entries need no
   account of their own: **a folder on this device** is part of Aperio
   itself, and **Google Drive** rides on a Google account you may already
   have for your calendars. For either, skip this step and pick it in the
   next one. **Google Drive is not a separate entry**: a Google account
   can hold the dataset itself, so if you already have one for your
   calendars, skip this step and pick it in the next one.
3. Switch to **Synchronization**. Under **Sync target** you'll find exactly
   the accounts that can hold a dataset.
4. Pick the one you want in the list and choose **Sync through …**. Aperio
   probes the connection before the choice is saved — if it fails, nothing
   changes.
5. Set up the same storage on your second device. Both devices now share the
   same state.

On the **very first launch** the setup wizard asks for the storage location
directly — that is also where you decide whether to adopt an existing
dataset or start a new one.

If Aperio cannot start syncing through the chosen account after a restart —
a locked keychain, a credential that is no longer there, a server
fingerprint not confirmed on this device, or a missing plugin — the sync
target says so on that account and offers **Try … again** for it. Pressing
that names the actual reason and, where the repair is a confirmation, offers
it right there. **Disconnect** stays available as well.

Protocols that identify a server by its key — SFTP, for one — ask you to
confirm its fingerprint the first time, and refuse anything that does not
match afterwards. The confirmed fingerprint is shown on that account, with
**Forget pin** next to it. Drop it when you know the server's key has
changed for a legitimate reason, such as a reinstall: the next connection
then asks you to confirm the new one. The pin belongs to this device alone
and is never sent anywhere.

Which account a device uses is a **device-local** decision: the accounts
themselves travel between your devices, the choice of target does not. So a
laptop can reach the same dataset over the internet while a desktop reaches
it over a share on the local network.

## Reconciliation and conflicts

- Reconciliation happens automatically in the background and on startup.
- You can also trigger a reconciliation **manually**.
- If two devices edit the same entry, Aperio detects the **conflict** and
  resolves it in a comprehensible way, or asks which version should win.

## Your devices

Every device that connects to your dataset registers itself there. **Settings →
Synchronization → Devices** shows you the list and lets you keep it in order.

**Device name.** Enter what this device should be called on your other devices —
"Work PC", "Phone". Without a name it shows up as a long string that tells
nobody which device is meant. Aperio suggests the name your computer or phone
already goes by; it is only stored once you press **Save device name**. The name
belongs to this device alone and reaches the others on the next round.

**Last seen.** Each row says when that device last completed a round. Devices
that have never reported in from a version that records it read as "unknown" —
never as an invented date.

**Removing devices.** After a few reinstalls or test devices, the list holds
entries with no device behind them any more. Those you can remove. What happens,
and what does not:

- **No data is deleted.** Only the entry that counts this device as a
  participant goes.
- Old entries genuinely cost something: when Aperio cleans up, it keeps old log
  files until **every** registered device has read them. An entry with nothing
  behind it keeps them forever.
- You cannot really get this wrong. If the device is still running, it simply
  registers again on its next round.
- The device you are sitting at cannot be removed — it would register itself
  again immediately. To take this device out of syncing, **Disconnect** is the
  right route.

## End-to-end encryption & credentials

In the synchronization settings you can turn on **end-to-end encryption** with a
password. The storage then only ever sees encrypted data – the password never
leaves your device, and without it the data cannot be recovered (so keep it
somewhere safe).

When encryption is on, your **account credentials** (passwords, tokens) are
synced encrypted as well, so your accounts work on every device **without**
re-entering them. This also covers accounts you created **before** turning
encryption on — their credentials are backfilled automatically when you enable
it. **Without** encryption, credentials stay on each device only.
If you turn encryption back off, they are removed from the sync store and kept
locally only.

> **Note:** External accounts (Google, iCloud, Outlook, Vikunja, Todoist) do
> **not** need this setup – they synchronize through their own service. The
> synchronization described here only concerns your **local** calendars and
> lists.

> **Screen-reader note:** The status of the last reconciliation (time,
> success or error) is shown in the synchronization settings and is announced
> through a live region when it changes. Conflict dialogs are marked up as
> dialogs and are fully operable by keyboard.

## Summary

You can synchronize your local data across devices through your own storage
and know how conflicts are handled. To finish, let's look at keyboard
shortcuts.
