---
title: "06 – Views"
---

Aperio shows your events and tasks in several views. In this chapter you get
to know them and switch between them.

## The views at a glance

| View | Shows |
|---|---|
| **Day** | a single day with an hourly grid |
| **Week** | seven days side by side |
| **Month** | a month as a grid; every day the same size |
| **Year** | a month overview of an entire year |
| **Agenda** | a continuous list of upcoming events |
| **Tasks** | your tasks, split into scheduled and backlog |
| **Week planner** | distribute tasks across the days of the week |

## Switching views

- Via the **view menu** in the toolbar.
- By keyboard: in many setups the digits `1`–`7` are mapped to the views;
  your actual mapping is in the [shortcut overview](/guides/tastaturkuerzel/),
  where you can change it.

## Navigating within a view

- **Arrow keys:** move the selection (day/hour/event).
- **One list per day:** in the day view, events **and** tasks live in the same
  list — everything with a time first, then the tasks without one. You never
  have to leave the list to learn what else the day holds. The band under the
  grid shows the same tasks for sighted users.
- **Back and forward:** use `Page Up` / `Page Down` to page to the
  previous/next period (week, month …).
- **Today:** jumps back to the current day (default: `T` or "Today" in the
  toolbar).
- **Week numbers** are shown in the week and month views.

## Visible hours in the hour grid

By default the hour grid (day and week views) shows the whole day from 0 to 24.
Under **Settings → Calendars → "Visible hours"** you set a start and end — e.g.
7:00 to 23:00 — in **half-hour steps**. The grid then shows only that window, so
your usual hours get more room.

Events or tasks **outside** the window aren't lost: they appear in a compact
**band** above (before the window start) / below (after the window end) the
grid, and stay reachable with **keyboard and screen reader** in time order —
with their real time in the name.

The setting is **synced across your devices**. It affects only the hour grid;
the compact list view still shows every entry.

## Collapsing the sidebar

The **sidebar** (accounts, calendars and task lists) can be collapsed and
expanded with the button at the **left edge**. When collapsed, the view — and,
in the week/month planner, the backlog list — gets **more room**.
The state is remembered across restarts.

> **Keyboard & screen reader:** The button sits **before** the sidebar in the
> tab order — after expanding, one more Tab reaches the sidebar; when collapsed,
> Tab skips it. Press `F6` to jump between regions (sidebar, toolbar, view,
> backlog).

## The backlog (week & month planner)

To the left of the week/month grid sits the **backlog** — your top-level tasks
that have **no day** assigned yet. It is split into three parts, by how far away
the deadline is:

- **This week** (top): deadlines in the **current calendar week**. That is not
  "the next seven days" — the section ends when the week ends, wherever your
  week-start setting puts that. **Overdue** deadlines are here too, at the very
  top.
- **Next week**: deadlines in the following calendar week, by the same rule.
- **Other**: first the deadlines further out (still **earliest first**), below
  them the remaining backlog — tasks **without** a deadline, **highest priority
  first**.

Every open or in-progress task with a deadline appears, even one already
scheduled onto a day, so this is your one place to see what is due soonest. Each
of those chips shows its due date.

Drag a chip onto a day cell to **schedule** it there, or drop a scheduled task
back onto the backlog to **clear** its day and deadline.

> **Keyboard & screen reader:** Each part is its own single-stop **listbox**:
> Tab lands on it once, `Arrow`/`Home`/`End` move between tasks, `Enter` opens a
> task, `Shift+D` opens the plan dialog (assign a day), and the context-menu key
> (or `Shift+F10`) opens the task menu.

## The month view

In the month view, every day cell is **the same size**, and the grid fills
the available window height: make the window taller and the cells grow,
showing **more events per day**. If not all events fit, the titles are
truncated (with "…") and the rest are summarized in a hint. When you select
an event, you always hear the **full** title – the truncation is purely
visual.

> **Screen-reader note:** When you switch views, the new view and the
> currently focused point in time are announced (e.g. "Month view, June
> 2026"). Within a view you move purely with the arrow keys; switching to
> browse mode is not necessary.

## Check-in: what a day was like

Beside the appointments and the tasks, a day can carry a note about
**itself** — the things you want to keep track of rather than tick off. A
habit, a mood, a fact worth remembering later.

### Your own list

Under **Settings → Check-in** you build the list. Aperio ships none: a guessed
set would be somebody else's habits. You decide both what is worth a check-in
and how much to say — a word, a whole sentence, or just an emoji.

Each item has:

- a **name**, which is what gets read aloud,
- an optional **short symbol** (usually one emoji) for the compact views,
- an optional **colour** from your existing palette,
- a **position**, so the list reads back in the order you built it.

Select an item to **edit**, **delete** or **reorder** it. On the desktop the
list is one arrow-navigable stop: arrow keys move the selection, Enter opens
the selected item, and the buttons under the list act on it — each of them
names the item it will affect. With a mouse, click to select, double-click to
edit, or simply **drag a row** to a new place. On the phone each row carries
its own fields and buttons.

Deleting an item leaves your past check-ins untouched. It simply stops
appearing in them — nothing rewrites your history, and re-creating it brings
it back.

### Checking in

Every calendar view has a **Check-in** button, and it acts on the day the view
is standing on: in the week and month grids that is the day cell you have
focused, in the day view it is that day, and in the agenda it is the day the
list starts on. There is deliberately no button per day — seven or thirty-one
of them would cost more to get past than they are worth. The button's spoken
name always names its day, so it is never ambiguous which one you are about to
open.

It opens a checkbox per item. Every tick **saves immediately** — there is no
Save button, because recording a day has to cost almost nothing. That also
means unticking is the undo: there is nothing to cancel.

### The overview

A day that says something says so in **its own heading**: "Monday, 17 August
2026. Check-in: Sport, Read." In the week and month grids the day cell
announces the same after its date and count. Sighted readers see the symbols
beside the date.

Nothing here is a separate stop to swipe past — the summary is part of the
name the day already had.

> **What is not a task.** Check-in items deliberately live outside your task
> lists. They are not work to be finished, so they never appear in the planner,
> never carry deadlines or reminders, and never turn up in the day-start
> review.

> **Privacy and sync.** Check-ins are stored **only on your devices** and
> travel over your own device sync, never to Google, Microsoft or any CalDAV
> server — none of them models "how was Tuesday", and this is the most private
> thing in the app. A check-in made on your phone reaches your desktop with the
> next sync round, without a restart. Two devices editing the *same* day
> between two rounds keep the later edit rather than merging them.

## Summary

You now know the views and how to switch between and navigate within them.
Next we'll set up reminders.
