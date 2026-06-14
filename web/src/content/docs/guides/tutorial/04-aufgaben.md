---
title: "04 – Tasks"
---

In this chapter you create tasks, assign them to lists, schedule them into
your day or week, and work through them.

## Creating a task

1. Open the **task view** or a task list in the sidebar.
2. Create a task: **Quick add task** (`Alt+N`) opens the quick dialog, **New
   task** (`Alt+Shift+N`) the full form.
3. Enter a **title**. Optionally:
   - the **task list** it is stored in,
   - a **due date** (and optionally a time),
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
- **Edit:** Open it with `Enter`.
- **Delete:** Choose **Delete** (default: `Delete`).

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
> **Todoist** and **Vikunja**, you can add, rename, and delete sections
> right in the task dialog — the change is made at the provider. A
> section's **color** always stays local: you can set it for any section
> (including Todoist/Vikunja, which have no section-color of their own) and
> it's never sent to the provider.

> **Moving tasks between sections:** Use the **Section** field in the task
> dialog to file a task under a different section, or pick **No section**
> to pull it out entirely. This works for local lists, for Todoist, and
> for Vikunja (0.24+); picking **No section** on Vikunja files the task
> into the default bucket, since Vikunja keeps every kanban task in a
> bucket.

> **With the mouse (drag & drop):** You can also drag a task onto a
> **section header** (in the task view) or onto a **list in the sidebar**
> to move it there; drag an **event** onto a **calendar in the sidebar**.
> For keyboard and screen-reader use, the Move/Copy dialog (or the Section
> field) remains the way.

## Scheduling tasks

Aperio distinguishes between tasks with and without a fixed date:

- **Backlog:** Tasks without a **scheduled day** collect here — including
  ones that have a deadline but no fixed work day.
- **Schedule:** Give a task a due date (or, in the week planner, drag it
  onto a day) to schedule it firmly.
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

## Recurring tasks

Like events, tasks can recur (daily, weekly, monthly, yearly). When you
check off a recurring task, Aperio automatically creates the next due date.

## Members and assignments

For shared lists (e.g. Todoist) you can:

- **manage members** from the list's context menu (invite, remove),
- **assign a person** to individual tasks.

> **Vikunja – finding people:** In the **Manage members** dialog you search
> for people. Vikunja matches a **username only exactly** (the full name,
> case-insensitive) – a partial username is not enough. Partial matches work
> only for the **display name**, and the **email** must again be exact. The
> person must also have marked themselves **discoverable** in their Vikunja
> settings (by name and/or by email). Vikunja deliberately offers no
> "list all users". You change permissions (read / write / admin) afterwards
> right in the member list.

> **Screen-reader note:** Tasks are announced with title, status (done /
> open), due date and priority. **High** priority also shows a "↑", **low**
> a "↓"; medium priority (the default) shows nothing. Checking off with
> `Space` is reported immediately as "done" or "open", without moving the
> focus.

## Summary

You can create, check off, schedule and repeat tasks, and assign them in
shared lists. In the next chapter you'll get to know the views.
