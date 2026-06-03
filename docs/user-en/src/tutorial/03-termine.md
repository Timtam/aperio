# 03 – Events

In this chapter you create, edit and move events and set up recurrences.

## Creating an event

1. Switch to a calendar view (e.g. **Week**, see
   [Chapter 05](05-ansichten.md)).
2. Use the arrow keys to navigate to the day or time you want.
3. Create an event with the **New event** command (default: `Ctrl+N`, or via
   the context menu). The currently selected time is suggested as the start.
4. In the dialog, enter at least a **title**.

In the event dialog you can also set:

- **Start and end** (or **All day**),
- the **calendar** the event is stored in,
- **location** and **description**,
- a **color label** (with a color dot in the picker),
- a **reminder** (see [Chapter 06](06-benachrichtigungen.md)),
- **attendees** (name and/or email address),
- a **recurrence** (see below).

**Save** creates the event; a live region confirms "Event saved".

> **Notify attendees:** When an event has attendees and the calendar
> supports server-side scheduling (iCloud, Google, Exchange/Outlook), a
> **Notify attendees** checkbox appears (on by default). When ticked, the
> provider sends invitations or updates automatically on save – Aperio
> itself never sends email. Deleting an event that has attendees likewise
> triggers a cancellation.

## Editing, moving and deleting events

- **Edit:** Select the event and open it with `Enter` (or **Edit** from the
  context menu).
- **Move:** Change the times in the dialog – this works reliably and is
  screen-reader friendly. With a mouse you can also drag events.
- **Delete:** Select the event and choose **Delete** (default: `Delete`).
  You are asked to confirm before deletion.

## Recurring events

Under **Recurrence** in the event dialog you choose a pattern:

- daily, weekly (with weekdays), monthly, yearly,
- an **end** (never, after X times, until a date).

When editing or deleting a recurring event, Aperio asks whether the change
should apply to **only this event**, **this and all following** or **all**.

> **Tip:** Recurring events from external calendars (e.g. iCloud) expand
> correctly in every view – even when the first occurrence lies in the past.

> **Screen-reader note:** When you create an event, focus moves to the
> dialog's title field. Use `Tab`/`Shift+Tab` to move through the fields;
> `Esc` cancels without saving. In a view, events are announced with title,
> time and calendar when selected.

## Summary

You can create, edit, move, delete and repeat events. Next we'll handle
tasks.
