//! Todoist tasks adapter — Phase 6h.
//!
//! Todoist is a hosted SaaS task manager with a REST API. The
//! adapter declares only `Capability::Tasks` and authenticates with
//! a personal API token the user mints in Todoist's UI under
//! Settings → Integrations → Developer. No OAuth dance: Todoist's
//! personal tokens are long-lived until revoked, exactly the shape
//! the keychain's `SecretSlot::ApiToken` was reserved for.
//!
//! Unlike Vikunja (self-hosted, server URL is user-supplied), the
//! Todoist API base URL is fixed (`https://api.todoist.com/rest/v2`)
//! — the account config carries no URL, just a display label.
//!
//! Scope of Phase 6h.1:
//!
//!   - List the user's projects as Aperio task lists (with named-
//!     palette colour mapped to hex)
//!   - Read active tasks per project
//!   - Create / update / delete tasks
//!   - Rename a project
//!
//! See `tasks.rs` for the mapping decisions (status via `/close` +
//! `/reopen`, priority inversion, due / deadline fields, named
//! colours) and the list of fields not yet round-tripped
//! (recurrence, reminders, labels, cross-project moves).

pub mod api;
pub mod error;
pub mod tasks;

use std::time::Duration;

use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Capability, Credentials as CoreCredentials, Error as CoreError,
    MemberRight, NewTask, Result as CoreResult, Section, Task, TaskList, TaskListShare, TaskUser,
    TasksFeature,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

pub use api::TodoistClient;
pub use error::{TodoistError, TodoistResult};

/// Persisted, non-secret half of a Todoist account's configuration.
/// The API token itself lives in the platform keychain
/// (`SecretSlot::ApiToken`). Todoist has no per-account server URL —
/// the API is hosted, so the config struct exists mostly to keep
/// the storage shape parallel with the other adapters and to hold
/// an optional display label.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodoistAccountConfig {
    /// Optional human-readable label captured at create time (e.g.
    /// the Todoist account's email). Used for display only.
    #[serde(default)]
    pub account_label: Option<String>,
}

#[derive(Debug)]
pub struct TodoistAdapter {
    client: TodoistClient,
    capabilities: Vec<Capability>,
    /// 5-minute listing-cache, same shape as the other adapters.
    task_lists_cache: Mutex<Option<(Vec<TaskList>, chrono::DateTime<chrono::Utc>)>>,
    listing_ttl: chrono::Duration,
}

impl TodoistAdapter {
    /// Build the adapter against the production Todoist API.
    /// `token` is the API token from the user's developer settings.
    pub fn new(token: String) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            client: TodoistClient::new(token, http),
            capabilities: vec![Capability::Tasks],
            task_lists_cache: Mutex::new(None),
            listing_ttl: chrono::Duration::minutes(5),
        }
    }

    /// Test-only constructor that points the client at a mockito
    /// stand-in instead of api.todoist.com. The integration tests
    /// in `tasks.rs` use the lower-level `TodoistClient` directly;
    /// this helper exists so smoke-test plumbing in the command
    /// layer can swap in a stub if needed.
    #[doc(hidden)]
    pub fn with_base_url_for_tests(token: String, base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            client: TodoistClient::with_base_url_for_tests(token, http, base_url),
            capabilities: vec![Capability::Tasks],
            task_lists_cache: Mutex::new(None),
            listing_ttl: chrono::Duration::minutes(5),
        }
    }

    #[doc(hidden)]
    pub fn with_listing_ttl(mut self, ttl: Duration) -> Self {
        self.listing_ttl =
            chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::zero());
        self
    }

    async fn cached_task_lists(&self) -> Option<Vec<TaskList>> {
        let guard = self.task_lists_cache.lock().await;
        let (items, ts) = guard.as_ref()?;
        let age = chrono::Utc::now().signed_duration_since(*ts);
        if age >= chrono::Duration::zero() && age < self.listing_ttl {
            Some(items.clone())
        } else {
            None
        }
    }

    /// One-shot reachability + auth probe. `GET /projects` accepts
    /// only an authenticated request — anything 2xx means the
    /// token is valid and the API is reachable. The same code
    /// path as `list_task_lists` keeps the request signature
    /// honest; the result is discarded.
    pub async fn smoke_test(&self) -> CoreResult<()> {
        let _: serde_json::Value = self
            .client
            .get_json("/projects")
            .await
            .map_err(to_core_error)?;
        Ok(())
    }
}

#[async_trait]
impl Adapter for TodoistAdapter {
    async fn authenticate(&self, _credentials: CoreCredentials) -> CoreResult<AuthToken> {
        // Same story as Vikunja: the token was already in hand at
        // construction time and lives inside the `TodoistClient`.
        // Nothing to do here.
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl TasksFeature for TodoistAdapter {
    async fn list_task_lists(&self) -> CoreResult<Vec<TaskList>> {
        if let Some(cached) = self.cached_task_lists().await {
            return Ok(cached);
        }
        let fresh = tasks::list_task_lists(&self.client)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await = Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_tasks(&self, list_id: &str) -> CoreResult<Vec<Task>> {
        tasks::get_tasks(&self.client, list_id)
            .await
            .map_err(to_core_error)
    }

    async fn list_sections(&self, list_id: &str) -> CoreResult<Vec<Section>> {
        tasks::list_sections(&self.client, list_id)
            .await
            .map_err(to_core_error)
    }

    /// The project's collaborators — the assignee pool for the picker.
    /// `current_user` stays trait-defaulted (`None`): Todoist's REST v2
    /// has no `/user` endpoint, so "assigned to me" highlighting is a
    /// Sync-API follow-up (DESIGN §9.7).
    async fn list_task_list_members(&self, list_id: &str) -> CoreResult<Vec<TaskUser>> {
        tasks::list_task_list_members(&self.client, list_id)
            .await
            .map_err(to_core_error)
    }

    /// Project shares incl. pending invites (Sync API). `search_users`
    /// and `set_task_list_member_right` stay trait-defaulted: Todoist
    /// has no user directory (members are invited by raw email) and no
    /// per-share roles.
    async fn list_task_list_shares(&self, list_id: &str) -> CoreResult<Vec<TaskListShare>> {
        tasks::list_task_list_shares(&self.client, list_id)
            .await
            .map_err(to_core_error)
    }

    /// Invite a member by email (Sync `share_project`). `right` is
    /// ignored — Todoist has no roles.
    async fn add_task_list_member(
        &self,
        list_id: &str,
        member_ref: &str,
        _right: Option<MemberRight>,
    ) -> CoreResult<()> {
        tasks::add_task_list_member(&self.client, list_id, member_ref)
            .await
            .map_err(to_core_error)
    }

    /// Revoke a member / cancel a pending invite (Sync
    /// `delete_collaborator`). `member_ref` is the member's email.
    async fn remove_task_list_member(&self, list_id: &str, member_ref: &str) -> CoreResult<()> {
        tasks::remove_task_list_member(&self.client, list_id, member_ref)
            .await
            .map_err(to_core_error)
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> CoreResult<Task> {
        tasks::create_task(&self.client, list_id, task)
            .await
            .map_err(to_core_error)
    }

    async fn update_task(&self, task: Task) -> CoreResult<Task> {
        tasks::update_task(&self.client, &task)
            .await
            .map_err(to_core_error)
    }

    async fn delete_task(&self, task_id: &str) -> CoreResult<()> {
        tasks::delete_task(&self.client, task_id)
            .await
            .map_err(to_core_error)
    }

    async fn rename_task_list(&self, list_id: &str, new_name: &str) -> CoreResult<()> {
        tasks::rename_task_list(&self.client, list_id, new_name)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await = None;
        Ok(())
    }

    async fn create_task_list(&self, name: &str, parent_id: Option<&str>) -> CoreResult<TaskList> {
        let created = tasks::create_task_list(&self.client, name, parent_id)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await = None;
        Ok(created)
    }

    async fn delete_task_list(&self, list_id: &str) -> CoreResult<()> {
        tasks::delete_task_list(&self.client, list_id)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await = None;
        Ok(())
    }
}

fn to_core_error(err: TodoistError) -> CoreError {
    use TodoistError::*;
    match err {
        Network(m) => CoreError::Network(m),
        Http { status, message } => match status {
            401 | 403 => CoreError::Authentication(message),
            404 => CoreError::NotFound(message),
            409 | 412 => CoreError::Conflict(message),
            _ => CoreError::Protocol(format!("Todoist HTTP {status}: {message}")),
        },
        Protocol(m) => CoreError::Protocol(m),
        Config(m) => CoreError::InvalidInput(m),
    }
}
