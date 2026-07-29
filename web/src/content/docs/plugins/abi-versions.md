---
title: "ABI versions and how to migrate"
description: "What each Aperio plugin-ABI revision changed, what a plugin author has to do about it, and the rules that decide when the number moves at all."
---

A plugin declares an `abi_version` in its `plugin.json`, and the host refuses to
load a plugin whose number is not exactly its own. This page says what each
revision contains and what moving to it costs you.

**Current: ABI 3.** The authoritative number is `ABI_VERSION` in
`crates/plugin-core/src/version.rs`; if your host refuses your plugin with an ABI
mismatch, that constant is what it compared against.

## If you only do one thing

Bump `"abi_version": 3` in your `plugin.json` and rebuild. For most plugins that
is the whole migration: the two new vtable slots are optional, the new manifest
blocks are optional, and every new payload field carries a serde default.

You must do more only if your plugin is a **videoconference adapter**, because
one existing method changed the shape of its argument. That is the next section.

## v2 → v3

### 1. `delete_meeting` takes an object, not a bare id

The only breaking change for existing code, and it affects videoconference
adapters alone.

It used to receive a JSON string — the meeting id. It now receives

```json
{ "id": "abc123", "notify_attendees": false }
```

Taking a meeting down is also a question about the people who were invited to
it. On a calendar that cannot cancel server-side — a local calendar, a
subscribed feed, plain CalDAV — the provider's own mail is the only word the
attendees get that the meeting is off, and the host now says which case it is in.

`notify_attendees` carries `#[serde(default)]`, so a payload without it decodes
as `false`, i.e. as silence.

Before:

```rust
unsafe extern "C" fn ffi_delete_meeting(
    h: *mut c_void, a: *const u8, l: usize,
) -> PluginCallResult {
    let id: MeetingId = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(h, move |p| async move { p.delete_meeting(&id).await })
}
```

After:

```rust
unsafe extern "C" fn ffi_delete_meeting(
    h: *mut c_void, a: *const u8, l: usize,
) -> PluginCallResult {
    let removal: MeetingRemoval = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(h, move |p| async move { p.delete_meeting(removal).await })
}
```

If your provider cannot notify anybody, ignore the flag. Reading `removal.id`
and doing exactly what you did before is a complete migration.

> **Why this was allowed at all.** Changing an existing slot's wire shape in
> place is normally forbidden — see the rules below. It was permissible here
> only because ABI 3 has never shipped, so no plugin exists that speaks the
> earlier v3 shape. Once v3 is released, the next such change takes v4.

### 2. `VcVtable` gained two slots

`resolve_meeting` and `list_meetings`, appended after `delete_meeting`:

```c
typedef struct AperioVcVtable {
    uint32_t vtable_version;
    AperioVtableMethodFn test_connection;
    AperioVtableMethodFn create_meeting;  /* NewMeeting -> Meeting */
    AperioVtableMethodFn get_meeting;     /* MeetingId -> Option<Meeting> */
    AperioVtableMethodFn delete_meeting;  /* {id,notify_attendees} -> () */
    /* ── ABI 3 ── */
    AperioVtableMethodFn resolve_meeting; /* {join_url} -> Option<Meeting> */
    AperioVtableMethodFn list_meetings;   /* {start,end} -> Meeting[] */
} AperioVcVtable;
```

Both may be `NULL`, and a plugin that leaves them so behaves exactly as it did
under v2 — the host simply omits the affordance rather than failing.

`resolve_meeting` finds a meeting from its join link. The link is the only
identifier that reaches a calendar event; your provider's own meeting id travels
nowhere. Without this slot the host can manage only meetings it created itself
and still remembers locally — not one made in your web interface, not one made
on the user's other device, not one an invitation brought in.

`list_meetings` enumerates a window. It is what lets the host surface meetings
that have no calendar entry at all.

**Appending those two slots is what forced the version bump**, and the reason is
worth knowing because it will bite the next person: the host has no per-vtable
length. It reads your vtable as a struct of the size IT was compiled with. A
plugin built against the shorter layout, loaded by a newer host, would be read
past its end. Strict equality on `abi_version` is the only thing standing
between that and calling whatever memory follows.

`vtable_version` — the `u32` at offset 0 of every vtable — is now actually read
(`vtable_layout_ok`), which it was not before v3 despite the header claiming so
since the ABI existed. Set it to the ABI version you build against; the SDK's
`VcVtable::empty()` and friends already do.

### 3. New manifest blocks, all optional

None of these break an existing manifest. Omitting them leaves your plugin on
exactly the path it was on.

**`adapter_kind`** — the short, stable routing key your accounts carry
(`"caldav"`, `"webex"`). Declaring it is what lets the host map an account row
back to your plugin without a list of kinds compiled into the core.

It is deliberately **not** your plugin id: the kind is persisted in every account
row and travels in every sync payload, so it has to stay byte-stable for the life
of the data, while a plugin id is free to change when a plugin is renamed. You
may use your id as your kind; they are simply separate promises.

**`account`** — the fields your connect form asks for, which of them are
secrets, and whether you sign in via OAuth. Declare it and the host draws your
form with no host-side code. See [the manifest reference](/plugins/manifest/).

**`strings`** — your own text, keyed by language then by key, referenced from
the `label_key` / `hint_key` of an account field. The host resolves against
YOUR catalogue, never its own translations, and falls back: requested language,
the base of a regional tag (`de-AT` → `de`), English, then the verbatim `label`
you wrote. A plugin with no catalogue renders its literals, which is a perfectly
good answer.

### 4. New payload fields, all serde-defaulted

`NewMeeting` gained `use_personal_room`, `attendees` and `notify_attendees`;
`Meeting` gained `invitees` and `join_details`. Every one carries
`#[serde(default)]`, so a plugin that has never heard of them still
deserialises. This is the general rule and it is worth stating outright: **adding
a serde-default field to a JSON payload is ABI-transparent.** Only a brand-new
*method* needs a vtable slot.

One thing to know if you are a videoconference adapter that fills them:
`NewMeeting::attendees` is **bare email addresses**. The host splits its own
display strings (`"Alice Smith <alice@example.test>"`) before handing them over.

### 5. Two new optional named exports

`aperio_plugin_strings` answers one language's strings, for a plugin whose
translations do not fit a JSON block in its manifest. The host calls it once per
language and caches the result, merging it over the manifest catalogue.

`aperio_plugin_set_host_channel` hands your plugin a sink for things the host
did not ask about. Vtable calls run host→plugin only, so this is the sole way an
adapter can say "the credential I hold has changed" — which an OAuth provider
that rotates refresh tokens forces it to say.

Neither is gated on v3, and neither needs any bump: `set_host_channel` in fact
landed while the ABI was still 2. They are listed here because they are new
since v2 and you may want them, not because the version number obliges you.

## The rules that decide whether the number moves

These are what the codebase actually follows. If you are extending Aperio rather
than writing a plugin, this is the checklist.

**Bump required:**

- Appending a slot to an **existing** vtable. The host has no per-vtable length,
  so an older plugin would be read past its end.
- Changing an existing slot's argument or return shape, once the current
  revision has shipped.
- Any change to a struct's C layout.

**No bump:**

- A new optional **named export**, looked up by symbol at load time and absent
  without consequence: `aperio_plugin_interactive_auth`,
  `aperio_plugin_discover`, `aperio_plugin_probe_host_key`,
  `aperio_plugin_strings`, `aperio_plugin_set_log`,
  `aperio_plugin_set_host_channel`. These are free functions with no instance
  handle; a host that predates one never looks it up, and a plugin that lacks one
  is simply asked to do less.
- Adding a `#[serde(default)]` field to a JSON payload.
- Adding a whole **new** vtable for a **new** plugin type. Nothing reads it
  unless that type exists.
- Adding an optional manifest block.

## Where the authoritative list lives

`crates/plugin-core/src/version.rs`, in the `## History` doc comment on
`ABI_VERSION`. This page mirrors it. If the two ever disagree, the Rust source
is right and this page is a bug worth reporting.
