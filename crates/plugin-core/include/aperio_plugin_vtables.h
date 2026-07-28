/*
 * aperio_plugin_vtables.h — Aperio plugin per-type vtable layouts
 * (DESIGN.md §20.3).
 *
 * This header is the C mirror of the Rust vtable structs in
 * `crates/plugin-core/src/vtables/*.rs`. `aperio_plugin.h` defines the
 * descriptor (`AperioPlugin`) whose `vtable` field points at one of the
 * structs below, selected by `plugin_type`:
 *
 *   - "calendar-adapter"        → AperioCalendarAdapterVtable (the
 *                                 multi-capability wrapper bundling up to
 *                                 three sub-vtables)
 *   - "sync-adapter"            → AperioSyncVtable (directly)
 *   - "videoconference-adapter" → AperioVcVtable (directly)
 *   - "notification"            → reserved (no vtable yet)
 *
 * Every slot is a function pointer of type `AperioVtableMethodFn`. A
 * plugin that doesn't implement a method leaves the slot NULL; the host
 * then surfaces `cal_core::Error::Unsupported` (or the sync/vc
 * equivalent) — the same UX as the default-`Unsupported` trait methods.
 *
 * ── Calling convention ────────────────────────────────────────────────
 * Each method takes the opaque per-instance handle returned by
 * `AperioPlugin.open_instance` as its first argument, followed by a
 * JSON-encoded argument buffer (`args_ptr` + `args_len`; `(NULL, 0)` for
 * no-arg methods). The argument object's keys mirror the Rust parameter
 * names. The method returns a `PluginCallResult`: on
 * `APERIO_PLUGIN_CALL_OK` the payload is the JSON-encoded return value
 * (empty for void-returning methods); on any non-zero status the payload
 * is a UTF-8 error message. See `aperio_plugin.h` for the buffer
 * ownership + threading rules.
 *
 * ── Stability ─────────────────────────────────────────────────────────
 * Every struct here is `#[repr(C)]` on the Rust side; field order and
 * types below MUST match it byte-for-byte.
 *
 * `vtable_version` is the FIRST field of every vtable, in every revision,
 * and the host READS it before trusting anything else in the struct: a
 * value other than `APERIO_PLUGIN_ABI_VERSION` means the host cannot know
 * how many slots are really there, so it refuses to wrap the plugin
 * rather than reading past the end of it. Plugin authors MUST set every
 * `vtable_version` field to `APERIO_PLUGIN_ABI_VERSION`.
 *
 * New methods are APPENDED at the end of a vtable, and appending to an
 * EXISTING vtable REQUIRES bumping `APERIO_PLUGIN_ABI_VERSION`. Strict
 * equality on the manifest then keeps an older plugin out entirely, which
 * is the only safe answer while the host has no per-vtable length. Adding
 * a WHOLE NEW vtable for a new plugin type needs no bump: nothing reads
 * it unless that type exists.
 *
 * The four bytes of alignment padding after `vtable_version` on 64-bit
 * targets are RESERVED for a future `uint32_t struct_size`. Do not write
 * into them. When that field arrives it will carry a NEW
 * `vtable_version`, because padding in a plugin built before it existed
 * is indeterminate and a garbage value there would defeat the very gate
 * it is meant to relax.
 *
 * Keep this file in sync with `crates/plugin-core/src/vtables/*.rs`
 * whenever a slot is added.
 */

#ifndef APERIO_PLUGIN_VTABLES_H
#define APERIO_PLUGIN_VTABLES_H

#include <stdint.h>
#include <stddef.h>

#include "aperio_plugin.h" /* PluginCallResult, APERIO_PLUGIN_ABI_VERSION */

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Method-pointer type used by every vtable slot. `instance` is the
 * handle `AperioPlugin.open_instance` returned for this account (may be
 * NULL for instance-less plugins). `args_ptr`/`args_len` carry the
 * JSON-encoded arguments. A NULL slot means "method not implemented".
 */
typedef PluginCallResult (*AperioVtableMethodFn)(
    void          *instance,
    const uint8_t *args_ptr,
    size_t         args_len
);

/*
 * ── CalendarVtable ────────────────────────────────────────────────────
 * Mirrors `cal_core::CalendarFeature`. JSON responses:
 *   list_calendars → Vec<Calendar>; get_events → Vec<Event>;
 *   create_event/update_event → Event; delete_event/add_event_exdate/
 *   rename_calendar → null; get_free_busy → Vec<FreeBusy>;
 *   calendar_color → Option<ContainerColor> (synchronous);
 *   get_events_delta → ChangeSet<Event>;
 *   current_user_email → Option<String>; respond_to_event → null.
 */
typedef struct AperioCalendarVtable {
    uint32_t vtable_version;

    /* Base Adapter methods. */
    AperioVtableMethodFn authenticate; /* Credentials -> AuthToken */
    AperioVtableMethodFn capabilities; /* () -> Vec<Capability> */

    /* CalendarFeature methods. */
    AperioVtableMethodFn list_calendars;
    AperioVtableMethodFn get_events;
    AperioVtableMethodFn create_event;
    AperioVtableMethodFn update_event;
    AperioVtableMethodFn delete_event;
    AperioVtableMethodFn get_free_busy;
    AperioVtableMethodFn calendar_color;   /* synchronous */
    AperioVtableMethodFn add_event_exdate; /* default-Unsupported */
    AperioVtableMethodFn rename_calendar;  /* default-Unsupported */
    /* get_events_delta(calendar_id, range, since_token) ->
       ChangeSet<Event> (CACHE-4). NULL ⇒ host falls back to a full
       get_events. */
    AperioVtableMethodFn get_events_delta;
    /* current_user_email() -> Option<String> (RSVP identity gate).
       NULL ⇒ host treats it as Ok(None) ("identity unknown"). */
    AperioVtableMethodFn current_user_email;
    /* respond_to_event(event_id, status, send_response) (RSVP).
       NULL ⇒ default-Unsupported. */
    AperioVtableMethodFn respond_to_event;
} AperioCalendarVtable;

/*
 * ── TasksVtable ───────────────────────────────────────────────────────
 * Mirrors `cal_core::TasksFeature`. JSON responses:
 *   list_task_lists → Vec<TaskList>; get_tasks → Vec<Task>;
 *   create_task/update_task → Task; delete_task → null;
 *   rename_task_list/delete_task_list → null;
 *   list_sections → Vec<Section> (NULL slot ⇒ empty, not Unsupported);
 *   create_task_list → TaskList; get_tasks_delta → ChangeSet<Task>.
 */
typedef struct AperioTasksVtable {
    uint32_t vtable_version;

    AperioVtableMethodFn authenticate;
    AperioVtableMethodFn capabilities;

    AperioVtableMethodFn list_task_lists;
    AperioVtableMethodFn get_tasks;
    AperioVtableMethodFn create_task;
    AperioVtableMethodFn update_task;
    AperioVtableMethodFn delete_task;
    AperioVtableMethodFn rename_task_list; /* default-Unsupported */
    AperioVtableMethodFn list_sections;    /* NULL ⇒ no sections */
    AperioVtableMethodFn create_task_list; /* default-Unsupported */
    AperioVtableMethodFn delete_task_list; /* default-Unsupported */
    /* get_tasks_delta(list_id, since_token) -> ChangeSet<Task>
       (CACHE-4). NULL ⇒ host falls back to a full get_tasks. */
    AperioVtableMethodFn get_tasks_delta;
    /* list_task_list_members(list_id) -> Vec<TaskUser> — assignee pool
       of a list (DESIGN §9.7). NULL ⇒ empty (no one to assign to). */
    AperioVtableMethodFn list_task_list_members;
    /* current_user() -> Option<TaskUser> — the account's own identity.
       NULL ⇒ None (no remote identity, e.g. local adapter). */
    AperioVtableMethodFn current_user;
    /* Membership management (DESIGN §9.7): list_task_list_shares ->
       Vec<TaskListShare>; search_users -> Vec<TaskUser>; add / remove /
       set_right -> unit. NULL ⇒ empty / Unsupported. */
    AperioVtableMethodFn list_task_list_shares;
    AperioVtableMethodFn search_users;
    AperioVtableMethodFn add_task_list_member;
    AperioVtableMethodFn remove_task_list_member;
    AperioVtableMethodFn set_task_list_member_right;
    /* Section CRUD — appended at the end (additive ABI).
       create_section(list_id, name) -> Section;
       update_section(list_id, section_id, new_name) -> Section;
       delete_section(list_id, section_id) -> null.
       A section's color is never sent (it's a local override). */
    AperioVtableMethodFn create_section;
    AperioVtableMethodFn update_section;
    AperioVtableMethodFn delete_section;
} AperioTasksVtable;

/*
 * ── ContactsVtable ────────────────────────────────────────────────────
 * Mirrors `cal_core::ContactsFeature`. JSON responses:
 *   list_contact_lists → Vec<ContactList>; get_contacts → Vec<Contact>;
 *   search_contacts → Vec<Contact>; create_contact/update_contact →
 *   Contact; delete_contact/rename_contact_list → null;
 *   get_contact_photo → Option<ContactPhoto>; set/delete_contact_photo →
 *   null; invalidate_contacts_cache → null (NULL slot ⇒ no-op);
 *   get_contacts_delta → ChangeSet<Contact>.
 */
typedef struct AperioContactsVtable {
    uint32_t vtable_version;

    AperioVtableMethodFn authenticate;
    AperioVtableMethodFn capabilities;

    AperioVtableMethodFn list_contact_lists;
    AperioVtableMethodFn get_contacts;
    AperioVtableMethodFn search_contacts;
    AperioVtableMethodFn create_contact;
    AperioVtableMethodFn update_contact;
    AperioVtableMethodFn delete_contact;
    AperioVtableMethodFn rename_contact_list;     /* default-Unsupported */
    AperioVtableMethodFn get_contact_photo;       /* NULL ⇒ Ok(None) */
    AperioVtableMethodFn set_contact_photo;       /* default-Unsupported */
    AperioVtableMethodFn delete_contact_photo;    /* default-Unsupported */
    AperioVtableMethodFn invalidate_contacts_cache; /* NULL ⇒ no-op */
    /* get_contacts_delta(list_id, since_token) -> ChangeSet<Contact>
       (CACHE-4). NULL ⇒ host falls back to a full get_contacts. */
    AperioVtableMethodFn get_contacts_delta;
} AperioContactsVtable;

/*
 * ── CalendarAdapterVtable (outer wrapper) ─────────────────────────────
 * The `AperioPlugin.vtable` pointer for a "calendar-adapter" plugin
 * points at one of these. Each sub-vtable pointer is NULL when the
 * plugin doesn't declare the matching capability; the manifest's
 * `capabilities` array MUST match the non-NULL pointers (the host
 * cross-checks at load time). At least one sub-vtable must be non-NULL.
 */
typedef struct AperioCalendarAdapterVtable {
    uint32_t                       vtable_version;
    const AperioCalendarVtable    *calendar; /* NULL ⇒ no Calendar cap */
    const AperioTasksVtable       *tasks;    /* NULL ⇒ no Tasks cap */
    const AperioContactsVtable    *contacts; /* NULL ⇒ no Contacts cap */
} AperioCalendarAdapterVtable;

/*
 * ── SyncVtable ────────────────────────────────────────────────────────
 * Mirrors `sync_core::SyncAdapter`. The `AperioPlugin.vtable` pointer
 * for a "sync-adapter" plugin points directly at one of these. Minimum
 * surface: fetch_meta + push_meta + fetch_new_logs + push_log.
 */
typedef struct AperioSyncVtable {
    uint32_t vtable_version;

    AperioVtableMethodFn test_connection;
    AperioVtableMethodFn fetch_meta;       /* () -> Option<MetaJson> */
    AperioVtableMethodFn push_meta;
    AperioVtableMethodFn fetch_new_logs;   /* DeviceCursor -> Vec<LogFile> */
    AperioVtableMethodFn push_log;
    AperioVtableMethodFn fetch_snapshot;   /* () -> Option<Snapshot> */
    AperioVtableMethodFn push_snapshot;
    AperioVtableMethodFn delete_log;
    AperioVtableMethodFn push_sound_asset; /* (hash, bytes) */
    AperioVtableMethodFn fetch_sound_asset; /* hash -> Option<bytes> */
} AperioSyncVtable;

/*
 * ── VcVtable ──────────────────────────────────────────────────────────
 * Mirrors `vc_core::VcAdapter`. The `AperioPlugin.vtable` pointer for a
 * "videoconference-adapter" plugin points directly at one of these.
 * Minimum surface: create_meeting + delete_meeting.
 */
typedef struct AperioVcVtable {
    uint32_t vtable_version;

    AperioVtableMethodFn test_connection;
    AperioVtableMethodFn create_meeting; /* NewMeeting -> Meeting */
    AperioVtableMethodFn get_meeting;    /* MeetingId -> Option<Meeting> */
    AperioVtableMethodFn delete_meeting; /* MeetingId -> () */
} AperioVcVtable;

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* APERIO_PLUGIN_VTABLES_H */
