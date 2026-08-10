---
title: "03 – Events"
---

In this chapter you create, edit and move events and set up recurrences.

## Creating an event

1. Switch to a calendar view (e.g. **Week**, see
   [Chapter 05](/guides/tutorial/05-ansichten/)).
2. Use the arrow keys to navigate to the day or time you want.
3. Create an event: **Quick add event** (`Ctrl+N`) opens the quick dialog,
   **New event** (`Ctrl+Shift+N`) the full form — or via the context menu.
   The currently selected time is suggested as the start.
4. In the dialog, enter at least a **title**.

In the event dialog you can also set:

- **Start and end** (or **All day**),
- the **calendar** the event is stored in,
- **location** and **description**,
- a **color label** (with a color dot in the picker),
- a **reminder** (see [Chapter 06](/guides/tutorial/06-benachrichtigungen/)),
- **attendees** (name and/or email address),
- a **recurrence** (see below).

> **Start and end are linked:** moving the **start** slides the **end** by the
> same amount, so the duration is preserved — across midnight and across
> several days too. Changing the **end** only resizes the event; an end before
> the start is pulled back to the start. New events begin at the next half hour
> (09:00 on another day) and run for one hour. This is identical on the desktop
> and in the mobile app.

> **Custom color:** Next to the color-label picker there's a **"Custom
> color…"** button. Use it to compose an arbitrary color on the fly (hex
> value or swatch) and apply it directly — no detour through the settings.
> You can optionally save it as a named label in your palette at the same
> time. The sidebar offers the same thing via the **Color → Other…** context
> menu on calendars and lists.

> **Recoloring a single event:** Besides the dialog, you can recolor an event
> directly from its **right-click menu** (the **Color** submenu) — handy for a
> quick tweak without opening the full form. Where the calendar's provider can
> store a per-event color (local calendars and color-capable CalDAV servers),
> the color travels with the event and shows up in other clients too. For
> iCloud, Google, Exchange/Outlook and subscribed feeds, the color is kept
> locally on this device instead (so it never triggers a sync error), and it
> stays applied just the same.

> **Subscribed calendars:** If a calendar you subscribe to (an iCal feed)
> sets its own per-event colors, those now show through as well — read-only,
> since a subscribed feed can't be edited.

**Save** creates the event; a live region confirms "Event saved".

> **Notify attendees:** When an event has attendees and the calendar
> supports server-side scheduling (iCloud, Google, Exchange/Outlook), a
> **Notify attendees** checkbox appears (on by default). When ticked, the
> provider sends invitations or updates automatically on save – Aperio
> itself never sends email.
>
> When you delete a **meeting you organize** (with attendees, on an account
> with server-side scheduling), Aperio asks in **one** dialog what should happen
> — no hidden second step. For a **recurring** meeting the dialog has a
> **Notify attendees / Remove without notifying** radio group (default: notify)
> and, below it, a button for each scope: **this occurrence**, **this and all
> following**, and **the whole series**. So you can cancel a single occurrence
> (attendees get a cancellation for exactly that date), end the series from a
> chosen date onward (**this and all following** keeps the earlier occurrences
> and drops this one plus every later one), or cancel all of it — and the radio
> decides in each case whether an email goes out. A single event has just the
> notify/silent choice. A meeting you were only invited to, or an event with no
> attendees, is deleted without asking. (On iCloud/CalDAV the server decides
> whether a cancellation goes out, so "without notifying" isn't guaranteed
> there.)

> **Check availability:** Below that toggle sits a **Check availability**
> button. It looks up, for the currently entered time window, which
> attendees are **free** or **busy**, and shows the result per attendee
> with a summary (announced via the live region). If a provider can't
> answer (missing permission), that attendee reads as "free/unknown".

> **Responding to invitations (RSVP):** When you open a meeting you were
> invited to (iCloud, Google, Exchange/Outlook), **Your response** appears
> at the top of the dialog with **Accept**, **Tentative** and **Decline**
> buttons — your current reply is highlighted. Your answer is sent to the
> organizer automatically. If you are the organizer, you instead see each
> attendee's response status.

## Editing, moving and deleting events

- **Edit:** Select the event and open it with `Enter`, **double-click** it,
  or choose **Edit** from the context menu.
- **Move:** Change the times in the dialog – this works reliably and is
  screen-reader friendly. With a mouse you can also drag an event onto a
  **different day** in the week or month view (time of day and duration
  are preserved) or onto a **calendar in the sidebar** to move it to that
  calendar. For recurring events Aperio asks whether to move just this
  occurrence or the whole series.
- **Delete:** Select the event and choose **Delete** (default: `Delete`).
  You are asked to confirm before deletion.

## Recurring events

Under **Recurrence** in the event dialog you choose a pattern:

- daily, weekly (with weekdays), monthly, yearly,
- an **end** (never, after X times, until a date).

When editing or deleting a recurring event, Aperio asks up front whether the
change should apply to **only this occurrence**, **this and all following**, or
the **whole series** — the same three scopes other calendars (Google, Outlook)
offer. **This and all following** splits the series at the chosen occurrence:
the earlier occurrences stay untouched, and this one plus every later one are
changed (on edit, a new series takes over from here) or removed (on delete).

> **Tip:** Recurring events from external calendars (e.g. iCloud) expand
> correctly in every view – even when the first occurrence lies in the past.

> **Screen-reader note:** When you create an event, focus moves to the
> dialog's title field. Use `Tab`/`Shift+Tab` to move through the fields;
> `Esc` cancels without saving. In a view, events are announced with title,
> time and calendar when selected.

## The same appointment in several calendars

One commitment often exists several times over: in the work calendar so
colleagues see it, copied into a private calendar because that is the one a
voice assistant reads out, and again in a colleague's calendar Aperio also
reads. To every provider those are unrelated events — Aperio can be told
otherwise.

Open an event's context menu (right-click, `Shift+F10`, or long-press on the
phone) and choose **Belongs together with…**. The dialog lists the other events
of that day; pick the twin and confirm with **Group**. The list reaches into
**switched-off calendars** as well — a colleague's calendar is often off
precisely because it is noisy, and that is where the third copy sits. Those
entries carry a "(calendar switched off)" note, so no event turns up out of
nowhere. The same dialog takes an
event back out (**Take this event out**) or drops the whole grouping
(**Dissolve group**). Its members are a **list**, and each one opens: you get
from the group straight into the editor of whichever copy you mean, and back
again. On an event that is already grouped the menu entry reads **Manage
grouping…**, so its name says there is something to manage.

Nothing reaches the provider. Grouping two events changes neither of them, and
ungrouping leaves both exactly as they were — the calendars keep their own
copies, Aperio just knows they are one appointment. The grouping travels
between your devices with everything else.

If the second copy is the obvious one — same name, same time, another calendar
— it is already picked when the dialog opens, with a line saying why. Confirming
is one keystroke; disagreeing is choosing something else. Aperio never groups
anything on its own: in an office full of "Team meeting" at 10:00 that would
declare two different meetings one appointment, and a wrong group hides a real
commitment behind a copy of something else.

If both events already belong to *different* groups, Aperio refuses rather than
guessing: merging two claims about what an appointment is would be a decision
you never asked for. Take one of them out first.

### What changes once events are grouped

**One row instead of four.** Every view shows the appointment once, and the row
says what it stands for: "one appointment with 2 others, in Work, Private". The
count is of the group, so a copy in a calendar you have switched off is counted
too — it matches what you know you keep.

You can see it too: a folded row carries a small mark — "3×" for the
appointment and its two other copies.

There is one exception, and it is deliberate: if the copies have drifted apart
— one was moved and the others were not — they are NOT folded. Each stays
visible and says so, with a highlighted mark ("3× ≠"). The group has stopped
being true, and that is the one thing you need to see.

**One edit instead of four.** After saving a change to a grouped event, Aperio
asks whether the other copies should follow, names each one it will write, and
names each one it may not. A colleague's calendar is read-only, and skipping it
quietly is how a group ends up meaning two different times. The copies are
named by their **calendar**: the title is the same on all of them — that is
what makes them a group.

If something goes wrong along the way, the dialog stays open and says which
calendars could not be written; **Try the rest again** retries exactly those.
Half-carried is the one state you have to see.

Only what the appointment IS travels: title, when, where, and the description.
Reminders stay with each copy — the private copy usually exists precisely
because it carries a reminder the work one does not. Colour, calendar and
attendees stay per copy for the same reason.

The question comes after the save, never before, so your own change is never at
stake and cancelling costs nothing.

**Series too, at every scope.** Change **this occurrence only** and each copy
gets what your event got: the occurrence cut out of its series and a standalone
event put in its place. Choose **this and all following** and each copy's series
is split at the same point — the earlier occurrences stay untouched, the later
ones carry the change. A copy running to a different pattern (fortnightly
against weekly) is split at its OWN next occurrence; a copy with none left from
there on is named rather than quietly skipped.

The dialog says which occurrences it is about. And because both scopes create
NEW entries, those are tied together afterwards — otherwise the appointment you
had just made one row would be four again from that point on.

**The meeting belongs to the appointment.** A meeting link hangs on exactly one
event, and which one is a coincidence of the moment it was attached. Inside a
group, **Join** appears on whichever copy you happen to be looking at.

**Copies are found again.** Event ids belong to the provider and change
underneath Aperio — a re-sync remints them, moving an event between calendars
remints it. A group remembers each member's name and start, so when an id stops
resolving the copy is looked for and the group repairs itself silently. If
nothing matching is there, nothing is changed: it may be a copy you deleted, and
dropping it on suspicion is not Aperio's call.

## Summary

You can create, edit, move, delete and repeat events. Next we'll handle
tasks.
