# Example: calendar-adapter-template

A realistic starting point for a **full** calendar adapter — every method
present as a stub with comments on what to fill in. Split it like the
bundled adapters: a **library crate** (`my-adapter`) with the trait impl
and HTTP/mapping logic, and a thin **plugin crate** (`my-adapter-plugin`)
with the SDK glue from [The Rust SDK](rust-sdk.md).

## The library crate: trait impl

```rust
use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Calendar, CalendarFeature, Capability, Contact,
    ContactList, ContactsFeature, Credentials, DateRange, Event, NewContact,
    NewEvent, NewTask, Result, Section, Task, TaskList, TasksFeature,
};

pub struct MyAdapter {
    http: reqwest::Client,
    token: String,
    base_url: String,
}

impl MyAdapter {
    pub fn new(token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            token,
            base_url: "https://api.example.com".into(),
        }
    }

    /// Test-only: point the client at a mockito stand-in. Keep this so
    /// your adapter is testable offline (see the dev book's Testing page).
    #[doc(hidden)]
    pub fn with_base_url_for_tests(token: String, base_url: String) -> Self {
        Self { http: reqwest::Client::new(), token, base_url }
    }
}

#[async_trait]
impl Adapter for MyAdapter {
    async fn authenticate(&self, _c: Credentials) -> Result<AuthToken> {
        // The token is already in hand (from open_instance config); nothing
        // to do. OAuth providers do the dance host-side and pass the token.
        Ok(AuthToken::default())
    }
    fn capabilities(&self) -> &[Capability] {
        // Declare every surface you implement below.
        &[Capability::Calendar, Capability::Tasks, Capability::Contacts]
    }
}

// ── Calendars + events ────────────────────────────────────────
#[async_trait]
impl CalendarFeature for MyAdapter {
    async fn list_calendars(&self) -> Result<Vec<Calendar>> {
        todo!("GET the provider's calendars, map to cal_core::Calendar")
    }
    async fn get_events(&self, cal: &str, range: DateRange) -> Result<Vec<Event>> {
        // Return recurring MASTERS (with RRULE) even if their first
        // occurrence is outside `range` — the frontend expands them. (Or,
        // like Graph, ask the server to expand and return instances.)
        todo!()
    }
    async fn create_event(&self, cal: &str, ev: NewEvent) -> Result<Event> { todo!() }
    async fn update_event(&self, ev: Event) -> Result<Event> { todo!() }
    async fn delete_event(&self, event_id: &str) -> Result<()> { todo!() }
    // get_events_delta(...) — implement if your provider has sync tokens;
    // otherwise the host falls back to a full range fetch.
}

// ── Tasks ─────────────────────────────────────────────────────
#[async_trait]
impl TasksFeature for MyAdapter {
    async fn list_task_lists(&self) -> Result<Vec<TaskList>> { todo!() }
    async fn get_tasks(&self, list_id: &str) -> Result<Vec<Task>> { todo!() }
    async fn list_sections(&self, list_id: &str) -> Result<Vec<Section>> {
        Ok(vec![]) // ok to leave empty if your provider has no sections
    }
    async fn create_task(&self, list_id: &str, t: NewTask) -> Result<Task> { todo!() }
    async fn update_task(&self, t: Task) -> Result<Task> { todo!() }
    async fn delete_task(&self, task_id: &str) -> Result<()> { todo!() }
    // Optional collaboration methods (assignees, membership) have
    // cal-core defaults — only override the ones your provider supports.
}

// ── Contacts ──────────────────────────────────────────────────
#[async_trait]
impl ContactsFeature for MyAdapter {
    async fn list_contact_lists(&self) -> Result<Vec<ContactList>> { todo!() }
    async fn get_contacts(&self, list_id: &str) -> Result<Vec<Contact>> { todo!() }
    async fn create_contact(&self, list_id: &str, c: NewContact) -> Result<Contact> { todo!() }
    async fn update_contact(&self, c: Contact) -> Result<Contact> { todo!() }
    async fn delete_contact(&self, contact_id: &str) -> Result<()> { todo!() }
}
```

## The plugin crate: glue

Follow [The Rust SDK](rust-sdk.md): a `cal_dispatch_helpers!(MyAdapter)`,
one `ffi_*` wrapper per method you implemented, a `CalendarVtable` /
`TasksVtable` / `ContactsVtable` filled with `Some(ffi_*)` (and
`..Vtable::empty()`), the `CalendarAdapterVtable` pointing at the three
(null for any capability you don't provide), and `declare_lifecycle!`.

## Tips

- **Only wire what you support.** Unimplemented trait methods use cal-core
  defaults (empty list / `Unsupported`); leave their vtable slots `None`.
- **Keep it testable.** The `with_base_url_for_tests` constructor lets you
  point at a `mockito` server — write a test per endpoint that asserts the
  wire → `cal-core` mapping.
- **Errors:** return `cal_core::Error` variants
  (`Network`, `Authentication`, `NotFound`, `Conflict`, `Protocol`,
  `Unsupported`, `InvalidInput`); the SDK marshals them across the ABI and
  the host maps them to user-facing messages.
- **Recurrence:** decide up front whether you return masters (frontend
  expands) or server-expanded instances — see the
  [adapters overview](/aperio/dev/adapters/overview.html).
