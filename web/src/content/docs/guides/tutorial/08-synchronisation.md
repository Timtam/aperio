---
title: "08 – Synchronization"
---

Accounts such as Google or iCloud are synced directly with the respective
provider. **Local** calendars and task lists can additionally be
synchronized across multiple devices through your **own storage**. That's
what this chapter is about.

## How synchronization works

Aperio stores your local data as a change log and reconciles it through a
storage location that **you** choose – for example WebDAV, Dropbox or
another folder shared between your devices. There is no Aperio server; your
data stays with you.

## Setting up synchronization

In Aperio a storage location is an **account** like any other: you add it
under **Settings → Accounts**, and the synchronization settings only ask
which of your accounts holds the dataset.

1. Open the **settings** and go to **Accounts**.
2. Add an account for your storage location (e.g. WebDAV, SFTP, FTPS,
   Dropbox, Google Drive, or a local/shared folder) and enter its address
   and credentials.
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
