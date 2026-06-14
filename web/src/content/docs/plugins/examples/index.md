---
title: "Examples"
---

Two worked examples, from smallest to a realistic starting point:

- **[hello-world](/plugins/examples/hello-world/)** — the absolute minimum: a
  calendar-adapter plugin that exposes one capability method and returns an
  empty list. The thing to copy when you just want *something* the host
  loads and recognises.
- **[calendar-adapter-template](/plugins/examples/calendar-adapter-template/)** — a full
  scaffold for a real calendar adapter, with every method present as a stub
  and inline comments pointing at what to fill in.

Both follow the structure from [The Rust SDK](/plugins/examples/rust-sdk/): a library crate
with your trait impl, plus a `cdylib` plugin crate with the SDK macros and
a `plugin.json`.

> The bundled adapters under `crates/cal-adapter-*` (and their
> `-plugin` wrappers) are themselves excellent, real-world references —
> `cal-adapter-todoist` is a compact tasks-only example;
> `cal-adapter-google` shows a full calendars+tasks+contacts provider.
