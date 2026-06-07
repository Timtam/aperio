# 04 – Tasks

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
  (pressing again unchecks it).
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

- **Backlog:** Tasks without a due date collect here.
- **Schedule:** Give a task a due date (or, in the week planner, drag it
  onto a day) to schedule it firmly.
- **Auto-schedule to today:** Setting a backlog task to **"In progress"**
  schedules it for **today** automatically — you've started the work after
  all. (Can be turned off under *Settings → Tasks*.)
- **Backlog rail:** The **week and month views** show a collapsible
  **Backlog** rail at the bottom with every unscheduled task. Drag a task
  from there onto a day to schedule it — or drag a scheduled task back onto
  the rail to return it to the backlog. Without a mouse: focus a task in the
  rail and press **Shift+D** (the plan dialog), or use the context menu.
- **Deadline:** A task with a deadline shows up in the week and day planner
  as a **due marker** on its deadline day ("due by …") — a single point, not
  a bar spanning every day until then.

## Recurring tasks

Like events, tasks can recur (daily, weekly, monthly, yearly). When you
check off a recurring task, Aperio automatically creates the next due date.

## Members and assignments

For shared lists (e.g. Todoist) you can:

- **manage members** from the list's context menu (invite, remove),
- **assign a person** to individual tasks.

> **Screen-reader note:** Tasks are announced with title, status (done /
> open), due date and priority. **High** priority also shows a "↑", **low**
> a "↓"; medium priority (the default) shows nothing. Checking off with
> `Space` is reported immediately as "done" or "open", without moving the
> focus.

## Summary

You can create, check off, schedule and repeat tasks, and assign them in
shared lists. In the next chapter you'll get to know the views.
