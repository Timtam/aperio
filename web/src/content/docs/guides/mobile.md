---
title: "Mobile app"
---

Aperio is also a **mobile app for iOS and Android**. It is built on the **same
core** as the desktop app, so your calendars, task lists, accounts and
synchronization work exactly the same way – and, like the desktop app, it was
designed from the ground up to be **fully usable with a screen reader**
(VoiceOver on iOS, TalkBack on Android).

This page covers what is specific to the mobile app. Everything about the
**features themselves** – events, tasks, views, reminders, search,
synchronization, contacts and colour labels – is described in the
[Tutorial](/guides/tutorial/01-installation/) and applies equally here.

## Getting the app

The mobile app is currently in **beta**:

- **iOS** – distributed through **TestFlight**. You receive an invitation link
  and install it via Apple's TestFlight app.
- **Android** – installed directly from a provided build.

Both are built from the same Rust core as the desktop app, so a calendar or
task you create on your phone behaves identically to one created on the desktop.

## Finding your way around

The app has a **bottom tab bar** with four tabs:

- **Tasks** – your task lists, grouped and collapsible, with the rich task
  editor.
- **Calendar** – the day, week, month, year and agenda views, plus calendar
  management.
- **Contacts** – your address books and contacts.
- **Settings** – accounts, synchronization, reminders, colour labels, logs and
  the general settings.

Editors (for a task, an event or a contact) open as **full screens** on top of
the current tab. Each has a **Save** and a **Cancel** action; use the system
**Back** gesture or the Cancel button to leave without saving.

## Using a screen reader

The mobile app follows the **same accessibility-first principles** as the
desktop app (see [Accessibility](/guides/barrierefreiheit/) for the shared
concepts), adapted to the way VoiceOver and TalkBack work:

- **One stop per item.** Each task, event or contact is a single focus stop.
  Swipe left or right to move between items, headings and controls.
- **Actions instead of shortcuts.** Where the desktop uses keyboard shortcuts,
  the mobile app exposes **custom actions** on the focused item – complete or
  reopen a task, edit, delete, reschedule, change status or priority, move it,
  and so on:
  - With **VoiceOver**, swipe up or down with one finger to move through the
    available actions, then double-tap to run the selected one.
  - With **TalkBack**, open the **actions menu** (swipe up then right) and
    choose an action.
- **Live announcements.** Status changes are announced without moving the focus
  – "Task done", "Event saved", the synchronization result, due reminders –
  exactly as on the desktop.
- **Group headings** (e.g. a task list or a section) are collapsible buttons
  that announce whether they are expanded or collapsed.
- **Dates and times** use the **native pickers**, so they read and behave the
  way you already know from other apps on your phone.

## Mobile-specific settings

A few settings exist only on mobile, under **Settings → General**. All three are
stored **on this device only** (they are not synchronized):

- **Background sync.** Lets the system wake the app to synchronize while it is in
  the background or closed, so a change made on another device – and any new
  reminders – arrive without you reopening the app. The operating system decides
  the exact timing (it is not immediate; on Android at least every 15 minutes, on
  iOS in system-chosen windows), so it is a best-effort catch-up. The app always
  does a full synchronization when you open it, so nothing is ever lost. You can
  see background rounds in the sync log, labelled **Background**. Default: on.
- **App icon badge.** Shows a number on the app icon for today's open tasks plus
  the events still ahead today. Needs notification permission. Default: on.
- **Haptic feedback.** A short vibration when an external-data refresh starts and
  finishes. Default: on.

## Reminders and notifications

Reminders are delivered as **local notifications**. On first use the app asks for
**notification permission** – grant it so reminders (and the app icon badge) can
appear. Reminder sounds, lead times and snooze work as described under
[Notifications](/guides/tutorial/06-benachrichtigungen/).

## Home-screen widget (iOS)

**Up Next** shows your next events and due tasks on the home screen. Add it the
usual way: touch and hold the home screen, choose **Add widget**, find **Aperio**
and pick a size. VoiceOver reads each row as one sentence – title, day, time –
rather than as separate fragments you have to swipe between.

Widgets follow **Aperio's language**, not the phone's — so if you have set
Aperio to German on an English phone, the widgets are German too. Clock format
and date order stay the phone's regional settings, the way every other clock on
the home screen behaves.

The widget draws from a small snapshot the app keeps up to date, covering the
next seven days. That snapshot is refreshed whenever the app runs and on each
background sync round, so the widget stays current without draining the battery.
Two states are deliberately worded differently:

- **"Nothing planned."** – there genuinely is nothing in the next seven days.
- **"No current data. Open Aperio."** – the widget has run past what it knows.
  Opening the app refreshes it.

Tasks can be **ticked off straight from the widget**: a task row *is* a
checkbox – the whole row, not a separate button beside it. VoiceOver reads it as
one item, ending with the checkbox and its state, so what the row is and what can
be done with it arrive together in a single swipe. The row disappears as soon as
you tick it.

A tap does exactly what a tap in the app does, including under the **cycling**
check-off mode: there one tap moves a task from open to in progress, and the next
one completes it. The widget does not decide that — it asks for a check-off and
the app applies your setting. Which state a task is in is on the row itself: an
empty circle for open, a half-filled one for in progress, and the word is spoken
too, so neither cue stands alone.

The tick is recorded immediately and carried out by the app the next time it
runs – on opening it, or on a background sync round. That is not a delay you have
to manage: completing a task in Aperio cascades to parents and subtasks,
advances a recurring series and queues a sync push, and a widget has neither the
memory nor the access to do that work itself. Events have no button, and neither
do future occurrences of a recurring task: completion belongs on the current
one, which is what moves the series forward.

Calendars you have hidden on this device stay hidden in the widget too. Tasks
come from **all** your lists, not only the ones currently selected in the task
view.

## Lock-screen widgets (iOS)

Both widgets can go on the lock screen, under the clock: touch and hold it,
choose **Customise**, then the area below the time.

**Up Next** – the same list as on the home screen, shortened to fit: three rows,
each on a single line, and each a checkbox in the same way. So a task can be
completed straight from the lock screen, without unlocking the phone or opening
anything.

**Next Up** is the other one: a single line saying what is next and how
long until it – "in 25 minutes", and once it has started, "Running until 11:00".
It shows **only items that have a clock time**. All-day entries are left out on
purpose: there is no moment to count down to, and a long one – a fortnight's
holiday, say – would otherwise answer "what is next" with "holiday" for the whole
fortnight, straight through every appointment you still have to keep. When
nothing timed is coming up it says **"Nothing with a time."**, which is not the
same claim as "nothing planned".

It reads the same snapshot as the home-screen widget, so it needs no extra
setup. VoiceOver reads it as one sentence, ending with whether the item is an
event or a task – something neither the icon nor a colour can say on its own.

The countdown itself is not spoken as it ticks. A number that changes every
second would interrupt continuously; the spoken form is the coarse one you hear
when you focus the widget.

Android widgets are not available yet.
