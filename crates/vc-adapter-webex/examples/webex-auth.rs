//! Interactive Webex sign-in, run by hand against a real account.
//!
//! Two jobs. It walks the whole OAuth flow end to end — consent screen, fixed
//! loopback redirect, token exchange, refresh — so the code is proven against
//! Cisco's live server before any meeting logic is built on it. And it settles
//! the one question Cisco's documentation does not answer: **is a client secret
//! actually required when PKCE is used?** It tries the public-client posture
//! first and only falls back to sending the secret if Webex refuses.
//!
//! ```text
//! cargo run -p vc-adapter-webex --example webex-auth
//! ```
//!
//! Credentials come from the same place every build reads them: the
//! `APERIO_OAUTH_WEBEX_CLIENT_ID` / `_CLIENT_SECRET` environment variables, or
//! `oauth-clients.local.env` in the repository root. This example reads that
//! file directly rather than through `builtin-oauth`, so it works without a
//! rebuild when the values change.
//!
//! Nothing is stored. Tokens are printed as lengths and expiry times, never as
//! values — the output of this program tends to end up pasted into a chat.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vc_adapter_webex::oauth;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let Some((client_id, client_secret)) = load_credentials() else {
        eprintln!(
            "No Webex credentials found.\n\n\
             Put them in `oauth-clients.local.env` in the repository root:\n\n\
             \x20   APERIO_OAUTH_WEBEX_CLIENT_ID=…\n\
             \x20   APERIO_OAUTH_WEBEX_CLIENT_SECRET=…\n\n\
             or export the same two variables. The file is gitignored."
        );
        std::process::exit(2);
    };

    println!("Client id ends in …{}", tail(&client_id));
    println!(
        "Client secret: {}",
        match &client_secret {
            Some(s) => format!("present, {} characters", s.len()),
            None => "absent — the public-client posture will be the only one tried".to_string(),
        }
    );
    println!("Redirect      {}", oauth::loopback_redirect_uri());
    println!("Scopes        {}", oauth::SCOPES);
    println!();
    println!("A browser window will open. Sign in and approve the request.");
    println!("If nothing opens, the URL is printed below — paste it by hand.");
    println!();

    let http = reqwest::Client::new();

    // Phase 1: consent + the authorization code. Runs once; the code is
    // single-use, so both token postures cannot be tried on one code.
    let captured = match capture_code(&client_id).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("\nSign-in failed: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "\nGot an authorization code ({} characters).",
        captured.code.len()
    );

    // Phase 2: the experiment. Public client first.
    println!("\nTrying the token exchange WITHOUT a client secret (PKCE only)…");
    let public_attempt = oauth::exchange_code(
        &http,
        oauth::WEBEX_TOKEN_URL,
        &client_id,
        None,
        &captured.code,
        &captured.verifier,
        &captured.redirect_uri,
    )
    .await;

    let tokens = match public_attempt {
        Ok(tokens) => {
            println!(
                "\n  ANSWERED: Webex ACCEPTS a public client. A shipped build does not need\n\
                 \x20 to carry a secret at all — Provider::Webex::requires_secret() in\n\
                 \x20 crates/builtin-oauth should become false."
            );
            tokens
        }
        Err(without_secret) => {
            // NOT a conclusion yet. A refusal here could be the missing secret,
            // or a spent code, a mistyped client id, a redirect mismatch, or
            // Webex having a bad minute. Only a retry that succeeds WITH the
            // secret, having changed nothing else, makes it evidence.
            println!("  refused: {without_secret}");
            let Some(secret) = client_secret.as_deref() else {
                eprintln!(
                    "\n  INCONCLUSIVE: no secret is configured, so this run cannot tell\n\
                     \x20 whether the refusal was about the missing secret or about\n\
                     \x20 something else entirely. Add APERIO_OAUTH_WEBEX_CLIENT_SECRET\n\
                     \x20 and run again."
                );
                std::process::exit(1);
            };
            println!(
                "\n  Retrying WITH the secret, changing nothing else. A fresh consent round\n\
                 \x20 is needed because the first code has been spent."
            );
            let again = match capture_code(&client_id).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("\n  INCONCLUSIVE: the second sign-in failed: {e}");
                    std::process::exit(1);
                }
            };
            match oauth::exchange_code(
                &http,
                oauth::WEBEX_TOKEN_URL,
                &client_id,
                Some(secret),
                &again.code,
                &again.verifier,
                &again.redirect_uri,
            )
            .await
            {
                Ok(t) => {
                    println!(
                        "\n  ANSWERED: the same flow succeeded WITH a client secret and failed\n\
                         \x20 without one, so Webex requires it even under PKCE. Keep\n\
                         \x20 Provider::Webex::requires_secret() = true, and keep shipping\n\
                         \x20 the secret in the built-in posture."
                    );
                    t
                }
                Err(with_secret) => {
                    // Both postures failed, so the secret was never the
                    // variable. Print them side by side — the difference, or
                    // the lack of one, is the whole diagnosis.
                    eprintln!(
                        "\n  INCONCLUSIVE: it failed WITH the secret too, so the first refusal\n\
                         \x20 was not about the secret. Compare the two:\n\
                         \x20   without secret: {without_secret}\n\
                         \x20   with secret:    {with_secret}"
                    );
                    std::process::exit(1);
                }
            }
        }
    };

    report("Exchange", &tokens);

    // Phase 3: refresh, which is where Webex differs from everyone else — the
    // refresh token rotates and its clock restarts.
    let Some(refresh_token) = tokens.refresh_token.clone() else {
        eprintln!("\nNo refresh token came back, so the refresh cannot be exercised.");
        std::process::exit(1);
    };
    println!("\nRefreshing…");
    match oauth::refresh(
        &http,
        oauth::WEBEX_TOKEN_URL,
        &client_id,
        client_secret.as_deref(),
        &refresh_token,
    )
    .await
    {
        Ok(refreshed) => {
            report("Refresh", &refreshed);
            match refreshed.refresh_token.as_deref() {
                Some(new) if new != refresh_token => println!(
                    "\n  Confirmed: the refresh token ROTATED. Every refresh must persist\n\
                     \x20 the new value or the account dies when the old one lapses."
                ),
                Some(_) => println!("\n  Note: the refresh token came back UNCHANGED this time."),
                None => println!("\n  Note: no new refresh token — the old one stays valid."),
            }
        }
        Err(e) => eprintln!("\nRefresh failed: {e}"),
    }

    println!("\nDone. Nothing was stored.");
}

struct Captured {
    code: String,
    verifier: String,
    redirect_uri: String,
}

/// Run the consent phase and keep the code instead of exchanging it, so the two
/// token postures can be driven separately against one flow.
async fn capture_code(client_id: &str) -> Result<Captured, String> {
    let (listener, authz) = oauth::begin_loopback(client_id, oauth::WEBEX_AUTH_URL)
        .await
        .map_err(|e| e.to_string())?;
    println!("{}", authz.authorize_url);
    oauth::open_consent_screen(&authz);
    let code = oauth::capture_loopback_code(listener, &authz)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Captured {
        code,
        verifier: authz.pkce_verifier,
        redirect_uri: authz.redirect_uri,
    })
}

fn report(phase: &str, tokens: &oauth::TokenSet) {
    println!("\n{phase} succeeded.");
    println!("  access token   {} characters", tokens.access_token.len());
    println!("  access expires {}", tokens.expires_at);
    match tokens.refresh_expires_at {
        Some(at) => println!("  refresh expires {at}"),
        None => println!("  refresh expires (not reported)"),
    }
    match &tokens.scope {
        Some(s) => println!("  granted scopes {s}"),
        None => println!("  granted scopes (not reported)"),
    }
}

/// Last four characters, so a printed id is recognisable without being usable.
fn tail(s: &str) -> String {
    s.chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// Environment first, then the gitignored file — the same precedence
/// `crates/builtin-oauth/build.rs` uses.
fn load_credentials() -> Option<(String, Option<String>)> {
    let from_env = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let file = read_env_file(&credentials_path());
    let pick =
        |name: &str| from_env(name).or_else(|| file.get(name).cloned().filter(|v| !v.is_empty()));
    let id = pick("APERIO_OAUTH_WEBEX_CLIENT_ID")?;
    Some((id, pick("APERIO_OAUTH_WEBEX_CLIENT_SECRET")))
}

fn credentials_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("APERIO_OAUTH_CLIENTS_FILE") {
        if !explicit.trim().is_empty() {
            return PathBuf::from(explicit);
        }
    }
    // CARGO_MANIFEST_DIR is <workspace>/crates/vc-adapter-webex.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("oauth-clients.local.env"))
        .unwrap_or_else(|| PathBuf::from("oauth-clients.local.env"))
}

fn read_env_file(path: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        out.insert(name.trim().to_string(), value.to_string());
    }
    out
}
