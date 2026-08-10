---
title: "05 – Views"
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
- **Everything else**: first the deadlines further out (still **earliest
  first**), below them the remaining backlog — tasks **without** a deadline,
  **highest priority first**.

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

## Summary

You now know the views and how to switch between and navigate within them.
Next we'll set up reminders.
