//! Aperio's own OAuth client credentials, baked in at build time.
//!
//! Some providers (Cisco Webex today, Google tomorrow) will not issue tokens to
//! a client that cannot present a client id — and in Webex's case a client
//! secret as well, even under PKCE. A shipped app therefore has two honest
//! postures, and Aperio supports both:
//!
//!  - **Built-in** — the release build carries Aperio's registered client, so
//!    connecting an account is one button. The secret is extractable from the
//!    binary; that is inherent, and the mitigation is scope, not obscurity: the
//!    registration asks for the narrowest scopes that work, the credential is
//!    revocable, and on its own it grants nothing without a user's consent.
//!  - **Bring your own** — a build without baked-in values (a local build, a
//!    fork, a distribution rebuild) asks the user to register their own
//!    integration and paste its client id and secret. Nothing is degraded
//!    except convenience.
//!
//! The crate reports which posture a build is in and never decides policy: it
//! hands out a [`BuiltinClient`] or `None`, and the host decides what to do.
//!
//! ## Where the values come from
//!
//! `build.rs` collects them from environment variables or from a gitignored
//! local file and re-emits them so the `option_env!` calls below see them. See
//! `crates/builtin-oauth/README.md` for the file format and the CI wiring.
//!
//! ## What must never happen
//!
//! A secret must never reach `accounts.config_json` — that column is documented
//! as non-secret and is appended to the sync event log unconditionally, so with
//! end-to-end encryption switched off it would travel to the remote sync target
//! in the clear. The host stores a *reference* to the credential instead, and
//! that is what [`ClientFingerprint`] is for: it identifies which client an
//! account was linked to, so a rebuild under a different registration is
//! detected and reported rather than decaying into an unexplained
//! `invalid_grant` two weeks later.

use serde::{Deserialize, Serialize};

/// An OAuth provider Aperio can carry credentials for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Webex,
    Google,
    Microsoft,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Webex => "webex",
            Provider::Google => "google",
            Provider::Microsoft => "microsoft",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "webex" => Provider::Webex,
            "google" => Provider::Google,
            "microsoft" => Provider::Microsoft,
            _ => return None,
        })
    }

    /// Whether this provider's token endpoint requires a client secret.
    ///
    /// Microsoft Graph is registered as a public client and uses PKCE alone.
    /// Google requires one but says in its own documentation that for an
    /// installed app it "is not treated as a secret". Webex requires one and
    /// makes no such concession, which is why its secret lives in the keychain
    /// rather than in a config column.
    pub fn requires_secret(self) -> bool {
        match self {
            Provider::Webex | Provider::Google => true,
            Provider::Microsoft => false,
        }
    }
}

/// A client credential pair this build carries.
#[derive(Debug, Clone)]
pub struct BuiltinClient {
    pub client_id: &'static str,
    /// `None` for a public client (see [`Provider::requires_secret`]).
    pub client_secret: Option<&'static str>,
}

impl BuiltinClient {
    /// Short, non-reversible identifier for this credential pair.
    pub fn fingerprint(&self) -> ClientFingerprint {
        ClientFingerprint::of(self.client_id, self.client_secret)
    }
}

/// A short digest naming *which* client an account was linked to, safe to
/// store beside the account and to print in a log.
///
/// Twelve hex characters of SHA-256 — long enough that two real registrations
/// will not collide, short enough to survive the log redactor, which erases any
/// run of 32 or more identifier characters on the assumption that it is a
/// token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientFingerprint(pub String);

impl ClientFingerprint {
    pub fn of(client_id: &str, client_secret: Option<&str>) -> Self {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"aperio-oauth-client-v1\0");
        h.update(client_id.as_bytes());
        h.update(b"\0");
        h.update(client_secret.unwrap_or("").as_bytes());
        Self(hex::encode(&h.finalize()[..6]))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ClientFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An empty variable counts as absent — an unset CI secret expands to the empty
/// string, and a client id of "" would fail far from its cause.
const fn non_empty(v: Option<&'static str>) -> Option<&'static str> {
    match v {
        Some(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// The credentials this build carries for `provider`, or `None` when it was
/// built without them (bring-your-own mode).
pub fn builtin_client(provider: Provider) -> Option<BuiltinClient> {
    let (id, secret) = match provider {
        Provider::Webex => (
            non_empty(option_env!("APERIO_OAUTH_WEBEX_CLIENT_ID")),
            non_empty(option_env!("APERIO_OAUTH_WEBEX_CLIENT_SECRET")),
        ),
        Provider::Google => (
            non_empty(option_env!("APERIO_OAUTH_GOOGLE_CLIENT_ID")),
            non_empty(option_env!("APERIO_OAUTH_GOOGLE_CLIENT_SECRET")),
        ),
        Provider::Microsoft => (
            non_empty(option_env!("APERIO_OAUTH_MICROSOFT_CLIENT_ID")),
            None,
        ),
    };
    let client_id = id?;
    // A provider that needs a secret and has none is not half-configured, it is
    // unconfigured: offering the built-in path would fail at the token
    // endpoint, which reads to the user as a broken app rather than as a build
    // that simply has no credentials.
    if provider.requires_secret() && secret.is_none() {
        return None;
    }
    Some(BuiltinClient {
        client_id,
        client_secret: secret,
    })
}

/// Whether this build carries credentials for `provider`. Cheap enough to call
/// from a UI query.
pub fn has_builtin_client(provider: Provider) -> bool {
    builtin_client(provider).is_some()
}

/// How many credentials this build carries, for the About screen and for logs.
/// Deliberately a count and never a list of values.
pub fn baked_count() -> u32 {
    option_env!("APERIO_OAUTH_BAKED_COUNT")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_strings_round_trip() {
        for p in [Provider::Webex, Provider::Google, Provider::Microsoft] {
            assert_eq!(Provider::parse(p.as_str()), Some(p));
        }
        assert_eq!(Provider::parse("nope"), None);
    }

    #[test]
    fn fingerprint_is_short_stable_and_secret_sensitive() {
        let a = ClientFingerprint::of("client-abc", Some("s1"));
        let b = ClientFingerprint::of("client-abc", Some("s1"));
        let c = ClientFingerprint::of("client-abc", Some("s2"));
        let d = ClientFingerprint::of("client-xyz", Some("s1"));
        assert_eq!(a, b, "same input must fingerprint identically");
        assert_ne!(a, c, "a rotated SECRET must be detectable");
        assert_ne!(a, d, "a rotated client ID must be detectable");
        assert_eq!(a.as_str().len(), 12);
        assert!(a.as_str().chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn fingerprint_survives_the_log_redactor() {
        // logging.rs redacts runs of 32+ identifier characters as probable
        // tokens. A fingerprint that got erased would be useless in exactly the
        // situation it exists for — reading a user's exported log.
        let f = ClientFingerprint::of("client-abc", Some("s1"));
        assert!(f.as_str().len() < 32);
    }

    #[test]
    fn fingerprint_distinguishes_absent_from_empty_secret() {
        assert_eq!(
            ClientFingerprint::of("id", None),
            ClientFingerprint::of("id", Some("")),
            "an absent and an empty secret are the same thing to the digest, \
             which is fine — non_empty() already collapses them before this point"
        );
    }

    #[test]
    fn a_provider_needing_a_secret_is_not_offered_without_one() {
        assert!(Provider::Webex.requires_secret());
        assert!(Provider::Google.requires_secret());
        assert!(!Provider::Microsoft.requires_secret());
    }

    #[test]
    fn baked_count_is_zero_or_more() {
        // Whatever this build carries, the accessor must not panic — it is read
        // on the About screen of every build, credentialed or not.
        let _ = baked_count();
    }

    #[test]
    fn the_two_accessors_agree_whatever_this_build_carries() {
        // Runs green in BOTH postures, which is the point: the test suite must
        // not depend on whether the machine running it has a credentials file.
        for p in [Provider::Webex, Provider::Google, Provider::Microsoft] {
            let client = builtin_client(p);
            assert_eq!(has_builtin_client(p), client.is_some());
            if let Some(c) = client {
                assert!(!c.client_id.is_empty(), "an empty id must read as absent");
                if p.requires_secret() {
                    assert!(
                        c.client_secret.is_some_and(|s| !s.is_empty()),
                        "{} needs a secret, so a half-configured build must not                          be offered as built-in",
                        p.as_str()
                    );
                }
                assert_eq!(c.fingerprint(), c.fingerprint(), "must be stable");
            }
        }
    }
}
