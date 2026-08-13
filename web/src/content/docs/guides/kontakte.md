---
title: "Contacts"
---

Aperio keeps an address book beside your calendars and task lists. It is the
same set of accounts: an iCloud or Nextcloud account brings its CardDAV
address books, a Google or Microsoft account brings its contacts, an Exchange
account brings yours plus the company directory — and there is a **local
address book** that needs no account at all.

Contacts are what the attendee picker offers when you invite people to an
event, and what the **birthday calendars** are built from.

## Finding your way around

Open **Contacts** from the sidebar (on the phone: the **Contacts** tab). The
list is grouped by address book, with the number of entries in each group
heading.

- `Enter` opens the selected contact for editing.
- `Insert` creates a new one.
- `Delete` removes the selected contact (with a confirmation step).
- The Menu key or `Shift+F10` opens its context menu.
- **Search** covers your own address books *and* connected directories such as
  the Global Address List — including people who are not in any book of yours.

On the phone the address books are **collapsed by default**: you get a short
list of book headings and expand only the one you want, so a company directory
with thousands of entries does not bury your personal book. Each contact row
carries **Edit** and **Delete** as accessibility actions on the row itself.

> **Read-only books.** Directories — the Global Address List, Google Directory
> and Other Contacts, Microsoft's Suggested People — are read-only by nature.
> Their entries open in a view-only editor: you can read every field, but
> Save and Delete are gone.

## What a contact holds

- **Display name** (required), first name, last name.
- **Organization**, **job title** and **department**.
- **Email addresses**, **phone numbers** and **websites** — any number of each,
  every one with its own label (see below).
- **Postal addresses** — street, postal code, city, region, country, each with
  a label.
- **Birthday** and **anniversary**.
- **Notes**.
- A **photo**, which you can set from a file (on the phone from the photo
  library) and remove again.

A contact can also be a **distribution list**: tick *This is a distribution
list (group)* and the person fields give way to a member editor — one member
per line, either `Name <email@example.com>` or a bare email address.

## Labels: which phone number is which

An email address, a phone number or a website is never just the value. Each one
sits in its own row with a **Label** in front of it:

**Home · Work · Mobile · Fax · Other · No label · Custom…**

Pick **Custom…** and a free-text field appears, so a number can be called
*Holiday home* or *Reception* if that is what it is.

On the desktop the label is a dropdown at the top of the row. On the phone it
is a **button** that opens the choice in a dialog; the button says what is
currently chosen, so you hear "Phone number 2, label: mobile" without opening
anything.

Add a row with **Add phone number** (or *Add email address* / *Add website*),
remove one with the **Remove** button at the end of its row.

> **Screen-reader note:** Each row is a small group of its own: the label
> button or dropdown, then the value, then Remove. The label always comes
> before the value — walking down a contact with four numbers, you hear which
> one it is before you hear the digits. The row number is part of every
> control's name ("Remove phone number 2"), so you always know where you are.

### What each provider does with a label

Every system files a phone number under some kind of label, but they do not
agree on how. Aperio stores your word and each account translates it on the
way out:

- **CardDAV / iCloud / Nextcloud** and **Google** keep whatever you typed.
  A custom label like *Holiday home* survives a round trip unchanged.
- **Exchange** has a fixed vocabulary and a fixed number of slots — four phone
  numbers and three email addresses per contact. A label it has no word for
  takes the next free slot: **the number always travels, only the word may be
  replaced**. A fifth number cannot be stored at all.
- **Outlook / Microsoft 365** has three phone collections — one mobile number,
  home numbers, business numbers. A second number labelled *mobile* joins the
  business list rather than pushing the first one out.
- The **local address book** stores everything exactly as you typed it.

The same applies to websites: CardDAV and Google keep as many as you like,
Exchange and Outlook keep exactly one (a *work*-labelled one is preferred, and
otherwise the first).

> **Anniversary on Outlook.** Microsoft 365 contacts have a birthday field but
> **no anniversary field**. An anniversary you enter on an Outlook contact has
> nowhere to be stored and will be empty again after the next sync. Every other
> account type keeps it.

## Birthdays in the calendar

Every address book that has birthdays in it also appears as a **read-only
birthday calendar** in the calendar list, which you can show or hide like any
other calendar. Those entries are derived from the contacts themselves — there
is nothing to edit there; change the birthday on the contact and the calendar
follows.

You can give a birthday calendar its own **default reminders**, so you are
warned a few days ahead rather than on the morning itself.

## Sync and privacy

Under **Settings → Contacts** you control:

- the **sync interval**, and a **Sync now** button;
- whether **read-only directories** (Global Address List, Suggested People,
  Other Contacts, Workspace Directory) are pulled on every sync. They are
  skipped by default because they can be very large — search reaches them
  either way, on demand;
- **Clear cache**, which drops the in-memory snapshots of external address
  books. Your own local contacts are untouched; the next sync re-pulls the
  rest.

Aperio syncs your contacts **directly with the connected providers** and keeps
names, addresses, numbers, birthdays and organisation fields in memory so that
search and the attendee picker stay responsive. What a provider itself collects
and how long it keeps it is governed by that provider's own privacy policy; the
settings page links to Google's and Microsoft's, and for CardDAV, iCloud and
Exchange servers the policy of that server applies.
