# 08 – Synchronization

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

1. Open the **settings** and go to **Synchronization**.
2. Choose the **storage type** (e.g. WebDAV or a local/shared folder) and
   enter the credentials or path.
3. **Save** – Aperio performs the first reconciliation.
4. Set up the same storage on your second device. Both devices now share the
   same state.

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
re-entering them. **Without** encryption, credentials stay on each device only.
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
