---
title: "04 – Tasks"
---

In this chapter you create tasks, assign them to lists, schedule them into
your day or week, and work through them.

## Creating a task

1. Open the **task view** or a task list in the sidebar.
2. Create a task: **Quick add task** (`Alt+N`) opens the quick dialog — it offers earlier
   entries too, and accepting one opens the already-filled editor, **New
   task** (`Alt+Shift+N`) the full form.
3. Enter a **title**. As you type, Aperio offers **earlier entries with that
   name** (arrow keys, `Enter` accepts; on the phone as a list under the
   field). Accept one and the rest of the editor fills from it — the length or
   the priority and effort, the repetition, the description, the reminders, and
   the **task list** (for an event, the **calendar**) it was filed under.
   **The day stays as it was:** that is what makes this a new entry. And if you
   picked a list or calendar yourself in the quick-add before accepting the
   offer, **your pick stays** — an older entry does not overrule a choice you
   just made.
   Optionally:
   - the **task list** it is stored in,
   - a **due date** (and optionally a time), each removable on its own:
     **Remove the time** strikes just the time and leaves the day standing,
     **Remove** clears the whole slot,
   - a **priority**,
   - a **description**,
   - a **color label**,
   - for collaborative lists (e.g. Todoist), an **assignee**.

## Completing and editing

- **Mark as done:** Select the task and check it off with `Space`
  (pressing again unchecks it). The **completion date** is recorded and
  shown — in the task list a finished task displays *“Completed: <date>”*
  in place of its due date, and the editor shows a *“Completed on”* line.
  It syncs both ways with every provider (Apple Reminders, Google, Vikunja,
  Microsoft To Do, Exchange, Todoist).
- **Edit:** Open it with `Enter` or a **double-click**. A single mouse click
  only **selects/focuses** the task, so you can pick one without the editor
  popping open.
- **Delete:** Choose **Delete** (default: `Delete`).

> **Quick status & priority:** Right-click a task — or press the Menu key
> (`Shift+F10`) — for a context menu with **Status** and **Priority**
> submenus. Change either in a single click without opening the editor; the
> current value carries a check-mark.

> **Two levels instead of three:** Under *Settings → Tasks* you can switch
> the whole app to a **two-level** priority system: a task is either
> **important** — marked with a **star** — or normal, and normal carries no
> mark at all. The editor then shows a single **Important** checkbox, and the
> context menu a single **Important** row. Priorities coming from providers
> with more levels read as normal unless they are the top one. Nothing is
> rewritten: switch back and low / medium / high are all still there, on every
> device (the setting itself syncs).

> **Priority & providers:** Every provider keeps a task's priority except
> **Google Tasks**, which has no priority field of its own — a priority you set
> on a Google task isn't stored on Google's side and reads back as *Medium*
> after the next sync. Local lists, Apple Reminders / CalDAV, Exchange,
> Microsoft To Do, Vikunja and Todoist all keep it.

> **"In progress" & providers:** Only providers with a real intermediate
> status store **In progress**: **local** lists, **Apple Reminders / CalDAV**
> (`IN-PROCESS`), **Exchange** and **Microsoft To Do**. **Google Tasks,
> Vikunja and Todoist** only know *open / done* — setting a task there to
> *In progress* falls back to *open* on the next sync. For those providers the
> automatic **schedule-to-today** that a started task would otherwise trigger
> is skipped too (it can't be remembered as "started" anyway). **Manual**
> scheduling — dragging a backlog task onto a day, or setting a date via the
> plan dialog (`Shift+D`) — still works exactly as before. In the **status
> cycle** (see *Check-off behaviour*) the check-off skips the *In progress*
> step for these providers: the cycle runs *open → completed → open*, so
> `Space` isn't trapped at *open*.

> **Check-off behaviour:** By default checking off flips between *open* and
> *completed*. Under **Settings → Tasks → Check-off behaviour** you can switch
> to a status cycle instead: each check-off steps *open → in progress →
> completed → open*, so the "in progress" state is reachable with `Space` or a
> click without opening the editor.

> **Hiding completed tasks:** Checked-off tasks move into a collapsed
> **Done (N)** group at the bottom of the list, so your open tasks stay
> uncluttered. The group shows the count. It's a regular row in the task
> tree: reachable with the arrow keys, and `Enter`/`Space` (or
> Right/Left arrow) expands or collapses it — just like a task with
> subtasks. The open/closed state is remembered.

> **Section colors:** Task sections can carry their own color (set it in
> the task dialog when you add or edit a section — or right on the section
> header in the task view, via right-click or the **⋮** button). Tasks
> **without** their own color take on their section's color; moving such a
> task into another section recolors it automatically. Order: task's own
> color → section → task list.

> **Creating, renaming, deleting sections:** On **local** lists and on
> **Todoist** and **Vikunja**, you can add, rename, and delete sections from
> three places — whichever is closest to hand: the section header's **⋮
> menu** (or right-click / `Shift+F10`) in the task view, a task list's
> **context menu in the sidebar** (*Add section*), or the **Section** field
> in the task dialog. The change is made at the provider. A section's
> **color** always stays local: you can set it for any section (including
> Todoist/Vikunja, which have no section-color of their own) and it's never
> sent to the provider.

> **Moving tasks between sections:** Use the **Section** field in the task
> dialog to file a task under a different section, or pick **No section**
> to pull it out entirely. This works for local lists, for Todoist, and
> for Vikunja (0.24+); picking **No section** on Vikunja files the task
> into the default bucket, since Vikunja keeps every kanban task in a
> bucket.

> **Where sections show:** In the **task view**, the list and section appear
> as their **own, focusable rows** in the tree (Backlog → list → section) —
> including inside the **Backlog** group, so a list's buckets (e.g. a Vikunja
> project's *To-Do / Doing / Done*) are visible even when nothing is
> scheduled. Each group row is reachable with the arrow keys;
> `Enter`/`Space` (or Right/Left arrow) expands or collapses it — just like
> the **Done** group or a task with subtasks. A section is only a grouping; a
> task's **status** (open / done) is independent of which section it's in.

> **With the mouse (drag & drop):** You can also drag a task onto a
> **section header** (in the task view) or onto a **list in the sidebar**
> to move it there; drag an **event** onto a **calendar in the sidebar**.
> For keyboard and screen-reader use, the Move/Copy dialog (or the Section
> field) remains the way.

## Grouping the list

The task-view header has a **Group by** selector:

- **State** (default): tasks are grouped by when they're due — **Overdue** (a
  planned day already past), **Today**, the **Backlog** (no planned day, still
  sub-grouped by list → section), **Upcoming** (tasks planned for a fixed future
  day, plus deferred tasks waiting to resurface — soonest first), **Done**, and
  **Cancelled** (tasks you've cancelled — kept out of the active groups, at the
  very end). Done, Upcoming and Cancelled start collapsed.
> **Overdue and the day-start review.** A task lands in **Overdue** on its
> **planned day** alone — the deadline does not put it there. The review at the
> start of the day asks about the two separately: missed **deadlines** first,
> then plans that **slipped**. A task shows up in one section, never both. When
> both of its dates lapsed on the same day, it is the **deadline** section that
> asks — and answering *Deadline today* there moves the planned day up as well,
> so the task really is settled and does not stay behind in Overdue.

- **List**: every open / in-progress task is grouped under **its own list** (and
  sections), regardless of whether it's scheduled, in the backlog, or deferred —
  one place per list. **Done** still sits separately at the bottom, exactly as in
  the State grouping. There is no separate Upcoming group here; deferred tasks
  simply appear in their list.

The choice is remembered **per device**.

## Scheduling tasks

Aperio distinguishes between tasks with and without a fixed date:

- **Backlog:** Tasks without a **scheduled day** collect here — including
  ones that have a deadline but no fixed work day.
- **Schedule:** Give a task a due date (or, in the week planner, drag it
  onto a day) to schedule it firmly. The quickest way is **Plan** in any
  task's context menu (right-click, `Shift+F10`, long-press or rotor action on
  the phone) — or `Shift+D` on the focused task.
- **Auto-schedule to today:** Setting a backlog task to **"In progress"**
  schedules it for **today** automatically — you've started the work after
  all. (Can be turned off under *Settings → Tasks*.)
- **Backlog list:** The **week and month views** show a fixed **Backlog**
  list **to the left of the grid** with every unscheduled task. Drag a task
  from there onto a day to schedule it — or drag a scheduled task back onto
  the list to return it to the backlog. Without a mouse: focus a task in the
  list and press **Shift+D** (the plan dialog), or use the context menu. Drag
  the list's **right edge** to resize it (the width is saved) — the view
  beside it adjusts accordingly.
- **Deadline:** A task with a deadline shows up in the week and day planner
  as a **due marker** on its deadline day ("due by …") — a single point, not
  a bar spanning every day until then. As long as no work day is set, it
  **also stays in the backlog**, so you can drag it onto a concrete work day.

## Projects: a parent task with its own subtasks

When a task has its own **subtasks**, Aperio treats the parent as a **project**
and shifts the day-to-day work onto the subtasks:

- **The parent stops nagging.** As long as the project still has open subtasks,
  the **day-start review never asks you about the parent** — you work the project
  through its subtasks. The parent simply keeps its **deadline**, shown as a due
  marker on its deadline day in the planner. It is never auto-pinned to today.
- **Plan the subtasks, not the parent.** Give each subtask its own day. A dated
  subtask now appears as its **own chip** in the week/month/day planner, marked
  with a leading **"↳"** and labelled with its parent ("subtask of …"); a subtask
  that carries its own deadline also shows in the backlog's **Deadline** column.
  So the daily review only ever asks you about the **subtask due that day**, not
  the whole project.
- **Closing the project.** Once **every** subtask is done (or cancelled), the
  parent returns to the day-start review so you can close it out — Aperio does
  **not** auto-complete it for you.

A term paper (deadline in three weeks, a growing list of subtasks spread across
the days) therefore plans itself through its subtasks, while the parent quietly
holds the final deadline and stays out of your way until the work is finished.

## Recurring tasks

Like events, tasks can recur (daily, weekly, monthly, yearly). When you
check off a recurring task, Aperio automatically creates the next due date.

> **When you have missed days:** the next turn is the **last one that was
> due** — not the next of the days you missed, and not the first one still
> ahead. A daily task left for a week is therefore back in step after **one**
> tick: yesterday's dose checked off this morning leaves today's, and only
> today's. For a monthly task — rent on the 1st, July's paid on 2 August — the
> next turn is **1 August**: a day overdue, but there, instead of a whole month
> falling away in silence. Catching up never costs more than **one** extra tick.
>
> The task you ticked stays on **the day it was planned for**, so each day in
> the calendar keeps its own turn instead of everything piling onto the day of
> the tick.

> **In the calendar:** A recurring task with a **scheduled day** now shows on
> **every** planned day in the day / week / month views — like a recurring
> event — not just its next turn. Only the **current** instance is interactive;
> the future days are read-only previews (announced *"recurring, planned"*, with
> a ↻ in place of the checkbox). Check one off, reschedule or edit it from the
> current instance — completing that advances the whole series. To get there
> from a preview, use **Go to the current task** (right-click or Shift+F10 on
> the desktop; the rotor actions or a long-press on the phone) — the calendar
> jumps to the day the real task sits on. This applies to
> **scheduled** recurrences that count **from the task's date**; *from
> completion* and *resurface-in-backlog* recurrences can't be projected ahead,
> so they still show only on their next day.

> **A planned block:** alongside a time you can give an **until**. The task
> then occupies its hours on the grid the way an event does, instead of sitting
> at a point — and is announced that way too ("09:00 to 10:30"). The field
> appears only in lists whose source can store an end: **local**, **CalDAV**,
> **Vikunja** and **Todoist**. The rest have nowhere to put one, so it is not
> offered there.
>
> **Times of day & providers:** not every service can store a **time** on a
> task. **Google Tasks**, **Microsoft To Do** and **Exchange** keep whole days:
> a time used to be accepted, dropped by the server, and silently gone after the
> next refresh. In those lists the editor no longer offers a time at all, and
> says why. Drag a task onto the hour grid there and the **day** is taken — and
> the announcement says so.
>
> **Recurrence & providers:** Whether — and how much of — a recurrence can be
> stored depends on the provider. The editor only appears where the list can
> store it, and **greys out individual fields the provider can't do** (rather
> than dropping them silently on save). **Local** lists, **Microsoft To Do**
> and **Apple Reminders / CalDAV** (`RRULE` in the VTODO) support full
> recurrence; **Exchange** too — only without a **yearly interval** (yearly
> works as "every year" but not "every 2 years"). **Vikunja** stores simple
> repeats — *daily*/*weekly* (with an interval, e.g. "every 2 weeks") and
> *monthly* — but has no *yearly*, no **weekday picker**, no fixed **day of
> month** (it repeats on the due date's day) and no **end date**; those
> fields are greyed out there. For **Google Tasks** and **Todoist** the
> recurrence editor isn't shown at all — these providers don't store task
> recurrence.

Not every chore repeats on a fixed calendar day — some come back **when
they're needed** and should reappear in the **backlog** as a reminder rather
than landing on a date. The recurrence editor has two extra choices for this:

- **Next instance** — *Schedule on a day* (the classic behaviour) or
  *Resurface in the backlog*, where the next turn is undated and simply shows
  up in the backlog again.
- **Counts from** — *From the task's date* (advance from the due date, as
  before) or *From completion* (advance from the day you actually finished it).

With *Resurface in the backlog* you can set the interval to **0** — *straight
back into the backlog on completion*. You can also give one or more **Fixed
dates** (month + day); these drive the schedule instead of the
daily/weekly/monthly interval. Two examples:

- **Empty the dishwasher** — *Counts from: From completion*, *Next instance:
  Resurface in the backlog*, interval **0**. Check it off and it's instantly
  back in your backlog, ready for next time.
- **Swap summer / winter shoes** — add the **Fixed dates** *1 April* and
  *1 October* with *Resurface in the backlog*. It reappears around each date
  every year instead of on a fixed interval.

> **The "Upcoming (N)" group:** A backlog task set to resurface on a future
> date doesn't clutter your active backlog until then — it waits in a
> collapsed **Upcoming (N)** group at the bottom of the task view, next to
> **Done**, with each task showing its resurface date (*"Resurfaces: …"*).
> It's a regular tree row: reachable with the arrow keys, `Enter`/`Space` (or
> Right/Left arrow) expands or collapses it, and the open/closed state is
> remembered. Want a waiting task back sooner? Right-click it (or
> `Shift+F10`) and choose **Bring to backlog** to pull it into the active
> backlog now. A future resurface date is a gentle reminder, not a deadline,
> so it never triggers the "missed tasks" prompt.

> **On-demand recurrence & providers:** *Resurface in the backlog*, *from
> completion* and *fixed dates* aren't part of any provider's own recurrence,
> so Aperio carries them itself and **creates the next instance for you on
> every list** — a provider can't. On a **shared plain-text list** (Vikunja,
> Todoist, Google Tasks) the extra data rides a small **managed block**
> appended to the task's description, marked *"⚙ Aperio · please don't edit"*:
> leave it untouched — Aperio strips it back out so your description stays
> clean. On **CalDAV, Exchange and Microsoft To Do** it rides an invisible
> custom property instead, so nothing shows in the description at all.

## Members and assignments

For shared lists (e.g. Todoist) you can:

- **manage members** from the list's context menu (invite, remove),
- **assign a person** to individual tasks.

### Letting Aperio assign you automatically

When a list's backend knows who *you* are (e.g. **Vikunja**), Aperio can keep
ownership in sync. With **Settings → Tasks → "Assign shared-list tasks to me"**
on (the default):

- Setting an **unassigned** task to **in progress** or **done** assigns it to
  **you**; reopening it removes **only your** assignment (a colleague's stays).
- For a **recurring** task only the completed instance gets your name — the next
  instance comes back **unassigned**, ready for whoever picks it up.
- The **Done** group shows a split count, e.g. *"Done – 12 by me, 3 by others"*,
  so you can see what you finished versus what teammates did.
- The **day-start review** only ever offers tasks that are **yours or
  unassigned**; a task assigned to someone else is left for them to handle.
- The **calendar views** (day, week, month) likewise show only tasks that are
  **yours or unassigned** — a task assigned to someone else stays off your
  calendar.

Turn the toggle off to keep assignments fully manual.

> **Vikunja – finding people:** In the **Manage members** dialog you search
> for people. Vikunja matches a **username only exactly** (the full name,
> case-insensitive) – a partial username is not enough. Partial matches work
> only for the **display name**, and the **email** must again be exact. The
> person must also have marked themselves **discoverable** in their Vikunja
> settings (by name and/or by email). Vikunja deliberately offers no
> "list all users". You change permissions (read / write / admin) afterwards
> right in the member list.

> **Screen-reader note:** Tasks are announced with title, status (done /
> open), due date, priority and — if anyone is assigned — the **assignee**
> (in **every** view: the task, week, day and month views, plus the
> backlog). In the three-level system **high** priority also shows "!!!",
> **medium** "!!" and **low** "!"; in the two-level one **important** shows a
> star and everything else nothing at all. The **list and
> section** are no longer repeated in each task's label — they come from the
> surrounding **group rows** (Backlog → list → section) you pass through as
> you arrow down, expanding or collapsing them with `Enter`/`Space`. Checking
> off with `Space` is reported immediately as "done" or "open", without
> moving the focus.

## Quick dates

Every date field in the quick-add carries a **Quick dates** button beside it.
It opens four offers — today, tomorrow, the coming weekend, the start of next
week — and each one says the date it will set, so you never have to accept one
to find out what it was. "Next week" follows the week start you chose under
**Settings → General**.

There is deliberately nothing to configure here. Four learnable buttons are
worth more than a list nobody would maintain, and the date field beside them
still takes any day at all.

A task you leave dateless still goes to the backlog — the offers sit next to
the field rather than filling it in for you.

## Summary

You can create, check off, schedule and repeat tasks, and assign them in
shared lists. In the next chapter you'll set up your address book.
