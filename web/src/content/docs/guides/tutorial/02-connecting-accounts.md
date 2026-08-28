---
title: "02 – Connecting Calendars and Task Lists"
---

Aperio shows several sources at once. In this chapter you connect accounts
and, if you like, create local calendars.

## Adding an account

1. Open the **settings** and go to the **Accounts** section.
2. Choose **Add account** and pick the right provider.
3. Follow the provider-specific flow (see below).

## The providers at a glance

| Provider | Sign-in | What you get |
|---|---|---|
| **Google** | Browser sign-in (OAuth) | Calendars, tasks, contacts |
| **iCloud / CalDAV** | Username + **app-specific password** | Calendars, tasks, contacts |
| **Outlook / Microsoft 365** | Browser sign-in (OAuth) | Calendars, tasks, contacts |
| **Exchange (EWS)** | Username + password | Calendars, tasks, contacts |
| **Vikunja** | API token + server address | Tasks only |
| **Todoist** | API token | Tasks only |

> **Google:** For now this requires your own **OAuth credentials** (client
> ID + client secret) from the Google Cloud Console, created once — Aperio
> does not ship a verified Google registration yet. The detailed
> step-by-step guide lives at
> [Connecting Google (OAuth guide)](/guides/google-oauth/).

> **iCloud:** Apple requires an **app-specific password** (create one in
> your Apple account under "Sign-In & Security"), **not** your regular Apple
> password.

> **Contacts-only (CardDAV):** A CardDAV-only server with no calendars
> (e.g. Synology Contacts) can also be added under **iCloud / CalDAV** —
> just enter its server address. Aperio detects that it only has contacts
> and leaves the calendar/task sections empty.

> **Vikunja / Todoist:** Create the API token in the developer or
> integration settings of the respective service and paste it here.

## Creating a local calendar

You don't need an account to get started. Local calendars and task lists
live only on your device (and, if set up, are reconciled through your own
[synchronization](/guides/tutorial/09-synchronization/)):

1. At the bottom of the **sidebar**, click the matching add button:
   **+ New calendar**, **+ New task list** or **+ New address book**.
2. In the dialog, enter a **name** and, optionally, a **color** (color
   label), then confirm.

> **Colors come from color labels:** The color of a calendar or list is
> bound to a color label. If you later recolor a label, every calendar bound
> to it changes too. You manage color labels in the settings.

## Managing multiple accounts

You can connect as many accounts as you like at the same time – even
several from the same provider (e.g. two Google accounts). Each source
appears in the sidebar with its own name and color and can be shown or
hidden individually.

### Editing an account

Server URL, endpoint, username, password or token can all be changed later
without re-adding the account: in **Settings → Accounts**, select the entry
and choose **Edit** (on the phone it is also in the row's actions). The form
is the same one the account was added with, prefilled with the stored
values — **password and token fields come back empty, and empty means "keep
the stored one"**. Where the provider supports it, **Test connection**
probes the new values (using the stored credential if you left the field
blank) before you save. One limit: an optional stored password cannot be
**removed** through an edit — blank always keeps it; to shed one entirely,
delete and re-add the account.

The changes synchronize: the new configuration reaches your other devices
with the next sync round, and a changed password or token travels too when
**end-to-end encryption** is enabled (see the synchronization chapter) — the
other devices switch over without any re-entry. The sign-in identity of a Google/Microsoft/Webex account (the
OAuth client) is not edited here; use **Reconnect** for that.

### Renaming an account

You can change an account's display name at any time – either in
**Settings → Accounts** (select the entry and press **F2**) or via the
**account row's context menu in the sidebar** (Application key → "Rename").
The new name syncs across your devices; the local account can be renamed
this way too.

> **Screen-reader note:** In the sidebar, accounts, categories (calendars /
> tasks / contacts) and the individual lists form a tree. Move up and down
> with the arrow keys; expand and collapse levels with left and right. Reach
> the context menu (Rename, Color, Members, Delete) with the Application
> key. The add buttons (**+ New calendar** etc.) sit below the tree and are
> reachable with `Tab`.

## Summary

You have connected accounts and/or created local lists. Now let's create
events.
