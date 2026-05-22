//! Todoist REST API v2 client — `https://api.todoist.com/rest/v2/...`.
//!
//! Auth is a Bearer token the user mints in Todoist's UI under
//! Settings → Integrations → Developer. No OAuth dance is needed
//! for personal accounts; the token is long-lived until the user
//! revokes it.
//!
//! Todoist v2 is a hosted SaaS API — unlike Vikunja or CalDAV
//! there's no server URL to configure. The base URL is hard-coded
//! and a test override sits inside this module for the mockito
//! suite.

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, Response};
use serde::Serialize;
use tracing::debug;

use crate::error::{TodoistError, TodoistResult};

/// Hard-coded production base URL. Tests override via
/// [`TodoistClient::with_base_url_for_tests`].
const PRODUCTION_BASE_URL: &str = "https://api.todoist.com/rest/v2";

/// Shared transport state.
#[derive(Debug, Clone)]
pub struct TodoistClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl TodoistClient {
    /// Build a client with the production base URL.
    pub fn new(token: String, http: reqwest::Client) -> Self {
        Self {
            base_url: PRODUCTION_BASE_URL.to_string(),
            token,
            http,
        }
    }

    /// Test-only constructor that injects an alternative base URL.
    /// Used by the mockito-driven tests to point requests at a
    /// stand-in HTTP server. Stays `#[doc(hidden)]` so production
    /// callers don't accidentally take a dependency on it.
    #[doc(hidden)]
    pub fn with_base_url_for_tests(
        token: String,
        http: reqwest::Client,
        base_url: String,
    ) -> Self {
        Self {
            base_url,
            token,
            http,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn auth_headers(&self) -> TodoistResult<HeaderMap> {
        let mut headers = HeaderMap::new();
        let value = format!("Bearer {}", self.token);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&value).map_err(|e| {
                TodoistError::Config(format!("auth header: {e}"))
            })?,
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        Ok(headers)
    }

    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> TodoistResult<T> {
        let response = self.send(Method::GET, path, Option::<&()>::None).await?;
        decode_json(response).await
    }

    pub async fn post_json<B: Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> TodoistResult<T> {
        let response = self.send(Method::POST, path, Some(body)).await?;
        decode_json(response).await
    }

    /// `POST` without a meaningful response body — used by
    /// `/tasks/{id}/close` and `/reopen`, which return `204 No
    /// Content` on success. Anything 2xx counts; the body (if any)
    /// is discarded.
    pub async fn post_empty(&self, path: &str) -> TodoistResult<()> {
        let response = self
            .send::<()>(Method::POST, path, None)
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        Err(TodoistError::Http {
            status: status.as_u16(),
            message: trim_message(&text),
        })
    }

    pub async fn delete(&self, path: &str) -> TodoistResult<()> {
        let response = self
            .send(Method::DELETE, path, Option::<&()>::None)
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        Err(TodoistError::Http {
            status: status.as_u16(),
            message: trim_message(&text),
        })
    }

    async fn send<B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> TodoistResult<Response> {
        let url = format!("{}{}", self.base_url, path);
        debug!(?method, %url, "todoist request");
        let mut builder =
            self.http.request(method, &url).headers(self.auth_headers()?);
        if let Some(b) = body {
            builder = builder.json(b);
        }
        Ok(builder.send().await?)
    }
}

/// Decode a 2xx JSON body or surface a non-2xx as
/// `TodoistError::Http`.
pub(crate) async fn decode_json<T: serde::de::DeserializeOwned>(
    response: Response,
) -> TodoistResult<T> {
    let status = response.status();
    if status.is_success() {
        let text = response.text().await?;
        return serde_json::from_str(&text).map_err(|e| {
            TodoistError::Protocol(format!(
                "json decode failed: {e} — body started with: {}",
                text.chars().take(120).collect::<String>(),
            ))
        });
    }
    let text = response.text().await.unwrap_or_default();
    Err(TodoistError::Http {
        status: status.as_u16(),
        message: trim_message(&text),
    })
}

fn trim_message(text: &str) -> String {
    text.chars().take(300).collect()
}
