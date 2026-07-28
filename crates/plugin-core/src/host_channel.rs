//! The plugin → host channel: how an adapter tells the host something the host
//! did not ask for.
//!
//! Vtable calls run one way. The host asks, the plugin answers, and there is no
//! room in that shape for "by the way, the credential I hold has changed" —
//! which is exactly what an OAuth provider that rotates tokens forces an
//! adapter to say. Without a channel the adapter refreshes into memory, the
//! host's stored copy goes stale, and the account dies quietly whenever the old
//! credential finally lapses.
//!
//! The shape mirrors [`crate::manager`]'s log bridge, for the same reason it
//! works there: the host hands every plugin a `'static` function pointer into
//! THIS binary's code, so a dlopen'd plugin — which links its own copy of
//! plugin-core, with its own statics — still reaches the host's state. On the
//! static-link path the pointer is simply into the same binary.
//!
//! ## Who is speaking
//!
//! A report has to name an account, and a plugin cannot be trusted to name one
//! honestly: every plugin in the process shares an address space, so an id it
//! simply asserts is an id it could have made up. So the host mints an opaque
//! random token per instance and splices it into the TRANSIENT config handed to
//! `open_instance` — never into the persisted account row, and therefore never
//! into the sync event log. The plugin echoes it back; the host resolves it to
//! an account. A guessable token would defeat the whole arrangement, so it is
//! CSPRNG-random rather than derived from anything.
//!
//! ## What the host promises
//!
//! Only [`ACCEPTED`](crate::abi::HOST_CHANNEL_ACCEPTED) promises anything: that
//! the host has taken durable responsibility. Everything else is information
//! the plugin may act on or ignore.

use std::collections::HashMap;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use serde::Deserialize;
use tracing::{debug, warn};

use crate::abi::{
    HOST_CHANNEL_ACCEPTED, HOST_CHANNEL_ENVELOPE_V1, HOST_CHANNEL_MALFORMED,
    HOST_CHANNEL_MAX_PAYLOAD, HOST_CHANNEL_UNKNOWN_KIND, HOST_CHANNEL_UNKNOWN_SCOPE,
};

/// How many recently-closed scopes stay resolvable.
///
/// A refresh that was in flight when an instance closed — a re-registration, an
/// account edit — would otherwise be dropped on the floor, stranding the only
/// credential that still works. They resolve for a while longer, logged, rather
/// than silently.
const CLOSED_GRACE: usize = 64;

/// A live, or recently closed, capability scope.
#[derive(Clone, Debug)]
pub struct ResolvedScope {
    pub account_id: String,
    pub plugin_id: String,
    /// Monotonic per-account generation. A report from an older generation is
    /// refused once a newer one has already written for that account, which is
    /// what stops an in-flight rotation from a closed instance from clobbering
    /// what a freshly re-registered one just persisted.
    pub generation: u64,
    /// False once the instance was closed. Still resolvable for a while — see
    /// [`CLOSED_GRACE`] — because a late report is usually the important one.
    pub live: bool,
}

/// What the platform host does with a report.
///
/// plugin-core cannot reach a keychain or an event log, so the implementation
/// lives in host-core and is installed at startup.
pub trait HostChannelHandler: Send + Sync {
    /// Handle one report. Returns an `APERIO_HOST_CHANNEL_*` status.
    ///
    /// Called on an ARBITRARY plugin thread, possibly while other vtable calls
    /// are in flight. It must not call back into any plugin, must not take a
    /// lock that a host command could hold while awaiting a plugin call, and
    /// must not panic.
    fn handle(&self, scope: &ResolvedScope, kind: &str, payload: &serde_json::Value) -> c_int;
}

static HANDLER: OnceLock<RwLock<Option<Arc<dyn HostChannelHandler>>>> = OnceLock::new();
static SCOPES: OnceLock<RwLock<ScopeTable>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct ScopeTable {
    live: HashMap<String, ResolvedScope>,
    /// Recently closed, oldest first.
    closed: Vec<(String, ResolvedScope)>,
}

fn handler_slot() -> &'static RwLock<Option<Arc<dyn HostChannelHandler>>> {
    HANDLER.get_or_init(|| RwLock::new(None))
}

fn scopes() -> &'static RwLock<ScopeTable> {
    SCOPES.get_or_init(|| RwLock::new(ScopeTable::default()))
}

/// Install (or replace) the process-wide handler.
///
/// Replacing one is legitimate — the mobile test binary opens many hosts — but
/// is logged, because in production it happens exactly once.
pub fn install_handler(handler: Arc<dyn HostChannelHandler>) {
    let mut slot = handler_slot()
        .write()
        .expect("host-channel handler poisoned");
    if slot.is_some() {
        warn!("replacing the plugin host-channel handler");
    }
    *slot = Some(handler);
}

/// Whether a handler is installed. A plugin reporting before one exists gets
/// `FAILED`, not silence.
pub fn handler_installed() -> bool {
    handler_slot().read().map(|h| h.is_some()).unwrap_or(false)
}

/// A minted capability, held by whatever owns the instance.
///
/// Not `Clone`: one instance, one token. Dropping it moves the scope into the
/// recently-closed ring rather than deleting it outright.
pub struct ScopeToken(String);

impl ScopeToken {
    /// The opaque string to splice into the instance's transient config.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for ScopeToken {
    fn drop(&mut self) {
        close_scope(&self.0);
    }
}

impl std::fmt::Debug for ScopeToken {
    /// Never prints the token. It is a capability: a log line carrying one
    /// would hand anyone who reads that log the ability to write another
    /// account's credentials.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScopeToken(<redacted>)")
    }
}

/// Mint a capability for one instance.
///
/// The token is 32 hex characters of CSPRNG output. Deliberately NOT derived
/// from the account id or a counter: a derivable token can be forged by any
/// plugin in the process, which would make the whole scheme decorative.
pub fn mint_scope(account_id: &str, plugin_id: &str) -> ScopeToken {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    let scope = ResolvedScope {
        account_id: account_id.to_string(),
        plugin_id: plugin_id.to_string(),
        generation: GENERATION.fetch_add(1, Ordering::Relaxed),
        live: true,
    };
    scopes()
        .write()
        .expect("scope table poisoned")
        .live
        .insert(token.clone(), scope);
    ScopeToken(token)
}

fn close_scope(token: &str) {
    let mut table = scopes().write().expect("scope table poisoned");
    if let Some(mut scope) = table.live.remove(token) {
        scope.live = false;
        table.closed.push((token.to_string(), scope));
        while table.closed.len() > CLOSED_GRACE {
            table.closed.remove(0);
        }
    }
}

fn resolve(token: &str) -> Option<ResolvedScope> {
    let table = scopes().read().expect("scope table poisoned");
    if let Some(scope) = table.live.get(token) {
        return Some(scope.clone());
    }
    table
        .closed
        .iter()
        .rev()
        .find(|(t, _)| t == token)
        .map(|(_, s)| s.clone())
}

#[derive(Debug, Deserialize)]
struct Envelope {
    v: u32,
    scope: String,
    kind: String,
    #[serde(default)]
    payload: serde_json::Value,
}

/// The trampoline handed to every plugin. Stable address in the HOST binary.
///
/// # Safety
///
/// FFI callback. `json_ptr` / `json_len` must describe a readable buffer valid
/// for the duration of the call; `json_ptr` may be NULL only when `json_len` is
/// zero.
pub unsafe extern "C" fn forward_host_event(json_ptr: *const u8, json_len: usize) -> c_int {
    // Everything here runs on a plugin's thread. A panic would unwind across
    // the FFI boundary, which is undefined behaviour, so the whole body is
    // caught — the worst outcome must be a status code, never a crash.
    let result = std::panic::catch_unwind(|| {
        if json_len == 0 || json_ptr.is_null() {
            return HOST_CHANNEL_MALFORMED;
        }
        // Bound before allocating: the length is the plugin's word, and this
        // runs before anything has validated it.
        if json_len > HOST_CHANNEL_MAX_PAYLOAD {
            warn!(
                json_len,
                "plugin host-channel envelope is too large; refused"
            );
            return HOST_CHANNEL_MALFORMED;
        }
        // SAFETY: caller contract — readable for `json_len` bytes.
        let bytes = unsafe { std::slice::from_raw_parts(json_ptr, json_len) };
        let Ok(envelope) = serde_json::from_slice::<Envelope>(bytes) else {
            return HOST_CHANNEL_MALFORMED;
        };
        if envelope.v != HOST_CHANNEL_ENVELOPE_V1 {
            return HOST_CHANNEL_MALFORMED;
        }
        let Some(scope) = resolve(&envelope.scope) else {
            // Either the instance is long gone, or the token was never one we
            // issued. Both are worth a line, neither is worth the token.
            debug!(kind = %envelope.kind, "plugin host-channel report names no live scope");
            return HOST_CHANNEL_UNKNOWN_SCOPE;
        };
        if !scope.live {
            warn!(
                account_id = %scope.account_id,
                kind = %envelope.kind,
                "accepting a report from a closed instance; it was probably in flight when \
                 the account was re-registered"
            );
        }
        let handler = {
            let slot = handler_slot()
                .read()
                .expect("host-channel handler poisoned");
            slot.clone()
        };
        let Some(handler) = handler else {
            warn!("a plugin reported before any host-channel handler was installed");
            return crate::abi::HOST_CHANNEL_FAILED;
        };
        handler.handle(&scope, &envelope.kind, &envelope.payload)
    });
    match result {
        Ok(status) => status,
        Err(_) => {
            warn!("the plugin host-channel handler panicked; reporting failure");
            crate::abi::HOST_CHANNEL_FAILED
        }
    }
}

/// Status for a `kind` this host does not know, for handlers to return.
pub const fn unknown_kind() -> c_int {
    HOST_CHANNEL_UNKNOWN_KIND
}

/// Status for a report the host took durable responsibility for.
pub const fn accepted() -> c_int {
    HOST_CHANNEL_ACCEPTED
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The handler is process-wide by design — one host, one handler — so the
    /// tests that install one must not run concurrently or they would answer
    /// each other's reports.
    static SERIALISE: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct Recorder {
        seen: Mutex<Vec<(String, String, serde_json::Value)>>,
        answer: c_int,
    }

    impl HostChannelHandler for Recorder {
        fn handle(&self, scope: &ResolvedScope, kind: &str, payload: &serde_json::Value) -> c_int {
            self.seen.lock().unwrap().push((
                scope.account_id.clone(),
                kind.to_string(),
                payload.clone(),
            ));
            self.answer
        }
    }

    fn send(json: &str) -> c_int {
        unsafe { forward_host_event(json.as_ptr(), json.len()) }
    }

    #[test]
    fn a_minted_token_resolves_to_its_account_and_the_handler_sees_it() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        let recorder = Arc::new(Recorder {
            answer: HOST_CHANNEL_ACCEPTED,
            ..Default::default()
        });
        install_handler(recorder.clone());
        let token = mint_scope("account-1", "com.aperio.vc-adapter-webex");

        let status = send(&format!(
            r#"{{"v":1,"scope":"{}","kind":"credential.rotated","payload":{{"slot":"refresh_token"}}}}"#,
            token.as_str()
        ));
        assert_eq!(status, HOST_CHANNEL_ACCEPTED);
        let seen = recorder.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "account-1");
        assert_eq!(seen[0].1, "credential.rotated");
    }

    #[test]
    fn a_token_nobody_issued_is_refused() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        install_handler(Arc::new(Recorder {
            answer: HOST_CHANNEL_ACCEPTED,
            ..Default::default()
        }));
        // The whole point of an unguessable token: a plugin cannot name another
        // account by constructing an id.
        let status =
            send(r#"{"v":1,"scope":"deadbeefdeadbeefdeadbeefdeadbeef","kind":"x","payload":{}}"#);
        assert_eq!(status, HOST_CHANNEL_UNKNOWN_SCOPE);
    }

    #[test]
    fn a_closed_instance_can_still_report_once() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        // A refresh in flight when the account was re-registered is exactly the
        // report worth keeping — dropping it strands the only working token.
        let recorder = Arc::new(Recorder {
            answer: HOST_CHANNEL_ACCEPTED,
            ..Default::default()
        });
        install_handler(recorder.clone());
        let token_value = {
            let token = mint_scope("account-2", "p");
            token.as_str().to_string()
        }; // dropped here → closed

        let status = send(&format!(
            r#"{{"v":1,"scope":"{token_value}","kind":"credential.rotated","payload":{{}}}}"#
        ));
        assert_eq!(status, HOST_CHANNEL_ACCEPTED);
        assert_eq!(recorder.seen.lock().unwrap()[0].0, "account-2");
    }

    #[test]
    fn a_malformed_or_oversized_envelope_never_reaches_the_handler() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        let recorder = Arc::new(Recorder {
            answer: HOST_CHANNEL_ACCEPTED,
            ..Default::default()
        });
        install_handler(recorder.clone());
        assert_eq!(send("not json"), HOST_CHANNEL_MALFORMED);
        assert_eq!(send(""), HOST_CHANNEL_MALFORMED);
        // A future schema is refused rather than guessed at.
        assert_eq!(
            send(r#"{"v":99,"scope":"x","kind":"y","payload":{}}"#),
            HOST_CHANNEL_MALFORMED
        );
        let huge = format!(
            r#"{{"v":1,"scope":"x","kind":"y","payload":"{}"}}"#,
            "z".repeat(HOST_CHANNEL_MAX_PAYLOAD)
        );
        assert_eq!(send(&huge), HOST_CHANNEL_MALFORMED);
        assert!(recorder.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_null_pointer_is_refused_rather_than_dereferenced() {
        assert_eq!(
            unsafe { forward_host_event(std::ptr::null(), 0) },
            HOST_CHANNEL_MALFORMED
        );
        assert_eq!(
            unsafe { forward_host_event(std::ptr::null(), 10) },
            HOST_CHANNEL_MALFORMED
        );
    }

    #[test]
    fn two_scopes_never_share_a_token() {
        let a = mint_scope("account-a", "p");
        let b = mint_scope("account-b", "p");
        assert_ne!(a.as_str(), b.as_str());
        assert_eq!(a.as_str().len(), 32);
        // And the Debug impl must not leak it — a log line carrying a token
        // hands its reader the ability to write that account's credentials.
        assert!(!format!("{a:?}").contains(a.as_str()));
    }

    #[test]
    fn generations_increase_so_a_stale_scope_is_recognisable() {
        let first = mint_scope("account-g", "p");
        let second = mint_scope("account-g", "p");
        let g1 = resolve(first.as_str()).unwrap().generation;
        let g2 = resolve(second.as_str()).unwrap().generation;
        assert!(
            g2 > g1,
            "a re-registration must out-rank the scope it replaced"
        );
    }
}
