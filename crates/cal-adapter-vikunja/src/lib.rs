//! Vikunja tasks adapter — Phase 6g.
//!
//! Vikunja is a self-hosted Open-Source task manager (AGPLv3) with a
//! REST API. The adapter declares only `Capability::Tasks` — Vikunja
//! has no calendar surface — and authenticates with a user-minted
//! API token sent as a Bearer credential. No OAuth dance, no JWT
//! refresh logic: the user mints a long-lived token in Vikunja's
//! own UI (Settings → API Tokens), pastes it into Aperio's account
//! dialog, and we use it on every request.
//!
//! Licence note: per DESIGN.md the adapter is a pure REST client.
//! No Vikunja source code is linked or shipped, which keeps the
//! crate cleanly under Aperio's own licence terms despite Vikunja
//! itself being AGPLv3.
//!
//! Scope of Phase 6g.1:
//!
//!   - List the user's Vikunja projects as Aperio task lists
//!   - Read every task in a project, paginated
//!   - Create / update / delete tasks
//!   - Rename a project
//!
//! See `tasks.rs` for the mapping decisions (status, dates,
//! priority) and the list of fields that are intentionally not
//! round-tripped yet (recurrence, reminders, subtasks, labels).

pub mod api;
pub mod error;
pub mod tasks;

use std::time::Duration;

use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Capability, Credentials as CoreCredentials, Error as CoreError, NewTask,
    Result as CoreResult, Section, Task, TaskList, TaskUser, TasksFeature,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

pub use api::VikunjaClient;
pub use error::{VikunjaError, VikunjaResult};

/// Persisted, non-secret half of a Vikunja account's configuration.
/// The API token itself lives in the platform keychain
/// (`SecretSlot::ApiToken`); only the server URL travels through the
/// `accounts` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VikunjaAccountConfig {
    /// Server root URL, e.g. `https://try.vikunja.io`. The client
    /// canonicalises this — the user can paste with or without
    /// trailing slash, and `/api/v1` is accepted as a no-op suffix.
    pub server_url: String,
    /// Optional human-readable label captured at create time (e.g.
    /// the username displayed in Vikunja). Used only to enrich the
    /// account row's display; not consulted at request time.
    #[serde(default)]
    pub account_label: Option<String>,
}

#[derive(Debug)]
pub struct VikunjaAdapter {
    client: VikunjaClient,
    capabilities: Vec<Capability>,
    /// 5-minute listing-cache, mirroring the CalDAV / EWS / Graph
    /// adapters. Vikunja's projects endpoint is fast (a few hundred
    /// ms over HTTPS) but the sidebar calls it from several refresh
    /// paths — caching keeps tab switches snappy.
    task_lists_cache: Mutex<Option<(Vec<TaskList>, chrono::DateTime<chrono::Utc>)>>,
    listing_ttl: chrono::Duration,
}

impl VikunjaAdapter {
    /// Build the adapter. `server_url` is the URL the user pasted
    /// into the dialog; `token` is the API token (treated as
    /// opaque). Returns `Err` if the URL is malformed.
    pub fn new(server_url: &str, token: String) -> VikunjaResult<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| VikunjaError::Config(format!("http client: {e}")))?;
        Ok(Self {
            client: VikunjaClient::new(server_url, token, http)?,
            capabilities: vec![Capability::Tasks],
            task_lists_cache: Mutex::new(None),
            listing_ttl: chrono::Duration::minutes(5),
        })
    }

    #[doc(hidden)]
    pub fn with_listing_ttl(mut self, ttl: Duration) -> Self {
        self.listing_ttl =
            chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::zero());
        self
    }

    /// Returns the canonicalised server root the client is talking
    /// to. Useful for the connect-account flow when it wants to log
    /// the actual URL the adapter ended up with.
    pub fn server_root(&self) -> &str {
        self.client.server_root()
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

    /// One-shot reachability + auth probe. Hits `GET /projects` with
    /// `per_page=1` — anything 2xx means the token is accepted and
    /// the server responded. Surfaces the same `cal_core::Error`
    /// taxonomy the rest of the adapter uses so the command layer
    /// can format the message uniformly.
    pub async fn smoke_test(&self) -> CoreResult<()> {
        // Going through the same client path means we exercise the
        // exact request shape the listing call uses — different
        // path from the live one (just 1 row instead of 50), but
        // the headers, base URL, and auth go through the same
        // codepath. Decoding fails on non-JSON (proxy login page,
        // …) which is also useful info.
        let _: serde_json::Value = self
            .client
            .get_json("/projects?page=1&per_page=1")
            .await
            .map_err(to_core_error)?;
        Ok(())
    }
}

#[async_trait]
impl Adapter for VikunjaAdapter {
    async fn authenticate(&self, _credentials: CoreCredentials) -> CoreResult<AuthToken> {
        // Vikunja auth is pre-baked: the API token came in at
        // construction time and lives inside the `VikunjaClient`.
        // The trait method is a no-op stub — nothing to do, nothing
        // to refresh.
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl TasksFeature for VikunjaAdapter {
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
        // Drop the cache so the next listing call sees the new
        // title; same trick the CalDAV / Google adapters use.
        *self.task_lists_cache.lock().await = None;
        Ok(())
    }

    async fn create_task_list(&self, name: &str, parent_id: Option<&str>) -> CoreResult<TaskList> {
        let created = tasks::create_task_list(&self.client, name, parent_id)
            .await
            .map_err(to_core_error)?;
        // The list set changed — drop the cache so the next listing
        // includes the new project.
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

    async fn list_task_list_members(&self, list_id: &str) -> CoreResult<Vec<TaskUser>> {
        tasks::list_task_list_members(&self.client, list_id)
            .await
            .map_err(to_core_error)
    }

    async fn current_user(&self) -> CoreResult<Option<TaskUser>> {
        tasks::current_user(&self.client)
            .await
            .map_err(to_core_error)
    }
}

fn to_core_error(err: VikunjaError) -> CoreError {
    use VikunjaError::*;
    match err {
        Network(m) => CoreError::Network(m),
        Http { status, message } => match status {
            401 | 403 => CoreError::Authentication(message),
            404 => CoreError::NotFound(message),
            409 | 412 => CoreError::Conflict(message),
            _ => CoreError::Protocol(format!("Vikunja HTTP {status}: {message}")),
        },
        Protocol(m) => CoreError::Protocol(m),
        Config(m) => CoreError::InvalidInput(m),
    }
}
