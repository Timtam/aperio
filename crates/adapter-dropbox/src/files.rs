//! Thin wrappers around the Dropbox API v2 file endpoints.
//!
//! Two host families:
//!
//! - **`api.dropboxapi.com`** for JSON-RPC calls
//!   (`list_folder`, `delete_v2`, `create_folder_v2`,
//!   `check/user`).
//! - **`content.dropboxapi.com`** for binary up- and downloads
//!   (`upload`, `download`). These use a special
//!   `Dropbox-API-Arg` header carrying a JSON snippet instead
//!   of a body argument.
//!
//! Every wrapper takes the access token as a parameter rather
//! than fetching it itself. That keeps this module testable
//! with a static token + a mockito server, and pushes the
//! refresh-on-401 retry into the trait method bodies in
//! `lib.rs`.

use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::json;

use crate::error::{DropboxError, DropboxResult};

const API_HOST: &str = "https://api.dropboxapi.com";
const CONTENT_HOST: &str = "https://content.dropboxapi.com";

/// Probe that the access token actually works against
/// `/2/check/user` — Dropbox's official "auth probe" endpoint.
/// Returns Ok on 200 + matching `result` body; surfaces 401 as
/// `DropboxError::Auth`.
pub async fn check_user(http: &Client, access_token: &str) -> DropboxResult<()> {
    let response = http
        .post(format!("{API_HOST}/2/check/user"))
        .bearer_auth(access_token)
        .header("content-type", "application/json")
        .body(r#"{"query":"aperio"}"#.to_string())
        .send()
        .await?;
    rpc_no_body(response, "check/user").await
}

/// `POST /2/files/create_folder_v2`. Idempotent in our usage:
/// `path/conflict/folder` (already exists) is folded into Ok
/// so callers can call it unconditionally on every sync round
/// without an exists-probe first.
pub async fn create_folder(http: &Client, access_token: &str, path: &str) -> DropboxResult<()> {
    // Dropbox refuses MKD on the literal root with
    // `path/cant_move_folder_into_itself`; skip the call when
    // the user wants the root folder.
    if path.is_empty() {
        return Ok(());
    }
    let body = json!({
        "path": path,
        "autorename": false,
    });
    let response = http
        .post(format!("{API_HOST}/2/files/create_folder_v2"))
        .bearer_auth(access_token)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await?;
    match rpc_no_body(response, "create_folder_v2").await {
        Ok(()) => Ok(()),
        // The dropbox error payload looks like
        // `{"error_summary":"path/conflict/folder/..","error":{...}}` —
        // map that to Ok since the caller's intent (make sure
        // the folder is there) is already satisfied.
        Err(DropboxError::Protocol(msg))
            if msg.contains("path/conflict") || msg.contains("already exists") =>
        {
            Ok(())
        }
        Err(other) => Err(other),
    }
}

/// `POST /2/files/upload` (content host). Uses
/// `mode: "overwrite"` so the existing file at the same path
/// is replaced atomically server-side — no tmp+rename dance
/// needed.
pub async fn upload(
    http: &Client,
    access_token: &str,
    path: &str,
    bytes: &[u8],
) -> DropboxResult<()> {
    let arg = json!({
        "path": path,
        "mode": "overwrite",
        "autorename": false,
        "mute": true,
        "strict_conflict": false,
    });
    let response = http
        .post(format!("{CONTENT_HOST}/2/files/upload"))
        .bearer_auth(access_token)
        .header("Dropbox-API-Arg", arg.to_string())
        .header("content-type", "application/octet-stream")
        .body(bytes.to_vec())
        .send()
        .await?;
    rpc_no_body(response, "upload").await
}

/// `POST /2/files/download` (content host). Returns the bytes
/// on success, `Ok(None)` on `path/not_found`, an error
/// otherwise.
pub async fn download(
    http: &Client,
    access_token: &str,
    path: &str,
) -> DropboxResult<Option<Vec<u8>>> {
    let arg = json!({ "path": path });
    let response = http
        .post(format!("{CONTENT_HOST}/2/files/download"))
        .bearer_auth(access_token)
        .header("Dropbox-API-Arg", arg.to_string())
        .send()
        .await?;
    let status = response.status();
    if status == StatusCode::OK {
        let bytes = response.bytes().await?;
        return Ok(Some(bytes.to_vec()));
    }
    // 409 Conflict is Dropbox's "your request was well-formed
    // but the path is wrong" status. The detail lives in the
    // body's JSON `.error_summary`.
    if status == StatusCode::CONFLICT {
        let text = response.text().await.unwrap_or_default();
        if text.contains("path/not_found") {
            return Ok(None);
        }
        return Err(classify_response_text(status.as_u16(), &text));
    }
    if status == StatusCode::UNAUTHORIZED {
        let text = response.text().await.unwrap_or_default();
        return Err(DropboxError::Auth(text));
    }
    let text = response.text().await.unwrap_or_default();
    Err(classify_response_text(status.as_u16(), &text))
}

/// `POST /2/files/delete_v2`. Idempotent semantics handled
/// upstream — the SyncAdapter wrapper folds `path/not_found`
/// into success.
pub async fn delete(http: &Client, access_token: &str, path: &str) -> DropboxResult<()> {
    let body = json!({ "path": path });
    let response = http
        .post(format!("{API_HOST}/2/files/delete_v2"))
        .bearer_auth(access_token)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await?;
    rpc_no_body(response, "delete_v2").await
}

/// One page of a `list_folder` / `list_folder/continue`
/// response. Module-scoped (rather than local to the fn) so the
/// parse is unit-testable against a captured response fixture.
#[derive(Deserialize)]
struct ListResponse {
    entries: Vec<Entry>,
    cursor: String,
    has_more: bool,
}

/// One listing entry. `.tag` discriminates files from folders;
/// `size` is present on every `FileMetadata` (file) row and
/// absent on folders. The size feeds the cursor's growth-refetch
/// check and MUST stay the raw remote byte count — under E2E
/// that's ciphertext, and `EncryptingAdapter` translates the
/// cursor's `known_lengths` into the same domain before this
/// adapter compares them.
#[derive(Deserialize)]
struct Entry {
    #[serde(rename = ".tag")]
    tag: String,
    name: String,
    #[serde(default)]
    size: Option<u64>,
}

/// Append one page's file rows (basename + listed size) to
/// `out`, skipping folder rows.
fn collect_files(page: &ListResponse, out: &mut Vec<(String, Option<u64>)>) {
    for entry in &page.entries {
        if entry.tag == "file" {
            out.push((entry.name.clone(), entry.size));
        }
    }
}

/// `POST /2/files/list_folder`. Walks pagination internally:
/// keeps issuing `/list_folder/continue` while the response's
/// `has_more` is `true`. Returns the filenames (basenames)
/// paired with the byte size Dropbox natively lists for every
/// file — the caller's `wants_sized` growth check needs it.
pub async fn list_folder(
    http: &Client,
    access_token: &str,
    folder: &str,
) -> DropboxResult<Vec<(String, Option<u64>)>> {
    let mut names = Vec::new();
    let initial = json!({
        "path": folder,
        "recursive": false,
        "include_media_info": false,
        "include_deleted": false,
        "include_has_explicit_shared_members": false,
        "include_mounted_folders": false,
        "limit": 2000,
    });
    let mut current: ListResponse = post_rpc_json(
        http,
        access_token,
        "/2/files/list_folder",
        initial.to_string(),
    )
    .await?;
    collect_files(&current, &mut names);
    while current.has_more {
        let body = json!({ "cursor": current.cursor }).to_string();
        current = post_rpc_json(http, access_token, "/2/files/list_folder/continue", body).await?;
        collect_files(&current, &mut names);
    }
    Ok(names)
}

// ─────────────────────────────────────────────────────────────────
// Low-level HTTP helpers
// ─────────────────────────────────────────────────────────────────

/// Post a JSON body to a Dropbox API host endpoint and
/// deserialise the response into `T`. Maps non-2xx to the right
/// [`DropboxError`] variant.
async fn post_rpc_json<T: for<'de> Deserialize<'de>>(
    http: &Client,
    access_token: &str,
    path: &str,
    body: String,
) -> DropboxResult<T> {
    let response = http
        .post(format!("{API_HOST}{path}"))
        .bearer_auth(access_token)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if status.is_success() {
        return serde_json::from_str(&text).map_err(|e| {
            DropboxError::Protocol(format!(
                "decode {path} body: {e}; raw: {head}",
                head = text.chars().take(200).collect::<String>(),
            ))
        });
    }
    if status == StatusCode::UNAUTHORIZED {
        return Err(DropboxError::Auth(text));
    }
    Err(classify_response_text(status.as_u16(), &text))
}

/// Drop the body, just check the response succeeded. Used by
/// endpoints whose successful result we don't care about
/// (upload, create_folder_v2, delete_v2).
async fn rpc_no_body(response: reqwest::Response, label: &str) -> DropboxResult<()> {
    let status = response.status();
    if status.is_success() {
        // Drain the body so the connection can be reused;
        // we don't parse it.
        let _ = response.bytes().await;
        return Ok(());
    }
    if status == StatusCode::UNAUTHORIZED {
        let text = response.text().await.unwrap_or_default();
        return Err(DropboxError::Auth(format!("{label}: {text}")));
    }
    let text = response.text().await.unwrap_or_default();
    Err(classify_response_text(status.as_u16(), &text))
}

/// Inspect a Dropbox error payload body and pick the right
/// `DropboxError` variant. The payload format is
/// `{"error_summary": "path/not_found/...", "error": {...}}`
/// for the 4xx cases; we substring-match on the summary
/// because the `.tag` discriminator is nested.
fn classify_response_text(status: u16, text: &str) -> DropboxError {
    let lower = text.to_lowercase();
    if lower.contains("path/not_found") || lower.contains("not_found") {
        return DropboxError::NotFound(text.chars().take(300).collect());
    }
    if lower.contains("invalid_access_token") || lower.contains("expired_access_token") {
        return DropboxError::Auth(text.chars().take(300).collect());
    }
    // 4xx with a body that looks like a Dropbox error payload
    // → surface as Protocol so the call site can decide what
    // to do. Other 4xx + 5xx → Http for upstream surfacing.
    if (400..500).contains(&status) && lower.contains("error_summary") {
        return DropboxError::Protocol(text.chars().take(300).collect());
    }
    DropboxError::Http {
        status,
        message: text.chars().take(300).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_folder_page_parses_the_native_size_field() {
        // Trimmed from a real /2/files/list_folder response.
        // Every file entry carries `size` (the growth-refetch
        // signal); folder entries never do and are dropped.
        let body = r#"{
            "entries": [
                {
                    ".tag": "file",
                    "name": "2026-05-01T00-00-00Z_dev-a.jsonl",
                    "path_lower": "/aperio/log/2026-05-01t00-00-00z_dev-a.jsonl",
                    "id": "id:a4ayc_80_OEAAAAAAAAAXw",
                    "server_modified": "2026-05-01T00:00:07Z",
                    "rev": "015f0f1a3b",
                    "size": 1234,
                    "content_hash": "cafe"
                },
                {
                    ".tag": "folder",
                    "name": "nested",
                    "path_lower": "/aperio/log/nested",
                    "id": "id:b5bzd_91_PFBBBBBBBBBYx"
                }
            ],
            "cursor": "AAaaExampleCursor",
            "has_more": false
        }"#;
        let page: ListResponse = serde_json::from_str(body).expect("fixture parses");
        let mut files = Vec::new();
        collect_files(&page, &mut files);
        assert_eq!(
            files,
            vec![("2026-05-01T00-00-00Z_dev-a.jsonl".to_string(), Some(1234))],
        );
        assert!(!page.has_more);
    }

    #[test]
    fn classify_not_found_picks_notfound_variant() {
        let body = r#"{"error_summary":"path/not_found/...","error":{".tag":"path","path":{".tag":"not_found"}}}"#;
        let err = classify_response_text(409, body);
        assert!(matches!(err, DropboxError::NotFound(_)));
    }

    #[test]
    fn classify_invalid_token_picks_auth_variant() {
        let body = r#"{"error_summary":"invalid_access_token/...","error":{".tag":"invalid_access_token"}}"#;
        let err = classify_response_text(401, body);
        assert!(matches!(err, DropboxError::Auth(_)));
    }

    #[test]
    fn classify_unknown_4xx_picks_protocol_variant() {
        let body = r#"{"error_summary":"path/disallowed_name/...","error":{}}"#;
        let err = classify_response_text(409, body);
        assert!(matches!(err, DropboxError::Protocol(_)));
    }

    #[test]
    fn classify_plain_http_5xx_picks_http_variant() {
        let body = "<html>500 server error</html>";
        let err = classify_response_text(500, body);
        assert!(matches!(err, DropboxError::Http { status: 500, .. }));
    }
}
