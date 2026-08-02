//! Thin wrappers around the Google Drive API v3.
//!
//! Drive doesn't address files by path the way Dropbox does —
//! every file has an opaque ID and the relationship to other
//! files lives in a `parents[]` array on each metadata
//! record. This module hides that asymmetry: the public
//! surface takes a `parent_id` + `name` pair, returns either
//! a file id or `Ok(None)` for not-found, and the upper layer
//! in `lib.rs` maps Aperio's path semantics
//! (`meta.json`, `log/<name>.jsonl`, …) onto those ID-based
//! ops with a small in-memory folder-ID cache.
//!
//! Endpoint families:
//!
//! - **api**: `https://www.googleapis.com/drive/v3/...`
//!   Metadata, listing, delete, get-content via `?alt=media`.
//! - **upload**: `https://www.googleapis.com/upload/drive/v3/...`
//!   Multipart create + media PATCH.

use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::json;

use super::error::{GoogleDriveError, GoogleDriveResult};

const API_BASE: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD_BASE: &str = "https://www.googleapis.com/upload/drive/v3";

/// MIME type Google Drive uses for folders (it's a special
/// non-file kind in their data model).
pub const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

/// Probe the access token via `GET /about?fields=user`. Cheap
/// "does the token still work" check used by `test_connection`.
pub async fn check_user(http: &Client, access_token: &str) -> GoogleDriveResult<()> {
    let response = http
        .get(format!("{API_BASE}/about?fields=user"))
        .bearer_auth(access_token)
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        let _ = response.bytes().await;
        return Ok(());
    }
    if status == StatusCode::UNAUTHORIZED {
        let text = response.text().await.unwrap_or_default();
        return Err(GoogleDriveError::Auth(text));
    }
    let text = response.text().await.unwrap_or_default();
    Err(classify_response_text(status.as_u16(), &text))
}

/// Find a child file by `name` under `parent_id`. Uses a `q=`
/// query so we don't have to paginate the whole folder.
/// `Ok(None)` when nothing matches; `Ok(Some(id))` for the
/// first match.
pub async fn find_child(
    http: &Client,
    access_token: &str,
    parent_id: &str,
    name: &str,
) -> GoogleDriveResult<Option<String>> {
    // Escape single quotes in `name` per Drive's query
    // grammar (RFC 3986-style — single quotes are the only
    // sensitive character).
    let escaped_name = name.replace('\'', "\\'");
    let q = format!("name = '{escaped_name}' and '{parent_id}' in parents and trashed = false");
    let response = http
        .get(format!("{API_BASE}/files"))
        .bearer_auth(access_token)
        .query(&[
            ("q", q.as_str()),
            ("fields", "files(id,name)"),
            ("pageSize", "1"),
            ("spaces", "drive"),
        ])
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if status == StatusCode::UNAUTHORIZED {
        return Err(GoogleDriveError::Auth(text));
    }
    if !status.is_success() {
        return Err(classify_response_text(status.as_u16(), &text));
    }
    #[derive(Deserialize)]
    struct ListResponse {
        files: Vec<FileEntry>,
    }
    #[derive(Deserialize)]
    struct FileEntry {
        id: String,
    }
    let parsed: ListResponse = serde_json::from_str(&text)
        .map_err(|e| GoogleDriveError::Protocol(format!("decode find_child: {e}")))?;
    Ok(parsed.files.into_iter().next().map(|f| f.id))
}

/// One entry from a [`list_children`] listing: everything the
/// log-fetch path needs so it never has to re-query per file —
/// `id` feeds the direct `?alt=media` download and `size` feeds
/// the cursor's growth-refetch check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveEntry {
    pub id: String,
    pub name: String,
    /// Raw stored byte count. Drive serialises this as a decimal
    /// STRING in JSON (int64 doesn't fit in a JS number); Docs-native
    /// files omit it entirely. Under E2E these are ciphertext bytes —
    /// exactly the domain the encrypting layer translates the
    /// cursor's known_lengths into, so the value is passed through
    /// raw, never adjusted.
    pub size: Option<u64>,
}

/// List every file (not folder) under `parent_id` with its id +
/// size. Walks pagination internally via `nextPageToken`.
/// `Ok(empty)` when the folder exists but is empty;
/// `Err(NotFound)` when the folder doesn't exist.
pub async fn list_children(
    http: &Client,
    access_token: &str,
    parent_id: &str,
) -> GoogleDriveResult<Vec<DriveEntry>> {
    let mut out = Vec::new();
    let mut page_token: Option<String> = None;
    let q = format!("'{parent_id}' in parents and trashed = false and mimeType != '{FOLDER_MIME}'");
    loop {
        let mut params: Vec<(&str, &str)> = vec![
            ("q", q.as_str()),
            ("fields", "files(id,name,size),nextPageToken"),
            ("pageSize", "1000"),
            ("spaces", "drive"),
        ];
        if let Some(tok) = page_token.as_deref() {
            params.push(("pageToken", tok));
        }
        let response = http
            .get(format!("{API_BASE}/files"))
            .bearer_auth(access_token)
            .query(&params)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status == StatusCode::UNAUTHORIZED {
            return Err(GoogleDriveError::Auth(text));
        }
        if status == StatusCode::NOT_FOUND {
            return Err(GoogleDriveError::NotFound(text));
        }
        if !status.is_success() {
            return Err(classify_response_text(status.as_u16(), &text));
        }
        let (entries, next_page_token) = parse_list_page(&text)?;
        out.extend(entries);
        match next_page_token {
            Some(t) if !t.is_empty() => page_token = Some(t),
            _ => break,
        }
    }
    Ok(out)
}

/// Decode one `files.list` response page into entries + the
/// pagination token. Split out of the HTTP loop so the `size`
/// handling is unit-testable without a server.
fn parse_list_page(text: &str) -> GoogleDriveResult<(Vec<DriveEntry>, Option<String>)> {
    #[derive(Deserialize)]
    struct ListResponse {
        files: Vec<FileEntry>,
        #[serde(default)]
        #[serde(rename = "nextPageToken")]
        next_page_token: Option<String>,
    }
    #[derive(Deserialize)]
    struct FileEntry {
        id: String,
        name: String,
        #[serde(default, deserialize_with = "de_size")]
        size: Option<u64>,
    }
    let parsed: ListResponse = serde_json::from_str(text)
        .map_err(|e| GoogleDriveError::Protocol(format!("decode list: {e}")))?;
    let entries = parsed
        .files
        .into_iter()
        .map(|f| DriveEntry {
            id: f.id,
            name: f.name,
            size: f.size,
        })
        .collect();
    Ok((entries, parsed.next_page_token))
}

/// Drive serialises `size` as a decimal string (`"size": "123"`).
/// Accept a bare number too (defensive — other Google APIs differ)
/// and fold anything unparseable into `None`: the growth-refetch
/// check treats a missing size as "no growth signal", which is safe
/// — the file is still fetched once it rises above the cursor, the
/// refetch is merely deferred to session rotation. Deserialising
/// via `serde_json::Value` keeps the fold TOTAL: a float, a
/// negative, or any other odd shape degrades to `None` instead of
/// failing the whole listing page (fail-fast would serve stale
/// indefinitely over a field we can safely do without).
fn de_size<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::Number(n)) => n.as_u64(),
            Some(serde_json::Value::String(s)) => s.parse().ok(),
            _ => None,
        },
    )
}

/// Create or update a file. If `existing_id` is `Some`, PATCH
/// the file's content in place; otherwise POST a multipart
/// "create" with metadata + bytes. Either way returns the
/// file id so the caller can cache it.
pub async fn upload(
    http: &Client,
    access_token: &str,
    parent_id: &str,
    name: &str,
    bytes: &[u8],
    existing_id: Option<&str>,
) -> GoogleDriveResult<String> {
    if let Some(id) = existing_id {
        // PATCH content via /upload/drive/v3/files/{id}?uploadType=media
        let response = http
            .patch(format!("{UPLOAD_BASE}/files/{id}?uploadType=media",))
            .bearer_auth(access_token)
            .header("content-type", "application/octet-stream")
            .body(bytes.to_vec())
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status == StatusCode::UNAUTHORIZED {
            return Err(GoogleDriveError::Auth(text));
        }
        if !status.is_success() {
            return Err(classify_response_text(status.as_u16(), &text));
        }
        return Ok(id.to_string());
    }

    // New file: multipart upload with metadata + content.
    // Drive's multipart envelope is a strict bytes layout
    // — two parts separated by a boundary marker, JSON
    // metadata first then octet stream.
    let boundary = "aperio_gd_boundary_8x7n4p2q";
    let metadata = json!({
        "name": name,
        "parents": [parent_id],
    })
    .to_string();
    let mut body = Vec::with_capacity(bytes.len() + metadata.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(metadata.as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--").as_bytes());

    let response = http
        .post(format!(
            "{UPLOAD_BASE}/files?uploadType=multipart&fields=id",
        ))
        .bearer_auth(access_token)
        .header(
            "content-type",
            format!("multipart/related; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if status == StatusCode::UNAUTHORIZED {
        return Err(GoogleDriveError::Auth(text));
    }
    if !status.is_success() {
        return Err(classify_response_text(status.as_u16(), &text));
    }
    #[derive(Deserialize)]
    struct CreateResponse {
        id: String,
    }
    let parsed: CreateResponse = serde_json::from_str(&text)
        .map_err(|e| GoogleDriveError::Protocol(format!("decode upload response: {e}")))?;
    Ok(parsed.id)
}

/// Download the bytes of a file by ID via `?alt=media`.
/// `Ok(None)` on 404; `Err(Auth)` on 401; anything else as a
/// classified error.
pub async fn download(
    http: &Client,
    access_token: &str,
    file_id: &str,
) -> GoogleDriveResult<Option<Vec<u8>>> {
    let response = http
        .get(format!("{API_BASE}/files/{file_id}?alt=media"))
        .bearer_auth(access_token)
        .send()
        .await?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if status == StatusCode::UNAUTHORIZED {
        let text = response.text().await.unwrap_or_default();
        return Err(GoogleDriveError::Auth(text));
    }
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(classify_response_text(status.as_u16(), &text));
    }
    let bytes = response.bytes().await?;
    Ok(Some(bytes.to_vec()))
}

/// Delete a file by ID. 404 is folded into success so the
/// caller's "make sure it's gone" semantic matches the
/// SFTP / FTP / Dropbox adapters.
pub async fn delete(http: &Client, access_token: &str, file_id: &str) -> GoogleDriveResult<()> {
    let response = http
        .delete(format!("{API_BASE}/files/{file_id}"))
        .bearer_auth(access_token)
        .send()
        .await?;
    let status = response.status();
    if status == StatusCode::NO_CONTENT || status.is_success() {
        let _ = response.bytes().await;
        return Ok(());
    }
    if status == StatusCode::NOT_FOUND {
        let _ = response.bytes().await;
        return Ok(());
    }
    if status == StatusCode::UNAUTHORIZED {
        let text = response.text().await.unwrap_or_default();
        return Err(GoogleDriveError::Auth(text));
    }
    let text = response.text().await.unwrap_or_default();
    Err(classify_response_text(status.as_u16(), &text))
}

/// Create a folder under `parent_id` (or under "root" / the
/// user's My Drive when `parent_id == "root"`). Returns the
/// new folder's ID. If a folder with the same name already
/// exists under that parent, returns its existing ID
/// (idempotent — saves the caller from doing
/// find-then-create twice).
pub async fn create_folder(
    http: &Client,
    access_token: &str,
    parent_id: &str,
    name: &str,
) -> GoogleDriveResult<String> {
    if let Some(existing) = find_child(http, access_token, parent_id, name).await? {
        return Ok(existing);
    }
    let metadata = json!({
        "name": name,
        "mimeType": FOLDER_MIME,
        "parents": [parent_id],
    });
    let response = http
        .post(format!("{API_BASE}/files?fields=id"))
        .bearer_auth(access_token)
        .header("content-type", "application/json")
        .body(metadata.to_string())
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if status == StatusCode::UNAUTHORIZED {
        return Err(GoogleDriveError::Auth(text));
    }
    if !status.is_success() {
        return Err(classify_response_text(status.as_u16(), &text));
    }
    #[derive(Deserialize)]
    struct CreateResponse {
        id: String,
    }
    let parsed: CreateResponse = serde_json::from_str(&text)
        .map_err(|e| GoogleDriveError::Protocol(format!("decode create_folder: {e}")))?;
    Ok(parsed.id)
}

/// Inspect a Drive error payload and pick the right
/// `GoogleDriveError` variant. The payload format is
/// `{"error":{"code":404,"message":"File not found: …","errors":[…]}}`.
fn classify_response_text(status: u16, text: &str) -> GoogleDriveError {
    let lower = text.to_lowercase();
    if status == 404 || lower.contains("\"code\": 404") || lower.contains("not found") {
        return GoogleDriveError::NotFound(text.chars().take(300).collect());
    }
    if status == 401
        || lower.contains("invalid_token")
        || lower.contains("invalid_grant")
        || lower.contains("invalid credentials")
    {
        return GoogleDriveError::Auth(text.chars().take(300).collect());
    }
    if (400..500).contains(&status) {
        return GoogleDriveError::Protocol(text.chars().take(300).collect());
    }
    GoogleDriveError::Http {
        status,
        message: text.chars().take(300).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_404_picks_notfound() {
        let err = classify_response_text(
            404,
            r#"{"error":{"code":404,"message":"File not found: abc"}}"#,
        );
        assert!(matches!(err, GoogleDriveError::NotFound(_)));
    }

    #[test]
    fn classify_401_picks_auth() {
        let err =
            classify_response_text(401, r#"{"error":{"code":401,"message":"invalid_token"}}"#);
        assert!(matches!(err, GoogleDriveError::Auth(_)));
    }

    #[test]
    fn classify_400_picks_protocol() {
        let err = classify_response_text(400, r#"{"error":{"code":400,"message":"Bad Request"}}"#);
        assert!(matches!(err, GoogleDriveError::Protocol(_)));
    }

    #[test]
    fn classify_500_picks_http() {
        let err = classify_response_text(500, "<html>Internal Server Error</html>");
        assert!(matches!(err, GoogleDriveError::Http { status: 500, .. }));
    }

    #[test]
    fn list_page_parses_drive_string_sizes() {
        // Drive's actual wire shape: size is a decimal STRING.
        let (entries, next) = parse_list_page(
            r#"{"files":[{"id":"a1","name":"x.jsonl","size":"12345"}],"nextPageToken":"tok"}"#,
        )
        .unwrap();
        assert_eq!(
            entries,
            vec![DriveEntry {
                id: "a1".into(),
                name: "x.jsonl".into(),
                size: Some(12_345),
            }]
        );
        assert_eq!(next.as_deref(), Some("tok"));
    }

    #[test]
    fn list_page_tolerates_numeric_missing_and_junk_sizes() {
        let (entries, next) = parse_list_page(
            r#"{"files":[
                {"id":"a","name":"n.jsonl","size":77},
                {"id":"b","name":"sizeless"},
                {"id":"c","name":"weird.jsonl","size":"not-a-number"},
                {"id":"d","name":"floaty.jsonl","size":123.0},
                {"id":"e","name":"negative.jsonl","size":-5},
                {"id":"f","name":"bool.jsonl","size":true}
            ]}"#,
        )
        .unwrap();
        assert_eq!(entries.len(), 6);
        assert_eq!(entries[0].size, Some(77));
        assert_eq!(entries[1].size, None);
        assert_eq!(entries[2].size, None);
        // Numeric-but-not-u64 (and outright junk) shapes degrade to
        // None — timestamp-only semantics — instead of failing the
        // whole listing page.
        assert_eq!(entries[3].size, None);
        assert_eq!(entries[4].size, None);
        assert_eq!(entries[5].size, None);
        assert!(next.is_none());
    }

    #[test]
    fn list_page_decode_failure_is_protocol_error() {
        let err = parse_list_page("<html>gateway timeout</html>").unwrap_err();
        assert!(matches!(err, GoogleDriveError::Protocol(_)));
    }
}
