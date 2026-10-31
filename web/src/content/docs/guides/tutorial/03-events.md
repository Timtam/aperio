---
title: "03 – Events"
---

In this chapter you create, edit and move events and set up recurrences.

## Creating an event

1. Switch to a calendar view (e.g. **Week**, see
   [Chapter 06](/guides/tutorial/06-views/)).
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
- a **reminder** (see [Chapter 07](/guides/tutorial/07-notifications/)),
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

> **Notify attendees:** When you organize an event, someone else is invited,
> and the calendar supports server-side scheduling (iCloud, Google,
> Exchange/Outlook, Microsoft 365), the provider can tell the attendees –
> Aperio itself never sends email. On Google and Exchange/Outlook a **Notify
> attendees** checkbox appears (on by default); when ticked, the provider
> sends invitations or updates on save. iCloud and Microsoft 365 inform the
> attendees of every change on their own and cannot save one silently, so
> instead of the checkbox the dialog says so, for example "iCloud informs the
> attendees of every change". That sentence is a stop Tab reaches, and it is
> announced when it appears while you edit, say when you add the first
> attendee.
>
> The organizer never counts as an attendee: an appointment you made in
> Outlook, which Outlook lists with you as its only attendee, has no
> attendees in Aperio. For a meeting someone else organizes, only they can
> send updates, so neither the checkbox nor the sentence appears there. If
> you remove the last attendee, the checkbox or the sentence stays, so the
> person you removed can get a cancellation. When you only change the title
> or the time, Aperio leaves the attendee list at the provider exactly as it
> is, their answers included.
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
> notify/silent choice. On iCloud and Microsoft 365 deleting always sends the
> cancellation, so there is no radio group: the dialog says who informs the
> attendees, and every scope cancels. A meeting you were only invited to, or
> an event with no attendees, is deleted without asking.

> **An invitation somebody else organizes:** On iCloud — and on any calendar
> server that schedules for you — the server takes only your own answer and
> your own reminders for a meeting you were invited to. Everything else it
> refuses, so Aperio shows such a meeting read-only: the title, the calendar,
> the times, the place, the description and the guest list are all there to
> read, each one a stop your screen reader reaches, but none of them can be
> changed. The dialog says so in a line above the fields. The description
> is a document of its own: on the desktop NVDA switches to browse mode when
> you reach it, so the arrow keys read it line by line, and `Tab` leaves it
> for the next field.
>
> What you can still do: answer (Accept, Tentative, Decline), set your own
> reminders — attached ones the calendar keeps, or private ones only Aperio
> rings — choose a sound, give it a colour, join a conference, check the
> attendees' availability, and delete it. Answering no longer closes the
> editor, so you can answer and set a reminder in one visit.
>
> Deleting it tells the organizer: the dialog says "The organizer gets a
> decline" before it happens, and the button is called **Delete and decline**.
> Skipping a single occurrence says the same sentence, above the usual scope
> buttons. Ending somebody else's series early is not offered,
> because the server would refuse it. Dragging such a meeting to another day
> or time is refused too, and Aperio says so; moving it to another calendar
> still works, and the copy has no attendees.
>
> The repeat rule is read out as a sentence — "every Monday, 5 times" — since
> there are no controls to read. Aperio says plainly when a rule is one it
> cannot put into words.

> **Check availability:** When an event has attendees on a calendar with
> server-side scheduling, a **Check availability** button appears, below
> **Notify attendees** when that is shown, and also for a meeting someone
> else organizes. It looks up, for the currently entered time window, which
> attendees are **free** or **busy**, and shows the result per attendee
> with a summary (announced via the live region). If a provider can't
> answer (missing permission), that attendee reads as "free/unknown".

> **Responding to invitations (RSVP):** When you open a meeting you were
> invited to (iCloud, Google, Exchange/Outlook), **Your response** appears
> at the top of the dialog with **Accept**, **Tentative** and **Decline**
> buttons — your current reply is highlighted. Your answer is sent to the
> organizer automatically. If you are the organizer, you instead see each
> attendee's response status; the organizer is not among them.

## Editing, moving and deleting events

- **Edit:** Select the event and open it with `Enter`, **double-click** it,
  or choose **Edit** from the context menu.
- **Move:** Change the times in the dialog – this works reliably and is
  screen-reader friendly. With a mouse you can also drag an event onto a
  **different day** in the week or month view, which keeps its time of day
  and duration, or into the hour grid of the day or week view, which gives it
  the time you drop it on. Dragged onto a **calendar in the sidebar**, it moves
  to that calendar. For recurring events Aperio asks whether to move just this
  occurrence or the whole series. The whole series moves every occurrence by
  the same number of days, so "every Monday" becomes "every Tuesday" and the
  end date moves along. Some rules cannot move that way, such as "the second
  Sunday of every month" or a day after the 28th; Aperio then says why and
  offers to move just this occurrence. See also **Single changed or deleted
  occurrences** below.
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
The new series repeats the way the repeat field says: change the rule there and
the new series follows it, leave it alone and the pattern simply continues.
Move the occurrence to another day and the repeat field moves with it right
away: every Monday becomes every Tuesday, every 10th of the month becomes every
11th, and an end date moves along so that the new series ends on the occurrence
in the place of the old last one. Change only something else, and the repeat
stays as it is. Some rules cannot move by days without meaning something else,
such as the second Tuesday of the month or every 30th, or a rule that names its
own times when the time changes: the field keeps the rule and says why, and
saving says so too. Choose the repeat yourself then, or edit just this
occurrence.
When nothing comes before the chosen occurrence — you picked the first one, or
every earlier one was deleted — there is nothing to keep: an edit changes the
whole series, which stays the same entry, and a delete removes the series.
Aperio says so when it happens. On an edit, Aperio first creates the new series
and then ends the old one before it. If ending the old one fails, Aperio deletes
the new one again, and nothing has changed. When it cannot tell whether the old
one was already ended, for example after a lost connection, both stay: your
change counts as saved, and the editor stays open with a note naming the day
from which the series may show twice, until you close it. If deleting the new
one fails as well, Aperio says so too; that note only closes, so that you check
the calendar instead of saving again. Aperio leaves the occurrences you deleted out of the new series,
and ones changed in another calendar app stay in it, at their usual time and
with the new series' content. Deleted occurrences follow your change: move the
occurrence to another day and each deleted one moves with it by its place in
the series, so a deleted third Monday becomes a deleted third Tuesday; switch
between all-day and a time of day and it stays on the day you saw it on. Give
the new series another repeat rule, and a deleted occurrence stays deleted only
on a day the new rule still has; a new end alone is no new rule, and neither is
setting by hand the rule the new date gives anyway. A monthly or yearly series that the new date makes skip
other months or years, such as one moved from the 31st to the 30th, moves its
deletions by their place only as far as the two versions still line up; if a
deletion lies beyond that, they all stay on their day, where a deleted
occurrence can show again. CalDAV calendars keep those deletions, and so do
Exchange and Google from today on: Aperio creates the new series there without
them and then removes them one by one (see the box below); the ones before
today show again, also in a series you split at an earlier occurrence. Aperio
knows a Google calendar's deleted occurrences only about a year ahead, so one
deleted further out comes back in the new series. And in an
all-day series on Google or CalDAV, an occurrence changed in another app also
stays in the old series for now, so that day shows twice.
**The whole series** opens the series itself, with its own start and end, even
when you opened it from a later occurrence. A new time applies to every
occurrence. A new date moves the series the way **this and all following** from
its first occurrence would: every Monday becomes every Tuesday, deleted
occurrences move with it by their place, and an end date moves so the series
stays as long as it was. The repeat field shows the moved rule right away, and
a rule that cannot move says so there and when you save. Switching the whole
series between all-day and a time of day keeps a deleted occurrence on the day
you saw it on, and an all-day series that gets a time of day gets your
device's time zone with it. When you carry such a change to the other copies
of the appointment, each copy that repeats moves the same way. The editor's title
names your choice, such as **Edit this occurrence only**, and is read out when
the editor opens. On the desktop the form repeats it as the read-only field
**Apply to**, which `Tab` reaches like every other field.

> **Single changed or deleted occurrences:** When a whole series moves to
> another day or time, by dragging it or in the dialog, some single occurrences
> stay tied to their old date: occurrences changed on their own in an external
> calendar (iCloud, Google), and occurrences deleted in a Google calendar, also
> when you deleted them in Aperio. After the move a deleted occurrence can come
> back, a changed one can show up twice, and another occurrence can be missing
> where the old one was. Aperio cannot repair this. Check the series afterwards
> in the calendar's own app, where such single changes can be undone. On
> Exchange, Aperio asks first, in the editor and when you drag the series with
> the mouse: Exchange drops every occurrence changed on its own when the series
> moves, so it takes the series' details again, and deleted occurrences Aperio
> deletes again itself afterwards.

> **A series in another calendar, copied or duplicated:** When you move a
> whole series to another calendar, copy it, duplicate it with `Ctrl+D` or give
> it another calendar in the editor, Aperio writes it there as a new series. It
> takes along the occurrences deleted from the old one, and nothing else: an
> occurrence changed on its own, for example in Outlook, is back at its usual
> time with the series' details. `Ctrl+D` on any appointment of a series
> duplicates the whole series from its first appointment, in the same calendar.
> A copy or a duplicate keeps the series' time zone. Moved or copied into
> Exchange or Google, or duplicated there, the new series keeps the deletions
> from today on (see the next box); into Microsoft 365 it shows deleted
> occurrences again for now. From a Google calendar, Aperio knows deleted occurrences from about
> three months before today to a year after (see
> [Troubleshooting & Logs](/guides/troubleshooting/#an-occurrence-is-back-after-a-series-was-moved-copied-or-duplicated)).

> **A new series on Exchange or Google, and the wait:** Exchange takes a new
> series without its deleted occurrences, and Google deletes an occurrence by
> cancelling it, so Aperio creates the series first and then removes them there
> one by one, from today on, and checks the result. On Exchange it invites
> only after that check; on Google the invitees get their invitation with the
> series and, when you notify them, a cancellation for each deleted occurrence
> from today on. That can take up to half a minute. Meanwhile Aperio shows
> the dialog **Working, please wait …**, on the desktop and in the mobile app,
> with no button; `Escape` and the back button do not close it, and it closes
> by itself when the work is done. If the deletions cannot all be made, Aperio
> removes the new series again and says nothing was changed. If even that
> fails, it says the new series may now stand in the calendar as well; that
> message only closes, so that you check the calendar instead of saving once
> more (see
> [Troubleshooting & Logs](/guides/troubleshooting/#a-new-series-may-stand-in-the-calendar-as-well)).

> **Changing an occurrence that was already changed:** An occurrence changed
> on its own in the calendar's own app (iCloud, Google, Exchange) stays part of
> its series when you change or move only this occurrence again: in the event
> dialog on the desktop and in the mobile app, and by dragging it on the
> desktop. An occurrence you changed on its own in Aperio already is an
> appointment of its own. Exchange is the exception: there such an occurrence may not move onto or
> past another occurrence of the same series. Aperio then makes it an
> appointment of its own at the new time, as it does with any other occurrence
> you move on its own.

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
Half-carried is the one state you have to see. A copy whose series may now show
twice from the cut is different: its new part was created, so it is not offered
again, and a note names its calendar and the day instead. When only such notes
are left, the dialog stays open with them and without **Try the rest again**.

On Exchange, carrying a change to a copy's whole series can make Exchange drop
that series' occurrences changed or deleted on their own, as in the editor.
Aperio then carries the change to every other copy first and asks once
afterwards, naming each such copy by its calendar and what it would lose:
**Carry over anyway** carries the change to them too, **Cancel** leaves them
among the copies not changed.

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
there on is named rather than quietly skipped, and a copy with nothing before
that point is changed as a whole instead of split.

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

## Importing appointments from a file

An appointment someone sent you as a `.ics` file — an invitation from Outlook,
Teams or Google, an export from another calendar — comes into Aperio from the
toolbar's **Import …** (or `Ctrl+O`), or by dropping the file onto the window.
On the desktop, Aperio registers for no file type; it imports only while it runs. On
the phone, **Import appointments …** in the calendar list (next to **New
calendar**) opens the file picker; a `.ics` file can also be shared to Aperio
or opened in it from another app (Mail, Files), where Aperio is offered
without becoming the default. In the list of several, each row is a
checkbox; **Edit …** is one of its actions (on iOS in the rotor).

- **One appointment** opens the editor, filled in from the file: its day and
  time, its place and description, its reminders, a series with the
  occurrences it skips. The **Calendar** field is where it goes; nothing is
  written until you press **Create**.
- **Several** open a list. Each one can be opened in the editor, or every
  checked one written to one calendar at once.
- **An appointment the calendar already holds** — the same one, imported
  before or sent twice — is found by the identity the file gives it, in a
  local, iCloud or other CalDAV, or Exchange calendar. The list names it and
  leaves it unchecked; checked again, it is imported as a copy. The editor
  says so before you create it, and creating it then makes a copy. Aperio
  asks the calendar you picked, and asks again when you pick another. Google
  (except an invitation it keeps in your primary calendar), Microsoft 365 and
  the phone's own calendars keep no such identity, and a changed occurrence
  imported as its own appointment has none: those are not recognised, come
  in again, and the list and the editor say so for such a calendar.
- **What the file holds besides appointments** — tasks, notes, a
  cancellation, an answer to an invitation — is named and left out.
- **A changed occurrence of a series** becomes an appointment of its own, and
  the series skips the slot it took.
- **An invitation someone else organizes** stays theirs in a calendar that
  can keep it without telling anyone: its organizer and attendees stay, nobody
  is notified, and the editor shows it as you received it — title, time,
  place, description and attendees as sent; the calendar, the reminders and
  the colour are yours to choose. Local, iCloud and other CalDAV calendars
  keep invitations so, and Exchange too: there your answer then reads
  “tentative”, without anyone being told, and you can accept or decline as
  with any invitation. In an Exchange calendar other than your main one,
  Exchange first puts it in the main calendar, and Aperio then moves it to
  the one you picked. Google keeps them in your primary calendar, not in
  your other Google calendars. In a calendar that cannot, it becomes your own
  appointment, without attendees. The editor and the list say which, for the calendar you
  picked.
- **A meeting you organize yourself** comes in as a new appointment, with
  its attendees, as if you had just made it. iCloud and other CalDAV
  calendars write attendees only along with their invitations, so the list,
  which tells nobody, brings such a meeting there without them. In the editor
  they stay, and are invited.
- **When not every one arrives**, the list says how many did, and names each
  one that did not with its reason; a busy provider's ends with “try again
  later”, and those stay checked. One that arrived only in part — a series
  whose skipped occurrences could not all be removed, or an invitation Exchange
  kept but did not finish — is marked “imported only in part” and not offered
  again: importing it once more would make it twice. Check it in the calendar.
  A series a calendar writes in steps shows the “please wait” dialog meanwhile.

Times keep their zone, including the Windows zone names Outlook writes. A
zone only the file itself defines (Outlook's "Customized Time Zone") gives
the right times, but for now a series in it repeats in UTC, an hour off after
a clock change.

## Time steps

Under **Settings → General**, **Time steps** decides how far one press of the
arrow keys moves a time field: 1, 5, 10, 15 or 30 minutes (15 by default). A
time that does not sit on that grid keeps stepping minute by minute, so nothing
you already saved becomes unsavable, and typing an exact time always works.

On the phone the same setting works differently, because the platform gives no
choice: the native picker's wheel always moves a minute at a time. There, the
setting decides which minutes the **Minutes** button beside a time field
offers — one tap instead of thirty swipes to reach half past.

## Signatures

A signature is a named block of text that goes at the **end** of a description
— a room's join details, a standing note, a department's dial-in. Under
**Settings → Signatures** — on the phone via **Signatures** in Settings — you
write them. WHICH one a calendar carries is set on the calendar itself, beside
its default reminders, and a calendar that has one puts it on **new
appointments by itself**: no press at all in the ordinary case. An
appointment imported from a file gets none by itself: it keeps the
description it came with. On your own appointment from a file, the
**Signature** button still adds one.

The binding is a default, not a restriction. Every signature stays pickable in
the editor whichever calendar the appointment is on, and the **Signature**
button beside the description is there for those exceptions — including taking
one back out. Change the calendar and the block is swapped for the new
calendar's; text you wrote yourself, or a block you deleted, is left alone.

Inserting is repeatable: a second press replaces the block rather than adding
another, and switching to a different signature swaps it. The text you wrote
above it is never touched — a signature is an addition at the end, not a
rewrite. It is separated by a line containing `-- `, the same marker mail
clients use, which is also how Aperio finds its own block again.

**Plain text only, and that is not a limitation we chose.** A sent invitation
travels as iCalendar, whose description field is defined as plain text — HTML
put there arrives as visible tags in every client that renders it literally.
Put a link on a line of its own and virtually every client will make it
clickable, which is all the formatting an invitation needs.

## Summary

You can create, edit, move, delete and repeat events. Next we'll handle
tasks.
