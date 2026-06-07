//! Data types for calendars, events, tasks, task lists, and contacts.

use base64::Engine;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::color::{ColorLabelId, ContainerColor};
use crate::reminder::{Reminder, SoundConfig};

/// Time interval (half-open: `[start, end)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl DateRange {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self { start, end }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Calendars & events
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar {
    pub id: String,
    pub name: String,
    pub color: Option<ContainerColor>,
    /// Optional binding to a global color label. When set, the rendered
    /// color resolves to the label's *current* hex (so recoloring the
    /// label recolors the container), taking priority over `color`. The
    /// id refers to a `ColorLabel`. `#[serde(default)]` keeps older wire
    /// payloads (no binding) deserialising.
    #[serde(default)]
    pub color_label: Option<ColorLabelId>,
    /// Read-only calendars (e.g. birthdays, public holidays, subscribed iCal
    /// feeds) cannot be modified by the caller.
    pub read_only: bool,
    /// Default sound for reminders of all events in this calendar.
    pub default_sound: Option<SoundConfig>,
    /// True when the backing provider can email attendees about
    /// invitations / updates / cancellations via *server-side* scheduling
    /// (no client SMTP): EWS, Google and Microsoft Graph always; CalDAV
    /// only when the server advertises RFC 6638 auto-scheduling; local /
    /// iCal never. Gates the "notify attendees" toggle in the UI.
    /// `#[serde(default)]` keeps older wire payloads + stores (which never
    /// set it) deserialising as `false`.
    #[serde(default)]
    pub supports_scheduling: bool,
    /// True when the backing provider can store a per-event color
    /// *natively* (RFC 7986 `COLOR`) so the color syncs to other clients
    /// and round-trips: **local** always; **CalDAV** when the server is
    /// color-capable (set to `!iCloud` by the CalDAV adapter); **Google /
    /// Microsoft Graph / EWS / iCal** never. When `false`, the host keeps a
    /// per-event color as a host-local override instead (the Stage 1
    /// `event_color_overrides` table). Gates how the frontend routes a
    /// recolor — through `update_event` (native) vs `set_event_color`
    /// (override). `#[serde(default)]` keeps older wire payloads + stores
    /// (which never set it) deserialising as `false`.
    #[serde(default)]
    pub supports_event_color: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub calendar_id: String,
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub recurrence: Option<EventRecurrence>,
    pub color_label: Option<ColorLabelId>,
    /// Transport-only native per-event color as `#RRGGBB`, used only with
    /// providers that store a color on the event itself (RFC 7986 `COLOR`,
    /// i.e. color-capable CalDAV). On a *write* the host resolves
    /// [`color_label`](Self::color_label) → this hex before handing the
    /// event to a color-capable adapter, which emits `COLOR`; on a *read*
    /// the adapter fills it from the provider's `COLOR` and the host maps
    /// it back to a `color_label`. `None` for local events and for
    /// non-capable providers (their color lives on `color_label`, kept
    /// either on the synced row or in a host-local override).
    /// `#[serde(default, skip…)]` keeps it `None` and off the wire / out of
    /// stores everywhere it isn't set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_hex: Option<String>,
    pub reminders: Vec<Reminder>,
    /// Sound override at the event level (section 14.4).
    pub sound: Option<SoundConfig>,
    pub attendees: Vec<String>,
    /// Transient organizer-side send intent for the pending write: when
    /// `true`, the adapter asks the provider to email attendees about this
    /// create/update through server-side scheduling. NOT persisted by any
    /// store and meaningless on a read — `#[serde(default, skip…)]` keeps it
    /// `false` and off the wire everywhere except an outbound update, where
    /// it rides the `Event` JSON across the plugin FFI.
    #[serde(default, skip_serializing_if = "is_false")]
    pub send_invitations: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Provider ETag / sync tag, used for optimistic-concurrency on push.
    pub etag: Option<String>,
    /// The organizer's address (RFC 5545 `ORGANIZER`, `mailto:` stripped),
    /// when the provider exposes it on read. Lets the host decide "is the
    /// connected account an *attendee* of this meeting rather than its
    /// organizer?" — the gate for showing RSVP buttons. Read-only: never
    /// sent on a write. `#[serde(default, skip…)]` keeps it `None` and off
    /// the wire on providers / stores that don't surface it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organizer: Option<String>,
    /// Per-attendee RSVP state, populated on read where the provider
    /// exposes it (CalDAV `ATTENDEE;PARTSTAT`, EWS `ResponseType`,
    /// Google/Graph `responseStatus`). Distinct from the flat, editable
    /// [`attendees`](Self::attendees) list. Empty on write and on
    /// providers that don't report response status.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendee_responses: Vec<AttendeeResponse>,
}

/// An attendee's RSVP status — RFC 5545 `PARTSTAT`, normalised across
/// providers. `NeedsAction` is the default for an invitee who hasn't
/// replied yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttendeeStatus {
    #[default]
    NeedsAction,
    Accepted,
    Declined,
    Tentative,
}

/// One attendee's RSVP state on an event read from a provider. Populated
/// on read only; the editable invitee set stays the flat
/// [`Event::attendees`] list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttendeeResponse {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub status: AttendeeStatus,
}

/// Payload for creating a new event (without server-assigned IDs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub recurrence: Option<EventRecurrence>,
    pub color_label: Option<ColorLabelId>,
    /// Native per-event color as `#RRGGBB` for this create — see
    /// [`Event::color_hex`]. The host fills it from `color_label` for a
    /// color-capable target; `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_hex: Option<String>,
    pub reminders: Vec<Reminder>,
    pub sound: Option<SoundConfig>,
    pub attendees: Vec<String>,
    /// Organizer-side send intent for this create — see [`Event::send_invitations`].
    #[serde(default, skip_serializing_if = "is_false")]
    pub send_invitations: bool,
}

/// serde `skip_serializing_if` predicate: keep a `false` flag off the wire.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Recurrence rule per RFC 5545 (RRULE).
///
/// Stored as a string so adapters can pass it through verbatim; evaluation
/// happens centrally in the backend with a dedicated crate (Phase 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecurrence {
    pub rrule: String,
    /// Exception dates that should be skipped.
    pub exceptions: Vec<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeBusy {
    pub email: String,
    pub slots: Vec<FreeBusySlot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeBusySlot {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

// ────────────────────────────────────────────────────────────────────────────
// Tasks & task lists
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskList {
    pub id: String,
    pub name: String,
    pub color: Option<ContainerColor>,
    /// Optional binding to a global color label — see `Calendar.color_label`.
    #[serde(default)]
    pub color_label: Option<ColorLabelId>,
    pub default_sound: Option<SoundConfig>,
    /// For task-capable calendars (CalDAV/VTODO, local): the calendar ID.
    /// For standalone task lists: `None`.
    pub embedded_in_calendar: Option<String>,
    /// Parent project id for backends with nested projects (Vikunja,
    /// Todoist). `None` ⇒ a top-level list, or a flat backend with no
    /// nesting at all (Google Tasks, CalDAV-per-calendar, …). The id
    /// refers to another `TaskList.id` from the same adapter.
    ///
    /// `#[serde(default)]` so wire payloads written before nested
    /// projects existed deserialise into a flat (parentless) list.
    #[serde(default)]
    pub parent_id: Option<String>,
    pub read_only: bool,
}

/// A sub-grouping of tasks *within* a single task list — a Vikunja
/// bucket or a Todoist section. Distinct from a nested project
/// (`TaskList.parent_id`): sections never contain other sections and
/// never contain sub-lists, they only group the tasks of one list.
///
/// Backends without the concept (Google Tasks, CalDAV VTODO, EWS,
/// local) simply return no sections from `TasksFeature::list_sections`
/// and leave every task's `section_id` at `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    /// The `TaskList.id` this section belongs to.
    pub list_id: String,
    pub name: String,
    /// Optional binding to a global color label — see `Calendar.color_label`.
    /// Cascades to the section's tasks that carry no color of their own
    /// (resolution chain: task → section → list). A label *reference*
    /// (not a frozen hex) so recoloring the label recolors every bound
    /// section live. A purely local, Aperio-synced concept like the
    /// section itself — no provider round-trip.
    #[serde(default)]
    pub color_label: Option<ColorLabelId>,
    /// Display order within the list; lower sorts first. Mirrors the
    /// `position` Vikunja attaches to buckets and the order Todoist
    /// gives its sections.
    pub order: u32,
}

/// A user in the task domain — used as a task assignee, as a member of a
/// task list's collaborator pool, and as the connected account's own
/// identity ("me"). `id` is the provider-native user id (Vikunja numeric
/// id stringified, Todoist user id, …); `name` is a display label;
/// `email` is best-effort (some providers omit it from the user listing).
/// See DESIGN §9.7 "Aufgaben-Zuweisung an andere Nutzer".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskUser {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
}

/// Permission level on a task-list share (Vikunja `0/1/2`). Adapters
/// without per-share roles (Todoist) report `None` on the share.
/// See DESIGN §9.7 "Mitglieder-/Freigabe-Verwaltung".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberRight {
    Read,
    Write,
    Admin,
}

/// One membership/share row of a task list: who has access, at what
/// right (if the backend models roles), and whether the invitation is
/// still pending acceptance (Todoist email invites are `pending` until
/// accepted). Distinct from `TaskUser` in the assignee pool — this is
/// the *editable* share list, not the effective members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskListShare {
    pub user: TaskUser,
    #[serde(default)]
    pub right: Option<MemberRight>,
    #[serde(default)]
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,

    // Scheduling
    //
    // The previous `deadline_type` enum is gone — what used to be
    // `type='on'` is now expressed by setting `scheduled_date` (and
    // optionally `scheduled_time`); what used to be `type='by'` is
    // the only deadline semantic left, expressed via `deadline_date`
    // (+ optional `deadline_time`).
    //
    // A task may have either, both, or neither set. Both means
    // "I plan to do it on `scheduled_date`, and it must be done by
    // `deadline_date`" — the deadline is the backstop, the schedule
    // is the working day.
    //
    // `*_time` fields require their matching `*_date` to be set;
    // the DB enforces this via CHECK constraints (migration 0006).
    pub scheduled_date: Option<NaiveDate>,
    pub scheduled_time: Option<NaiveTime>,
    pub deadline_date: Option<NaiveDate>,
    pub deadline_time: Option<NaiveTime>,

    pub recurrence: Option<TaskRecurrence>,
    pub parent_id: Option<String>,
    /// Section (Vikunja bucket / Todoist section) this task is filed
    /// under within its list. `None` ⇒ ungrouped, or a backend with no
    /// sections. Refers to a `Section.id` whose `list_id == self.list_id`.
    #[serde(default)]
    pub section_id: Option<String>,
    pub color_label: Option<ColorLabelId>,
    pub reminders: Vec<Reminder>,
    pub sound: Option<SoundConfig>,
    /// Users this task is assigned to. Empty ⇒ unassigned. Multi-valued;
    /// single-assignee backends (Todoist) take the first and warn on the
    /// rest. Read/written through the adapter's normal task get/create/
    /// update — rides serde, no separate FFI surface. See DESIGN §9.7.
    #[serde(default)]
    pub assignees: Vec<TaskUser>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub etag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewTask {
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub scheduled_date: Option<NaiveDate>,
    pub scheduled_time: Option<NaiveTime>,
    pub deadline_date: Option<NaiveDate>,
    pub deadline_time: Option<NaiveTime>,
    pub recurrence: Option<TaskRecurrence>,
    pub parent_id: Option<String>,
    /// Section to file the new task under; see `Task::section_id`.
    #[serde(default)]
    pub section_id: Option<String>,
    pub color_label: Option<ColorLabelId>,
    pub reminders: Vec<Reminder>,
    pub sound: Option<SoundConfig>,
    /// Assignees to set on the new task (see `Task::assignees`). Empty ⇒
    /// unassigned. Adapters clamp to their capability.
    #[serde(default)]
    pub assignees: Vec<TaskUser>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Open,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskPriority {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecurrence {
    pub frequency: RecurrenceFrequency,
    pub interval: u32,
    pub day_of_week: Option<Vec<Weekday>>,
    pub day_of_month: Option<u8>,
    pub end: Option<RecurrenceEnd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecurrenceFrequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecurrenceEnd {
    Never,
    After { occurrences: u32 },
    OnDate { date: NaiveDate },
}

// ────────────────────────────────────────────────────────────────────────────
// Contacts (DESIGN.md §10)
// ────────────────────────────────────────────────────────────────────────────

/// An address book — the contacts equivalent of `Calendar` and `TaskList`.
/// Every `Contact` belongs to exactly one list.
///
/// Sticking the contacts under their own container surface (rather than
/// flattening them under the account) keeps the model coherent with the
/// rest of the app: the sidebar tree still has one section per provider
/// → one node per container → leaf items, and per-list overrides work
/// (e.g. "include in autocomplete" can flip on a list at a time).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactList {
    pub id: String,
    pub name: String,
    pub color: Option<ContainerColor>,
    /// Optional binding to a global color label — see `Calendar.color_label`.
    #[serde(default)]
    pub color_label: Option<ColorLabelId>,
    /// Read-only address books (provider-curated lists like "Other
    /// contacts" on Google) can't be modified. Defaults to `false`
    /// for user-owned lists.
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub id: String,
    /// Container reference — the address book this contact lives in.
    pub list_id: String,
    /// Display name as the user wants to see it ("Max Mustermann").
    /// Required: a contact with no display name is a row we wouldn't
    /// know how to render in the picker.
    pub display_name: String,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    /// Organisation / company name. Free-form, multi-valued in
    /// providers like Google but Aperio keeps it scalar for now —
    /// the autocomplete picker only needs the primary value.
    pub organization: Option<String>,
    pub emails: Vec<String>,
    pub phone_numbers: Vec<String>,
    pub birthday: Option<NaiveDate>,
    /// Free-form notes the user (or the provider's UI) attached to
    /// the contact. Surfaced verbatim in the contact dialog; not
    /// indexed for autocomplete.
    pub notes: Option<String>,
    /// Group membership marker (DESIGN.md §10 distribution lists).
    /// `None` ⇒ this is a regular person-contact.
    /// `Some(members)` ⇒ this is a distribution list / address-book
    /// group, with the listed members in declared order. An empty
    /// `Vec` is still a group (a freshly created empty group),
    /// distinguishable from a person via the `Some` wrapper.
    ///
    /// On the wire each provider has its own group convention:
    ///   - EWS uses a separate `<t:DistributionList>` item type
    ///     alongside `<t:Contact>` in the same folder
    ///   - vCard 4.0 uses `KIND:group` + `MEMBER:mailto:…`
    ///   - vCard 3.0 (Apple / older servers) uses
    ///     `X-ADDRESSBOOKSERVER-KIND:group` +
    ///     `X-ADDRESSBOOKSERVER-MEMBER`
    ///
    /// All three funnel into this one field so the rest of the
    /// stack stays group-agnostic.
    ///
    /// **Serialization note:** we deliberately keep this field on
    /// the wire even when `None` (no `skip_serializing_if`). The
    /// frontend's group / person discriminator is a `!== null`
    /// check; if serde omitted the field instead, the value
    /// would arrive as `undefined`, satisfy `!== null`, and then
    /// crash on `.members.length`. Keep it explicit.
    #[serde(default)]
    pub members: Option<Vec<GroupMember>>,
    /// Photo presence flag. `true` ⇒ the contact has an avatar
    /// stored on the source (CardDAV PHOTO body, EWS
    /// ContactPicture attachment, local SQLite BLOB). The bytes
    /// themselves are pulled lazily via
    /// `ContactsFeature::get_contact_photo` so listings and the
    /// attendees picker stay cheap — a 1000-contact pull doesn't
    /// haul a megabyte of JPEGs across the wire.
    #[serde(default)]
    pub has_photo: bool,
    /// Postal addresses (DESIGN.md §10, Phase 10l / vCard ADR).
    /// Maps to vCard `ADR`, EWS `PhysicalAddresses/Entry`, Google
    /// People `addresses[]`, and MS Graph
    /// `homeAddress` / `businessAddress` / `otherAddress`. Empty
    /// `Vec` ⇒ the contact has no postal addresses on record;
    /// `default` on serde means older wire payloads (pre-Phase
    /// 10l) deserialise into an empty list without complaining.
    #[serde(default)]
    pub addresses: Vec<ContactAddress>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub etag: Option<String>,
}

/// One postal address attached to a contact. The shape is the
/// least-common-denominator across the four wire formats we
/// translate to:
///
///   - **vCard ADR** (RFC 6350 §6.3.1) has seven semicolon-
///     separated components: po-box; extended-address; street;
///     locality; region; postal-code; country-name. We fold
///     po-box + extended into `street` because Aperio's input
///     surface is a single multi-line text field — the
///     distinction matters to formal mailing software, not to
///     a calendar app's contact picker.
///   - **Google People API** has the same five-field flat shape
///     (`streetAddress` / `city` / `region` / `postalCode` /
///     `country`) plus a `type` string ("home" / "work" /
///     "other") that we round-trip in `label`.
///   - **MS Graph** flattens into three named slots (homeAddress,
///     businessAddress, otherAddress), each holding the five
///     fields. We map our `label` onto that slot name on the
///     write side.
///   - **EWS** uses `PhysicalAddresses/Entry[Key]` with
///     Street/City/State/PostalCode/CountryOrRegion children;
///     same structure.
///
/// `label` is a free-form string but the four convention slots
/// (`"home"`, `"work"`, `"other"`, `"work"` ↔ `"business"`) round-
/// trip on every adapter that distinguishes them. Unknown labels
/// fall through as the third "other" slot where the wire format
/// requires a choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ContactAddress {
    /// Category tag — `"home"` / `"work"` / `"other"`. Free-form
    /// because vCard 4 allows arbitrary TYPE parameter values; the
    /// adapter mappers normalise the common slots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Street + house number, optionally including apt / extended
    /// line. Kept as one field rather than split because the UI
    /// only renders a single multi-line input — splitting would
    /// force a confusing per-line decision on the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    /// Locality / city / town.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// Region / state / province / county. Province-shaped data
    /// in EU contexts is fine here too — adapters that don't
    /// model the field (e.g. CardDAV servers that only carry
    /// locality) emit a placeholder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Postal code / ZIP / Postleitzahl.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
    /// Country name as a string. We deliberately avoid an
    /// enum-of-ISO-codes — vCard transports country names
    /// verbatim and forcing a code dictionary would lose data on
    /// the read side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
}

/// One member of a distribution list. Mirrors the
/// `<t:Mailbox>` shape EWS uses and the `MEMBER` shape vCard 4.0
/// uses — both encode "(optional display name, email address)" as
/// the canonical identity. We do not link to a concrete `Contact.id`
/// here because the underlying contact can live on a different
/// server, in a different book, or not yet exist as a row at all
/// (typing an email into the picker should still produce a valid
/// member).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupMember {
    /// Display name as the user / provider typed it. Optional —
    /// raw email addresses without a name are common in dumb
    /// CSV imports.
    pub name: Option<String>,
    /// Required: the picker needs an addressable identifier and
    /// vCard's `MEMBER:mailto:` only accepts a URI scheme that
    /// resolves to something. We pin email here.
    pub email: String,
}

/// Payload for creating a new contact. Mirrors `NewTask` / `NewEvent`:
/// no server-assigned ids, no timestamps — the adapter fills those
/// in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewContact {
    pub display_name: String,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub organization: Option<String>,
    pub emails: Vec<String>,
    pub phone_numbers: Vec<String>,
    pub birthday: Option<NaiveDate>,
    pub notes: Option<String>,
    /// Postal addresses, same shape as `Contact::addresses`.
    /// Defaults to empty so the existing call sites that build
    /// a `NewContact` without thinking about addresses still
    /// compile after the Phase 10l field is added.
    #[serde(default)]
    pub addresses: Vec<ContactAddress>,
    /// See `Contact::members`. `Some(empty vec)` creates a new
    /// distribution list with no initial members; `None` creates a
    /// person-contact (the common case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<GroupMember>>,
    /// Optional avatar to attach on create. `None` ⇒ no photo;
    /// `Some` ⇒ the adapter writes the bytes through the same
    /// path `set_contact_photo` would. Carrying the photo on
    /// create lets a "new contact with photo" gesture land as a
    /// single command rather than a create-then-upload pair the
    /// caller has to keep transactionally consistent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<ContactPhoto>,
}

/// Binary avatar attached to a contact. Carried inline in
/// `NewContact` on create and shuttled across
/// `ContactsFeature::get_contact_photo` / `set_contact_photo` on
/// read / update; pulled lazily so it doesn't bloat listings.
///
/// `content_type` is a MIME type — we expect `image/jpeg`,
/// `image/png`, or `image/gif` in practice (the three EWS's
/// `ContactPicture.jpg` attachment slot and vCard `PHOTO`
/// property formally permit, and the three the frontend's file
/// picker filters down to).
///
/// `data` is the raw bytes. JSON serialisation uses base64 so the
/// payload travels through the Tauri IPC without ballooning into
/// the giant integer-array shape `Vec<u8>` produces by default —
/// every adapter and the command layer agree on this single
/// encoding via the `serialize_with` / `deserialize_with`
/// helpers below.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactPhoto {
    pub content_type: String,
    #[serde(serialize_with = "serialize_b64", deserialize_with = "deserialize_b64")]
    pub data: Vec<u8>,
}

fn serialize_b64<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    s.serialize_str(&encoded)
}

fn deserialize_b64<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(d)?;
    base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod rsvp_tests {
    use super::*;

    #[test]
    fn attendee_status_serialises_kebab_case() {
        // The TS union type + the FFI wire depend on exactly these.
        assert_eq!(
            serde_json::to_string(&AttendeeStatus::NeedsAction).unwrap(),
            "\"needs-action\""
        );
        assert_eq!(
            serde_json::to_string(&AttendeeStatus::Accepted).unwrap(),
            "\"accepted\""
        );
        assert_eq!(
            serde_json::to_string(&AttendeeStatus::Declined).unwrap(),
            "\"declined\""
        );
        assert_eq!(
            serde_json::to_string(&AttendeeStatus::Tentative).unwrap(),
            "\"tentative\""
        );
        assert_eq!(AttendeeStatus::default(), AttendeeStatus::NeedsAction);
    }

    #[test]
    fn attendee_response_omits_absent_name() {
        let r = AttendeeResponse {
            email: "bob@example.com".into(),
            name: None,
            status: AttendeeStatus::Accepted,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"status\":\"accepted\""));
        assert!(!json.contains("name"));
    }
}
