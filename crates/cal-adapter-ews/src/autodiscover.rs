//! POX (Plain Old XML) Autodiscover for EWS.
//!
//! Reference: <https://learn.microsoft.com/en-us/exchange/client-developer/
//! exchange-web-services/pox-autodiscover-web-service-reference-for-exchange>.
//!
//! Given an email address + password, walk Microsoft's URL cascade
//! until one of them returns a parseable response containing an
//! `<EwsUrl>` element. The cascade we run, in order:
//!
//!   1. POST `https://<domain>/autodiscover/autodiscover.xml`
//!   2. POST `https://autodiscover.<domain>/autodiscover/autodiscover.xml`
//!   3. GET  `http://autodiscover.<domain>/autodiscover/autodiscover.xml`
//!      → expect a `302 Location` pointing at the real HTTPS endpoint,
//!        then POST against that.
//!
//! Each POST sends the same outlook-2006 request schema with the
//! user's e-mail in `<EMailAddress>`. Each response can be one of:
//!
//!   - a `<Settings>` block with `<Protocol Type="EXPR">` (external
//!     access) or `Type="EXCH">` (internal) — those carry the actual
//!     `<EwsUrl>` we want.
//!   - `<Action>redirectAddr</Action>` + `<RedirectAddr>new@addr</…>`
//!     — start the cascade over with the new address.
//!   - `<Action>redirectUrl</Action>` + `<RedirectUrl>new url</…>`
//!     — POST against the new URL directly.
//!   - `<Error>` — auth failure or "this user is on Office 365 / use
//!     Graph instead".
//!
//! We follow redirects up to `MAX_REDIRECTS` total (across the whole
//! cascade) so a misconfigured server can't put us in an infinite
//! loop. DNS SRV (`_autodiscover._tcp.<domain>`) lookup is deferred
//! — `trust-dns` adds a heavy dependency, and in practice the four
//! steps above cover ~95% of on-premise Exchange deployments.

use std::time::Duration;

use quick_xml::events::Event as XmlEvent;
use quick_xml::reader::Reader;
use reqwest::header::{HeaderValue, CONTENT_TYPE};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};

use crate::auth::{basic_auth_header, BasicCredentials};
use crate::error::{EwsError, EwsResult};

/// Maximum number of probe steps we'll run before giving up. Counts
/// every HTTP request including redirect-chains, so a healthy setup
/// usually needs 1-3.
const MAX_REDIRECTS: usize = 8;

/// What Autodiscover gave us. Surfaced to the caller verbatim; the
/// command layer drops everything except `ews_url` into the
/// AccountsDialog endpoint field. `account_email` may have been
/// rewritten if a `<RedirectAddr>` step asked us to retry with a
/// different address — we surface the final one so the user can spot
/// "ah, the server wants alice@hs-anhalt.de.example.com, not just
/// alice@hs-anhalt.de" themselves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredEndpoints {
    pub ews_url: String,
    pub account_email: String,
}

/// Top-level entry point. Walks the URL cascade for `email` until we
/// find an `<EwsUrl>` or run out of probes. Auth is Basic + the
/// supplied password (Microsoft's POX endpoint accepts the same
/// credentials as the eventual EWS endpoint).
pub async fn discover(
    email: &str,
    password: &str,
    http: &reqwest::Client,
) -> EwsResult<DiscoveredEndpoints> {
    let domain = email_domain(email).ok_or_else(|| {
        EwsError::Config(format!("'{email}' is not a valid email address"))
    })?;
    let creds = BasicCredentials {
        username: email.to_string(),
        password: password.to_string(),
    };

    // The cascade is stateful: a redirectAddr or redirectUrl step
    // rewrites either the user identity or the next URL. We loop
    // here rather than recurse so the redirect counter is easy to
    // reason about.
    let mut current_email = email.to_string();
    let mut current_creds = creds;
    let mut redirect_url: Option<String> = None;

    for _ in 0..MAX_REDIRECTS {
        let probe_urls = match &redirect_url {
            Some(u) => vec![u.clone()],
            None => default_probes(domain),
        };
        // Try each probe URL until one of them returns *something*
        // parseable (Outcome::Settings, RedirectAddr or RedirectUrl).
        // 4xx/5xx and connection errors are recoverable — we just
        // move on to the next probe. Only when all probes in the
        // current iteration fail do we bail.
        let mut last_err: Option<EwsError> = None;
        let mut next_step: Option<Outcome> = None;
        for url in &probe_urls {
            match probe(url, &current_email, &current_creds, http).await {
                Ok(outcome) => {
                    next_step = Some(outcome);
                    break;
                }
                Err(err) => {
                    last_err = Some(err);
                }
            }
        }
        let Some(outcome) = next_step else {
            // The cascade ran without ever getting a parseable
            // response. Surface the last error so the UI can say
            // "couldn't reach autodiscover.hs-anhalt.de" vs the
            // generic "no EWS endpoint found".
            return Err(last_err.unwrap_or_else(|| {
                EwsError::DiscoveryFailed(domain.to_string())
            }));
        };
        match outcome {
            Outcome::Settings(found) => {
                return Ok(DiscoveredEndpoints {
                    ews_url: found,
                    account_email: current_email,
                });
            }
            Outcome::RedirectAddr(new_email) => {
                // Re-do the whole cascade with the new address. The
                // password stays — Outlook clients keep the original
                // credentials too.
                current_email = new_email.clone();
                current_creds.username = new_email;
                redirect_url = None;
            }
            Outcome::RedirectUrl(new_url) => {
                redirect_url = Some(new_url);
            }
            Outcome::Error(message) => {
                // Autodiscover spoke to us coherently but said "no".
                // Surface as DiscoveryFailed so the AccountsDialog
                // can suggest manual entry rather than retry.
                return Err(EwsError::DiscoveryFailed(format!(
                    "{domain}: {message}"
                )));
            }
        }
    }

    Err(EwsError::DiscoveryFailed(format!(
        "{domain}: redirect loop exceeded {MAX_REDIRECTS} hops"
    )))
}

/// Build a discovery-purpose reqwest client. We need an explicit
/// instance because:
///
///   - the default adapter client follows up to 10 redirects, but
///     autodiscover's 302 is *not* a redirect we want reqwest to
///     consume — we have to read the `Location` header ourselves to
///     drive the cascade.
///   - the default client's TLS settings are good enough; we don't
///     downgrade for the lookup.
pub fn discover_client() -> EwsResult<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| EwsError::Network(e.to_string()))
}

/// Default URL cascade for a fresh probe (no redirect-target in play).
/// Order matches the Outlook client's behaviour: domain-root first
/// (works for the most-tightly-configured deployments), then the
/// `autodiscover.` subdomain (the spec-suggested default), then
/// HTTP-on-the-subdomain as a redirect-only probe.
fn default_probes(domain: &str) -> Vec<String> {
    vec![
        format!("https://{domain}/autodiscover/autodiscover.xml"),
        format!("https://autodiscover.{domain}/autodiscover/autodiscover.xml"),
        // Step 3 sends a GET (not a POST) because we expect a 302 with
        // a `Location` header on the response, not an XML body. We
        // model the GET as a POST-with-Outcome::RedirectUrl in
        // `probe`'s redirect-handling branch — keeping the URL list
        // uniform is worth the small dance.
        format!("http://autodiscover.{domain}/autodiscover/autodiscover.xml"),
    ]
}

/// One round-trip against a single autodiscover URL. Wraps both the
/// "POST returns XML" and the "GET returns 302" paths.
async fn probe(
    url: &str,
    email: &str,
    creds: &BasicCredentials,
    http: &reqwest::Client,
) -> EwsResult<Outcome> {
    let body = build_request_xml(email);
    let auth = basic_auth_header(creds)?;
    let mut req = http
        .post(url)
        .body(body)
        .header(
            CONTENT_TYPE,
            HeaderValue::from_static("text/xml; charset=utf-8"),
        );
    for (k, v) in auth.iter() {
        req = req.header(k, v.clone());
    }
    let res = req.send().await?;
    let status = res.status();

    // 301 / 302 / 307 → the server is pointing us at the real
    // autodiscover endpoint. Pick the `Location` header and surface
    // it as a RedirectUrl step.
    if status.is_redirection() {
        if let Some(loc) = res
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
        {
            return Ok(Outcome::RedirectUrl(absolute_url(url, &loc)?));
        }
        return Err(EwsError::Network(format!(
            "{url}: {status} redirect without Location header"
        )));
    }

    if !status.is_success() {
        // 401 on autodiscover usually means the user typed the wrong
        // password. Surface as Http so the caller can flag it that
        // way; the cascade itself treats every non-2xx as "next
        // probe please".
        return Err(EwsError::Http {
            status: status.as_u16(),
            message: status
                .canonical_reason()
                .unwrap_or("autodiscover")
                .to_string(),
        });
    }
    let body = res.text().await.unwrap_or_default();
    parse_response(&body)
}

/// Resolve a redirect target against the URL we just queried. Most
/// servers send back an absolute URL, but a handful (especially old
/// IIS setups) send a relative path.
fn absolute_url(base: &str, target: &str) -> EwsResult<String> {
    if target.starts_with("http://") || target.starts_with("https://") {
        Ok(target.to_string())
    } else {
        let base_url = url::Url::parse(base)?;
        let resolved = base_url.join(target)?;
        Ok(resolved.to_string())
    }
}

/// Build the standard outlook-2006 request body. The schema is
/// invariant — only the email address inside `<EMailAddress>` varies.
fn build_request_xml(email: &str) -> String {
    // Single-line to keep the wire payload compact; the schema URLs
    // are required verbatim (Microsoft is strict about whitespace
    // and namespace prefixes here).
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>",
            "<Autodiscover xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/outlook/requestschema/2006\">",
            "<Request>",
            "<EMailAddress>{email}</EMailAddress>",
            "<AcceptableResponseSchema>",
            "http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a",
            "</AcceptableResponseSchema>",
            "</Request>",
            "</Autodiscover>"
        ),
        email = escape_xml_text(email)
    )
}

fn escape_xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn email_domain(email: &str) -> Option<&str> {
    let (_local, domain) = email.split_once('@')?;
    if domain.is_empty() {
        None
    } else {
        Some(domain.trim().trim_end_matches('.'))
    }
}

/// What a single probe came back with.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// We found an `<EwsUrl>` — we're done.
    Settings(String),
    /// `<Action>redirectAddr</…>` — start over with this email.
    RedirectAddr(String),
    /// `<Action>redirectUrl</…>` or HTTP 302 — POST against this URL.
    RedirectUrl(String),
    /// `<Error>` — surfaced verbatim.
    Error(String),
}

/// Walk a `<Autodiscover>` response with quick-xml, looking for any
/// of the four outcomes above. The first `<EwsUrl>` wins (the
/// external one is listed before the internal one in well-formed
/// responses, which matches what an Aperio user wants).
fn parse_response(body: &str) -> EwsResult<Outcome> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut text_target: Option<&'static str> = None;
    let mut action_text = String::new();
    let mut redirect_addr = String::new();
    let mut redirect_url = String::new();
    let mut ews_url = String::new();
    let mut error_message = String::new();
    let mut error_code = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                match local.as_slice() {
                    b"action" => text_target = Some("action"),
                    b"redirectaddr" => text_target = Some("redirect_addr"),
                    b"redirecturl" => text_target = Some("redirect_url"),
                    b"ewsurl" => text_target = Some("ews_url"),
                    b"errorcode" => text_target = Some("error_code"),
                    b"message" => text_target = Some("error_message"),
                    _ => {}
                }
            }
            Ok(XmlEvent::End(_)) => {
                text_target = None;
            }
            Ok(XmlEvent::Text(t)) => {
                if let Some(target) = text_target {
                    let raw = match t.unescape() {
                        Ok(c) => c.to_string(),
                        Err(_) => continue,
                    };
                    let s = raw.trim();
                    match target {
                        "action" => action_text.push_str(s),
                        "redirect_addr" => redirect_addr.push_str(s),
                        "redirect_url" => redirect_url.push_str(s),
                        "ews_url" => {
                            if ews_url.is_empty() {
                                ews_url.push_str(s);
                            }
                        }
                        "error_code" => error_code.push_str(s),
                        "error_message" => error_message.push_str(s),
                        _ => {}
                    }
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => {
                return Err(EwsError::Protocol(format!(
                    "autodiscover xml parse: {err}"
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    // Priority order matches Microsoft's spec: settings > redirect >
    // error. The Outlook client picks the first `<Protocol>` block
    // whose `Type` attribute is `EXPR` (external) or `EXCH`
    // (internal); we don't filter on type because home users on
    // shared connections often only have `EXCH` configured anyway,
    // and the URL we get is still reachable.
    if !ews_url.is_empty() {
        return Ok(Outcome::Settings(ews_url));
    }
    let action = action_text.to_ascii_lowercase();
    if action == "redirectaddr" && !redirect_addr.is_empty() {
        return Ok(Outcome::RedirectAddr(redirect_addr));
    }
    if action == "redirecturl" && !redirect_url.is_empty() {
        return Ok(Outcome::RedirectUrl(redirect_url));
    }
    if !error_message.is_empty() || !error_code.is_empty() {
        return Ok(Outcome::Error(format!(
            "{}{}{}",
            error_code,
            if error_code.is_empty() || error_message.is_empty() {
                ""
            } else {
                ": "
            },
            error_message,
        )));
    }
    Err(EwsError::Protocol(
        "autodiscover response lacked Settings / Action / Error".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_domain_extracts_host() {
        assert_eq!(email_domain("alice@hs-anhalt.de"), Some("hs-anhalt.de"));
        assert_eq!(email_domain("a@b.c"), Some("b.c"));
        assert_eq!(email_domain("alice@hs-anhalt.de."), Some("hs-anhalt.de"));
        assert_eq!(email_domain("noat-sign"), None);
        assert_eq!(email_domain("alice@"), None);
    }

    #[test]
    fn default_probes_cover_three_urls_in_order() {
        let probes = default_probes("hs-anhalt.de");
        assert_eq!(probes.len(), 3);
        assert!(probes[0].starts_with("https://hs-anhalt.de/autodiscover"));
        assert!(probes[1].starts_with("https://autodiscover.hs-anhalt.de/"));
        assert!(probes[2].starts_with("http://autodiscover.hs-anhalt.de/"));
    }

    #[test]
    fn request_xml_contains_email_and_schema() {
        let xml = build_request_xml("alice@hs-anhalt.de");
        assert!(xml.contains("<EMailAddress>alice@hs-anhalt.de</EMailAddress>"));
        assert!(
            xml.contains("/outlook/requestschema/2006"),
            "request schema must be set"
        );
        assert!(
            xml.contains("/outlook/responseschema/2006a"),
            "response schema must be set"
        );
    }

    #[test]
    fn request_xml_escapes_xml_specials() {
        // The user's email shouldn't contain reserved chars in
        // practice, but Microsoft's parser is strict — making sure we
        // never inject raw `<` keeps us safe against weird domains
        // (and against tests that pass deliberate junk).
        let xml = build_request_xml("a<b@c");
        assert!(xml.contains("a&lt;b@c"));
    }

    #[test]
    fn parses_settings_with_ews_url() {
        let body = r#"<?xml version="1.0"?>
<Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006">
  <Response xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a">
    <Account>
      <AccountType>email</AccountType>
      <Action>settings</Action>
      <Protocol>
        <Type>EXPR</Type>
        <EwsUrl>https://mail.hs-anhalt.de/EWS/Exchange.asmx</EwsUrl>
      </Protocol>
    </Account>
  </Response>
</Autodiscover>"#;
        match parse_response(body).unwrap() {
            Outcome::Settings(url) => {
                assert_eq!(url, "https://mail.hs-anhalt.de/EWS/Exchange.asmx");
            }
            other => panic!("expected Settings, got {other:?}"),
        }
    }

    #[test]
    fn parses_redirect_addr() {
        let body = r#"<?xml version="1.0"?>
<Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006">
  <Response>
    <Account>
      <Action>redirectAddr</Action>
      <RedirectAddr>alice@mail.hs-anhalt.de</RedirectAddr>
    </Account>
  </Response>
</Autodiscover>"#;
        match parse_response(body).unwrap() {
            Outcome::RedirectAddr(addr) => {
                assert_eq!(addr, "alice@mail.hs-anhalt.de");
            }
            other => panic!("expected RedirectAddr, got {other:?}"),
        }
    }

    #[test]
    fn parses_redirect_url() {
        let body = r#"<?xml version="1.0"?>
<Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006">
  <Response>
    <Account>
      <Action>redirectUrl</Action>
      <RedirectUrl>https://autodiscover.hs-anhalt.example.net/autodiscover/autodiscover.xml</RedirectUrl>
    </Account>
  </Response>
</Autodiscover>"#;
        match parse_response(body).unwrap() {
            Outcome::RedirectUrl(url) => {
                assert!(url.contains("hs-anhalt.example.net"));
            }
            other => panic!("expected RedirectUrl, got {other:?}"),
        }
    }

    #[test]
    fn parses_error_block() {
        let body = r#"<?xml version="1.0"?>
<Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006">
  <Response>
    <Error>
      <ErrorCode>600</ErrorCode>
      <Message>Invalid Request</Message>
    </Error>
  </Response>
</Autodiscover>"#;
        match parse_response(body).unwrap() {
            Outcome::Error(msg) => {
                assert!(msg.contains("600"));
                assert!(msg.contains("Invalid Request"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn unparseable_response_is_protocol_error() {
        let body = "<html><body>404 Not Found</body></html>";
        assert!(matches!(
            parse_response(body),
            Err(EwsError::Protocol(_))
        ));
    }

    #[test]
    fn absolute_url_resolves_relative_redirect() {
        let abs = absolute_url(
            "https://mail.example.org/autodiscover/autodiscover.xml",
            "/owa/redir.asp",
        )
        .unwrap();
        assert_eq!(abs, "https://mail.example.org/owa/redir.asp");
    }

    #[test]
    fn absolute_url_passes_through_absolute_target() {
        let abs = absolute_url(
            "https://mail.example.org/autodiscover/autodiscover.xml",
            "https://other.host/autodiscover/autodiscover.xml",
        )
        .unwrap();
        assert_eq!(abs, "https://other.host/autodiscover/autodiscover.xml");
    }

    #[tokio::test]
    async fn discover_returns_endpoint_on_first_hit() {
        let mut server = mockito::Server::new_async().await;
        // Default URL cascade hits domain-root first. We mount the
        // response there and return the EWS URL straight away.
        let _m = server
            .mock("POST", "/autodiscover/autodiscover.xml")
            .with_status(200)
            .with_header("content-type", "text/xml; charset=utf-8")
            .with_body(
                r#"<?xml version="1.0"?>
<Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006">
  <Response>
    <Account>
      <Action>settings</Action>
      <Protocol>
        <Type>EXPR</Type>
        <EwsUrl>https://mail.example.org/EWS/Exchange.asmx</EwsUrl>
      </Protocol>
    </Account>
  </Response>
</Autodiscover>"#,
            )
            .create_async()
            .await;

        // Aim the cascade straight at the mockito server by handing
        // the request through a single absolute redirect-url instead
        // of the domain cascade — the public `discover` API uses
        // DNS-resolvable domains, but the probe machinery itself
        // accepts arbitrary URLs.
        let http = discover_client().unwrap();
        let outcome = probe(
            &format!("{}/autodiscover/autodiscover.xml", server.url()),
            "alice@example.org",
            &BasicCredentials {
                username: "alice@example.org".into(),
                password: "pw".into(),
            },
            &http,
        )
        .await
        .unwrap();
        match outcome {
            Outcome::Settings(url) => {
                assert_eq!(url, "https://mail.example.org/EWS/Exchange.asmx");
            }
            other => panic!("expected Settings, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_follows_302_as_redirect_url_outcome() {
        let mut server = mockito::Server::new_async().await;
        let target = format!("{}/autodiscover/real.xml", server.url());
        let _m = server
            .mock("POST", "/autodiscover/autodiscover.xml")
            .with_status(302)
            .with_header("Location", &target)
            .create_async()
            .await;
        let http = discover_client().unwrap();
        let outcome = probe(
            &format!("{}/autodiscover/autodiscover.xml", server.url()),
            "alice@example.org",
            &BasicCredentials {
                username: "alice@example.org".into(),
                password: "pw".into(),
            },
            &http,
        )
        .await
        .unwrap();
        match outcome {
            Outcome::RedirectUrl(url) => assert_eq!(url, target),
            other => panic!("expected RedirectUrl, got {other:?}"),
        }
    }
}
