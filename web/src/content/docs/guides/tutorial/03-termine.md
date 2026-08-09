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
of that day; pick the twin and confirm with **Group**. The same dialog takes an
event back out (**Take this event out**) or drops the whole grouping
(**Dissolve group**).

Nothing reaches the provider. Grouping two events changes neither of them, and
ungrouping leaves both exactly as they were — the calendars keep their own
copies, Aperio just knows they are one appointment. The grouping travels
between your devices with everything else.

If both events already belong to *different* groups, Aperio refuses rather than
guessing: merging two claims about what an appointment is would be a decision
you never asked for. Take one of them out first.

## Summary

You can create, edit, move, delete and repeat events. Next we'll handle
tasks.
