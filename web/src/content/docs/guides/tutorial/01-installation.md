---
title: "01 – Installation & First Launch"
---

In this chapter you install Aperio, start it for the first time and set up
the most important basics.

## Installing Aperio

Aperio is distributed as an archive (ZIP):

1. Download the ZIP file for your operating system.
2. **Extract** it to a location of your choice (e.g. a dedicated folder in
   your user directory).
3. Start the program from the extracted folder.

> **Portable:** If a `data/` folder sits next to the program, Aperio stores
> its data there – handy for USB sticks. Otherwise the data is kept in your
> system's standard data directory. For notifications on Windows, Aperio
> needs one entry outside its folder: a Start menu shortcut, created only
> when you ask (see [Chapter 07](/guides/tutorial/07-notifications/)).

## The first-launch wizard

The first time you start Aperio – on a fresh install with no accounts, sync
or data yet – a short, accessible wizard walks you through the essentials:

- The interface **language** (default: your system language).
- **Reminders on Windows** – on Windows, unless notifications are already set
  up: the Start menu entry Windows needs before it shows Aperio's reminders
  (see [Chapter 07](/guides/tutorial/07-notifications/)). **Set up
  notifications** creates it; **Next** moves on without it. The step
  indicator then counts four steps.
- A **first account** (or skip it and add one later – see
  [Chapter 02](/guides/tutorial/02-connecting-accounts/)).
- The **storage location** – restore your data from an existing one, set up
  a new one to sync across your devices, or skip it for now. The fields you
  are asked for come from the plugin itself, so they are the same ones you
  would fill in when adding an account. If you connect to a location that
  already holds an Aperio dataset, you first see when it was last compacted
  and which devices use it; then your data and accounts are restored.

Storage comes last either way: joining an existing dataset brings the
accounts with it, and starting a fresh one means you have already been
through the account step.

You can change everything later in the **settings**.

## Finding your way around

Aperio has two main areas:

- the **sidebar** on the left with your calendars, task lists and address
  books (which you can show and hide),
- the **main area** on the right with the current view (day, week,
  month …).

Use the Tab key to move between the areas; within a view you navigate with
the arrow keys.

> **Screen-reader note:** Aperio runs in application mode
> (`role="application"`). You do **not** need to switch to browse (virtual)
> mode – the arrow keys control the calendar and lists directly. On startup
> the active view is announced; changes (e.g. "Event saved") are
> communicated through a live region.

## Language

By default Aperio uses your **system language** (falling back to English if
your language isn't available). Under **Settings → General → Language** you
can pin it to **Deutsch** or **English**, or pick "System language". The
choice is synced across your devices.

## Summary

You have extracted and started Aperio and completed the first-launch
wizard. In the next chapter you connect your first calendar or task list.
