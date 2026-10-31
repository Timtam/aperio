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

## An account stops updating

If a connected account can no longer be refreshed — most commonly because its
password or app password was changed or revoked — Aperio keeps showing the
last data it has and warns you instead of failing silently:

- **Desktop:** the account in the sidebar carries a warning, and a polite
  screen-reader announcement points you at **Settings → Accounts**. There the
  affected account lists each failing calendar or list, the provider's error,
  and when the last successful update happened. If the errors look like a
  login problem, a **Re-enter password** button opens the reconnect flow
  directly.
- **Mobile:** the sync button in the header turns into a warning (its label
  says some accounts are not updating), the details are on the **Sync**
  screen, and the affected account on the accounts screen gets a
  **Reconnect** button to re-enter the password or redo the provider sign-in.

The sentence that ends a refresh names what it could not update: "External
data updated, except: Work and Home." (on the phone after every refresh, on
the desktop after one you started). If the refresh could not read anything at
all, it says that nothing could be updated, and the warning with its cause
follows; a first network hiccup that is not confirmed yet only says that the
refresh ended. The accounts it names are not announced a second time, unless their
cause grows more severe, and their warnings appear together with the
sentence.

When the operating system withholds an account's data — the phone's own
calendars without access — the account shows one line instead of one row per
calendar ("Aperio may not read the calendars"), and on the phone a button that
opens the accounts, where **Allow access…** is. A withheld grant comes before
a login problem, a login problem before a busy provider, and a busy provider
before anything else, wherever Aperio names only one of them. An account that
is already failing is announced again only when its cause grows more severe.

When the provider is busy — it turns requests away because many arrived in a
short time — the warning says so, and that you need do nothing: on the
desktop Aperio tries again by itself, on the phone the next time it updates,
for instance when you open the app again. Its rows say "The provider is
busy." While it is busy, the views stop asking the provider for what they
already show; a week or month that was never loaded, or a list that never
loaded, is still asked for, once.

A brief, one-off connection hiccup does not raise the warning: a network
failure or a busy provider is only shown once it recurs in a second automatic
update at least a minute after the first, so a cold start on a not-yet-ready
network never flashes a false alarm, and the two or three updates that
opening the phone starts count once. A busy provider the views meet when they
ask on their own does not count; any other failure of theirs counts each time
it happens. A login problem — which never fixes itself — shows straight away,
and a refresh you start, or the change of an account, reports its result at
once for the accounts concerned, also in the first seconds after the app
starts. The warning clears by itself as soon as an update succeeds again.

## This device stops updating after a move to a new phone

Moving to a new iPhone brings Aperio's data along, the **This device**
account included, but not the permission to read the phone's own calendars
and reminders: iOS asks for that on each phone anew. Until it is given,
Aperio keeps showing what it last read, and the account does not update.

When the account exists and iOS has never asked on this phone, Aperio asks at
start, once the app is unlocked and the first screen has loaded. Allow full
access to the calendars and to the reminders; Aperio then updates the account
straight away and says so.

While access is missing, **This device** under Settings → Accounts carries a
badge that names what is missing (**No access**, **Calendars without access**
or **Reminders without access**) and the action **Allow access…**. If iOS has
not asked yet, it asks now. If you declined, iOS does not ask again: the
action then says what is missing and opens Aperio's page in the Settings app.
Set **Calendars** and **Reminders** to **Full Access** there (up to iOS 16:
turn them on). "Add events only" is not enough: Aperio has to read your
calendars. When you come back to Aperio, it updates the account and says so,
or says what access is still missing. If a restriction such as Screen Time
forbids access, Aperio says that too; only that restriction can change it.

On Android the same holds for the calendars. When the account exists and
Aperio has never asked on this phone — a new phone included — it asks once at
start. After that the action asks while Android still asks, and otherwise
leads to Aperio's permissions in the Android settings.

There is one **This device** account per phone. Choosing **This device**
again under Add account does not add a second one: while access is missing it
leads to the same **Allow access…**, and otherwise it says the account is
already added and moves to it. If **This device** shows twice (added twice by
an older version), every device calendar appears twice; delete one of the
two.

## Reminders don't appear on Windows

Windows shows notifications only from programs that have an entry in the
Start menu. Aperio runs without an installer and creates that entry only when
you ask. Until then Windows stores every reminder and shows none: no banner,
and nothing in the notification center.

What you can do: open **Settings, General, Windows notifications** and choose
**Set up notifications**. The next reminder appears; Aperio does not need a
restart. The same button is in the hint **Reminders are not showing**, which
Aperio opens once per start when a reminder came due while Windows would hide
it.

The section says where the entry stands:

- **Not set up.** There is no entry. Set it up.
- **Set up.** Windows can show the reminders. If they still do not appear,
  check Windows itself (below).
- **Not set up, although an entry named Aperio exists.** That entry was not
  made for notifications, for example one you made yourself. **Set up
  notifications** replaces it. **Remove Start menu entry** never touches it.
- **Set up for another copy of Aperio**, with the path of that copy. This
  happens after you moved Aperio's folder or started another copy. At the
  next start Aperio points the entry at itself; **Set up notifications** does
  it now.
- **A development build.** Started from a build folder, it shows its
  notifications as Windows PowerShell. There is nothing to set up.
- **Aperio could not check the Start menu entry**, with the error. Open the
  section again later. If the message stays, send the log with your report
  (see below).

If the entry is set up and reminders still do not appear, check Windows:

- In **Windows Settings, System, Notifications**, notifications must be on,
  and **Aperio** must not be switched off in the list of apps.
- **Do not disturb** holds back banners. The notifications then wait in the
  notification center.

The **Reminders overview** (`Ctrl+Shift+R`) cannot tell you whether a
notification was shown. It lists the reminders coming up, computed from your
events and tasks. It is not a record of the notifications Windows delivered.

## Reminders don't appear on the phone

The phone shows Aperio's reminders only if you allow it. The app asks on
first use; if you declined, the phone drops every reminder without a word.

Under **Settings, General, Notification sounds** a line says where the
permission stands:

- **Allowed.** The phone shows the reminders; how they appear is up to its
  system settings.
- **Not allowed yet.** The button **Allow notifications** asks the phone. If
  you allow it, Aperio schedules your reminders right away.
- **Not allowed, and only the system settings can allow it.** The button
  **Open system settings** opens Aperio's page there.
- **Aperio could not check** whether it may show notifications. The button
  then also opens the system settings.

The line is read again when you open the screen and when you come back to
the app, so a change made in the system settings shows on the way back.

## A task's time shifted, once

Tasks with a **time of day** on a **CalDAV** account (iCloud Reminders,
Nextcloud, Radicale, Tasks.org) used to be stored by Aperio as a UTC time, even
though the time is a local wall clock. Aperio never noticed, because it made the
same mistake in reverse when reading — but in every other program the task sat
at the wrong hour, off by your time-zone offset.

From this version the time is written as what it is. Tasks that **Aperio itself**
created with a time therefore shift **once**, by exactly that offset — a 09:00
task reads as 11:00 in central Europe. Correct it once and it stays put. Tasks
created in another program were wrong before and are right now.

Two more things are fixed with it: a task whose time carries a **time zone** (how
Thunderbird, Tasks.org and Nextcloud write it) did not merely lose its time here,
it lost its **whole day** and sat undated in the backlog. And a task from
**Microsoft To Do**, created by someone in their own time zone, could show up a
day early.

## A recurring appointment is an hour off after the clocks change

Saving a recurring appointment in Aperio's editor used to drop its time zone.
That affected every series with a time zone that was edited as a whole in
Aperio since June 2026, including series created elsewhere, for example on an
iPhone, in Outlook or in Google Calendar, and on local calendars as much as on
iCloud, Google, Microsoft 365 or Exchange. An appointment turned into a series
in the editor got no time zone at all.

Without its zone, a series keeps its time in UTC. After the clocks change it
shows an hour early or late, here and in every other program that reads the
calendar. In the calendar views you can see it already when you look past the
next change.

From this version the editor keeps the zone, and an appointment that becomes a
series gets your device's time zone. If your device reports UTC, or a time zone
Aperio does not know, the series gets no time zone.

A series whose time zone is UTC under another name, such as "Etc/UTC" or "GMT",
counts as a series without a time zone; its times do not change. So does a time
zone Aperio does not know, such as the name of a Windows time zone or a bare
offset such as "+05:30". A series stored with such an offset now repeats in
UTC, and near midnight its appointments can fall on other days than before. All
of these series repeat in UTC, in the views and in the reminders alike, and are
an hour off after the clocks change, as described above.

A series that has already lost its zone is not changed on its own: to Aperio it
looks the same as a series that is meant to run in UTC. To fix an affected
series, create it again, or set its time zone again in the program it came
from.

## An Exchange appointment moved after it became a series

If you turned an appointment on an Exchange account into a series in Aperio,
or gave an all-day Exchange series a time of day, the appointment usually
moved by an hour or two afterwards, and near midnight to another day. An appointment created in Aperio
for Monday at 00:30, for example, then stood on Sundays at 23:30.

When its time zone changes, Exchange keeps the stored time of day and reads it
in the new zone. From this version Aperio sends the time zone first and the
start and end after it, so the appointment stays where it was. When an all-day
series gets a time of day, Aperio sends the start and end before and after the
time zone and the repeat last: Exchange refuses the time zone first for a
daily series, and without the repeat the series would start a day later.

What you can do: move a series that has already moved back to its place once,
by hand.

## Aperio asks whether occurrences of a series may be lost

When you save an Exchange series, Aperio sometimes asks "Occurrences will be
lost", with a sentence such as: ""Team meeting": This change writes the series'
start and end again. Exchange then drops one occurrence that was changed on its
own: it takes the series' details again. Save anyway?"

Exchange drops every occurrence of a series that was changed or deleted on its
own as soon as Aperio writes the series' start and end, its time zone or its
repeat pattern again: when you move the series to another time or another day,
change how long it lasts, make it all-day, or choose other weekdays, another
interval or another frequency (daily instead of weekly). The title, the place
and the reminder change nothing about that, and neither, as a rule, do the
number of appointments and when the series ends; then Aperio does not ask. Only
on a series whose time zone Aperio writes again with its repeat (see below) do
they bring the question too.

Deleted occurrences Aperio deletes again itself after saving, where they now
stand. Only when you change the pattern and the time or the day together, or
when Aperio cannot read the series' time zone, can it not tell where they stand;
then the question counts them too. Occurrences changed on their own are lost:
they take the series' details again.

What you can do: "Cancel" leaves the series as it is, and your change stays in
the editor. "Save anyway" saves it. If a deleted occurrence could not be deleted
again afterwards, Aperio names its day; then delete it on its own.

In this version you cannot choose a series' time zone yourself. So Aperio asks
about another time zone only for a series from another program whose end is in
another time zone than its start, or when the series' time zone was changed
elsewhere since Aperio last read it; in the second case, open the series again
once Aperio has refreshed the calendar. On such a series, if something would be
lost — occurrences changed on their own, or deleted ones Aperio cannot place —,
the question comes whenever you change its repeat, even only the number of
appointments or the end; changing or deleting it from a later appointment on is
then refused with the sentence instead (below). Otherwise Aperio saves and
deletes the deleted ones again.

If you drag a series with the mouse and choose "Whole series", Aperio asks the
same question afterwards, in a dialog of its own; "Save anyway" moves the
series, "Cancel" leaves it where it was. If you carry a change to copies in
other calendars, Aperio first carries it to all the other copies, then asks
once about the copies whose series would lose occurrences, each named by its
calendar: "Carry over anyway" carries the change to them too, "Cancel" leaves
them among the copies not changed.

Changing or deleting a series from one of its appointments on only shortens the
old series, and that keeps its changed and deleted occurrences. Where Aperio
would write the time zone again with it (see above) and something would be
lost, the series is not shortened: Aperio does not ask there, it says the
sentence and changes nothing. Do that in Outlook then. Nor is a series with a
time of day shortened when Exchange stores it in a time zone Aperio cannot read
(see the next section). Changed from its first appointment on, the whole series
is written again, and Aperio asks as in the editor.

If Aperio cannot read a series' current state from the server, it does not
change it, because it would not know what would be lost. Aperio then says:
"Aperio could not read this appointment's current state from the server, so it
did not change it. Try again in a moment."

## Exchange stores an appointment in a time zone Aperio cannot read

Aperio then says: "Exchange stores this appointment in a time zone Aperio
cannot read, so its day cannot be changed here; its title and the rest can.
Nothing was changed."

Exchange places an all-day appointment on the midnights of the time zone it
stores it in. So that the appointment stays one day long, Aperio writes a new
day as midnight in exactly that time zone. Usually Aperio knows it: a Windows
time zone such as "W. Europe Standard Time" or a name such as "Europe/Berlin".
A custom time zone, such as "Customized Time Zone" on appointments from other
programs, Aperio can read when Exchange sends its rules along. When Exchange
sends only a name Aperio does not know, without its rules, Aperio does not
guess: the appointment would otherwise stretch over two days in Outlook.

What you can do: change the day in Outlook. Everything else, such as the
title, the place or the reminder, you can still change in Aperio, as long as
you do not move the day with it.

The same sentence comes when you change or delete a series with a time of day
from one of its appointments on, and Exchange stores it in such a time zone.
Exchange counts the day a shortened series ends on in that time zone, so
Aperio could not tell which day to end it on, and changes nothing. Change or
delete that series from the appointment on in Outlook. Its title, place and
reminder you can still change in Aperio.

A similar message is: "Aperio could not read this appointment's current state
from the server, so it did not change it." To know the time zone,
Aperio reads the appointment from the server before saving. When that fails,
for instance because the server is not answering right now, Aperio sends
nothing. Try again in a moment.

## An occurrence is back after a series was moved, copied or duplicated

When you move a whole series to another calendar, copy it, duplicate it with
`Ctrl+D` or choose another calendar for it in the editor, Aperio writes it
there as a new series. It takes along the occurrences deleted from the old
series, and nothing else:

- An occurrence changed on its own, for example in Outlook, is back at its
  usual time, with the series' title, place and time. After a move its own
  change is gone with the old series; after a copy or a duplicate it stays in
  the old one.
- Moved or copied into Exchange or Google, or duplicated there, the new
  series keeps the deletions from today on: Exchange takes none when a series
  is created, and Google deletes an occurrence by cancelling it, so Aperio
  removes them one by one afterwards. The ones before today show again.
- Moved or copied into Microsoft 365, the new series shows deleted
  occurrences again for now: Microsoft 365 takes no deletions when a series
  is created.
- From a Google calendar, Aperio knows deleted occurrences from about three
  months before today to a year after. One deleted further away is back in the
  new series.

What you can do: delete or change the occurrence that came back, in the new
calendar.

## Aperio removed a new series again

On Exchange and Google, Aperio creates a new series first and then removes
its deleted occurrences there one by one, while the dialog **Working, please
wait …** is shown: Exchange takes a new series without them, and Google
deletes an occurrence by cancelling it. When that cannot be done completely —
the calendar does not find an occurrence, answers too slowly, or the series
afterwards does not hold exactly the deletions Aperio made —, Aperio removes
the new series again and says so: nothing was changed, the old series is as
it was. On Exchange the removed series is in the Deleted Items. If the series
invited people, they got its cancellation too, and the message says so; on
Google, when you notified them, they may also have got a cancellation for each
occurrence Aperio had cancelled before. When it says it took too long, the
calendar answered slowly, or the series has very many deleted occurrences
from today on. When it says the provider was busy, the calendar kept asking
Aperio to wait: it does that when many requests arrive in a short time.

What you can do: try again later. If it keeps failing, send the log with the
report (see below): it names the step that failed and why, without any
appointment or calendar id.

## A new series may stand in the calendar as well

If Aperio could neither complete a new series on Exchange or Google nor
remove it again, it says the series may now stand in the calendar as well — next to the
old one, from the day you split it, or in the other calendar after a move or
a copy. This message only closes: saving again would create yet another
series. If invitations may have gone out, the message says that too.

What you can do: open the calendar, in Aperio or in the calendar's own app
(Outlook, Google Calendar), and delete the series that is there twice.

## The provider is busy right now

When many requests come in a short time, some providers turn further ones
away for a while. Aperio then says "The provider is busy right now": with
"Nothing was changed" when saving, without it when loading, checking
availability and for meetings. In an editor, what you entered stays; the
views show what was loaded last. On the desktop, where a list's sections
could not be loaded yet, the section picker of the task editor and of
Move/Copy says so; a section you create there still shows in it. Aperio does not say how long the
provider asks to wait; the log does.

What you can do: wait a moment and try again.

## A change was only partly saved

Some providers save a task, an appointment or a contact in several steps:
first the item, then, for example, its assignees, its photo or a group's
members. When the provider is busy after the first step, the item stands, but
not everything about it. Aperio then says the change was only partly saved,
and the message only closes: saving again would create the item a second
time. The view reloads and shows what the provider holds.

When the provider does not take a new contact's photo, the contact stands
without it, and Aperio says so.

When you create a task with subtasks and one of the subtasks fails, the
editor now edits the created task: the subtasks not yet created stay listed
under their own heading, and saving keeps your changes to the task and
creates the subtasks still missing. A subtask whose answer was lost may have
been created: Aperio says so and does not offer it again; check it.

When a task was saved only in part but its new status got through (a later
step, such as its assignees, was turned away), the tasks above and below it
still take the new status, as when you check it off; if one does not, the
message says so after its own sentence. If the status itself did not get
through, the editor leaves them as they were; a check-off still writes each of
them, so some may have taken the new status while the task kept its old one.

What you can do: open the item again later and add what is missing.

## After a move, the original is still there

When you move a task or an appointment into another list or calendar — any
move except one between two of Aperio's own local ones, also within one
account — Aperio creates a copy there and then deletes the original. When the
provider is busy for the delete, the moved item stands at the target, and the
original stays where it was. Aperio names the list or calendar it is still in.

What you can do: delete the original there yourself. Do not move it again,
because that would create a second copy at the target.

## Reporting a bug

1. In Settings → Logs, set the level to **Debug**.
2. Reproduce the problem.
3. **Export** the log (or copy it) and attach it to your report, together with
   what you did and what you expected.
