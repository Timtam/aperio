//! Turning the sync target this device already has into an account.
//!
//! Everything else in this module now speaks accounts: [`super::from_account`]
//! builds the adapter from a row and a schema, [`super::select_account`] records
//! which row this device syncs through. What none of that does is help the user
//! who configured WebDAV eighteen months ago and has twenty preference keys and
//! a keychain entry to show for it. This is the one-way door between the two,
//! and it is the highest-risk function in the crate: a user with a working sync
//! must still have one afterwards, whatever happens in the middle.
//!
//! ## Nothing old is deleted
//!
//! Not one preference, not one keychain entry. Writing the new state and
//! clearing the old one in a single pass means a process that dies between the
//! two — or a keychain that accepts a write while the database refuses one —
//! leaves a device that can no longer sync at all. So the old target stays
//! exactly where it is and stays readable by [`super::build`], and a LATER
//! release, which can see that this one ran, does the clearing. That release
//! also removes the two values this leaves duplicated in a place they should not
//! be (the Dropbox and Drive client secrets, which have lived in preferences
//! since they were first stored and are copied into the keychain here); nothing
//! about that exposure is new, and undoing it is not worth the risk of doing it
//! here.
//!
//! ## What deliberately does not move
//!
//! The end-to-end encryption key and its flag. They belong to the DATASET and to
//! this device's posture towards it, not to the place the dataset is kept — the
//! reasoning is in [`super::from_account`] and this is not the place to
//! re-open it. [`super::build_selected`] reads both from exactly where they are.
//!
//! Host-key pins. They stay keyed by `host:port` under
//! `sync.adapter.sftp.knownHosts.`, and the migrated account has to produce the
//! SAME lookup string as the old builder did — including a port that was absent
//! or unparsable, where the old builder substituted 22. A pin that goes missing
//! is not a broken sync; it is a fingerprint dialog for a server the user
//! already confirmed, which is how people are taught to click through them.
//!
//! ## No event is emitted
//!
//! A sync-only account does not travel — `accounts::travels_between_devices`
//! says so, and every kind here answers false — but the stronger guarantee is
//! structural: this function takes no emitter, so there is nothing to call.
//! The row is created straight through [`AccountsRepo`].

use serde_json::{Map, Value};
use sync_engine::{SecretSlot, SecretStore};

use super::build::text_result;
use super::from_account::selected_account_id_result;
use super::*;
use crate::accounts::{AccountsRepo, AdapterKind};
use crate::user_prefs::UserPrefsRepo;

/// The account this device's old sync target became.
///
/// ## Why a marker, and not one of the other two answers
///
/// **Not the pointer's own presence.** [`PREF_SELECTED_ACCOUNT`] is a CHOICE,
/// and the user is allowed to unmake it: "this device does not sync" clears it.
/// The next launch would then find no pointer, migrate again, and hand them a
/// second copy of an account they had just decided not to use. It also cannot
/// tell "this ran" from "the user picked an account by hand".
///
/// **Not matching an existing row.** The local-folder adapter's only field is
/// device-local, so every local-folder account carries the same empty
/// `config_json` and they are indistinguishable by value. Adopting one would
/// mean silently repointing an account the user made themselves at a different
/// folder, which is a worse outcome than the duplicate it would have prevented.
///
/// The value is the account id rather than a bare `true`: the cleanup release
/// has to know which row owns the old preferences before it may delete them, and
/// a support question answers itself.
pub const PREF_MIGRATED_ACCOUNT: &str = "sync.adapter.migratedAccountId";

/// Why the migration could not finish.
///
/// Strings rather than the underlying error types, as in [`super::persist`]:
/// every caller here logs and moves on, and the old target is still readable, so
/// nothing downstream needs to match on the cause.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    /// A stored kind this build does not serve. Refused rather than guessed:
    /// there is no field table to translate it with, and inventing one would
    /// write an account nobody can open.
    #[error("unknown sync adapter kind: {0}")]
    UnknownKind(String),
    #[error("preferences: {0}")]
    Prefs(String),
    #[error("accounts: {0}")]
    Accounts(String),
    #[error("keychain: {0}")]
    Secret(String),
}

// ── the per-kind table ───────────────────────────────────────────────────────

/// Where a value is kept today.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// A preference key.
    Pref(&'static str),
    /// A keychain entry under one of the sync target's pseudo-accounts.
    Keychain(&'static str, SecretSlot),
}

/// Where it goes on the account.
///
/// The three destinations are the three halves `account_setup::plan_new_account`
/// splits a connect form into, and the split has to come out the same way here:
/// a value that lands in `config_json` when the schema calls it device-local
/// would travel to every other device, and one that lands in the device-local
/// store when the schema does not would be missing on every other device.
#[derive(Debug, Clone, Copy)]
enum Dest {
    /// `config_json`, under the PLUGIN's own field key — which is not always
    /// the preference's: `…local.path` is `remote_root` and `…dropbox.path` is
    /// `base_path`.
    Config(&'static str),
    /// `config_json`, as a JSON NUMBER, with the default the old builder
    /// substituted when the stored text would not parse.
    ///
    /// Separate from [`Self::Config`] because a string here is fatal twice
    /// over. The plugins declare `port: u16` and serde will not coerce `"2222"`,
    /// so the whole init config fails to deserialise; and the host-key pin is
    /// looked up by `host:port`, so a port that formats differently silently
    /// loses a fingerprint the user already confirmed.
    Port(&'static str, u16),
    /// This device's half, through `account_local`, under the plugin's own
    /// field key.
    Local(&'static str),
    /// The keychain, under the NEW account id, in this slot.
    ///
    /// The slot is stated rather than carried over from the source: SFTP's key
    /// passphrase lives in a keychain pseudo-account of its own today, in the
    /// `Password` slot, and has to arrive in `KeyPassphrase` — one account id
    /// now holds both, and two fields sharing a slot means the second write
    /// wins and the value comes back under both names.
    Secret(SecretSlot),
}

#[derive(Debug, Clone, Copy)]
struct Move {
    from: Source,
    to: Dest,
    /// The values this build understands, the FIRST being what an absent or
    /// unrecognised one resolves to.
    ///
    /// One field sets this, and the reason is not that it is a `choice` — the
    /// FTPS mode is a choice too and deliberately does NOT. What decides is
    /// what the OLD BUILDER did with an unrecognised value, because that is the
    /// state the user is actually in:
    ///
    /// - SFTP's `auth_method`: anything that was not `key` became `password`,
    ///   and that device syncs today. The plugin refuses an unknown method
    ///   outright, so echoing the stored value would stop a working sync at the
    ///   moment of migration. Normalising it is what keeps that user whole.
    /// - FTP's `mode`: an unrecognised value was REFUSED — see [`FTP`], where
    ///   the value is carried through untouched for exactly that reason.
    ///
    /// So this is not a general "tidy up a choice" rule. It is the one place
    /// where the old builder's answer, rather than the stored text, is the
    /// value the user has been living with.
    choice: Option<&'static [&'static str]>,
}

const fn from_pref(key: &'static str, to: Dest) -> Move {
    Move {
        from: Source::Pref(key),
        to,
        choice: None,
    }
}

const fn from_keychain(account: &'static str, slot: SecretSlot, to: Dest) -> Move {
    Move {
        from: Source::Keychain(account, slot),
        to,
        choice: None,
    }
}

const fn one_of(spec: Move, allowed: &'static [&'static str]) -> Move {
    Move {
        choice: Some(allowed),
        ..spec
    }
}

/// The local folder. Its one field is device-local, so the row that travels
/// carries nothing at all — which is the point: a path on this disk means
/// nothing on any other device.
const LOCAL: &[Move] = &[from_pref(PREF_LOCAL_PATH, Dest::Local("remote_root"))];

const WEBDAV: &[Move] = &[
    from_pref(PREF_WEBDAV_URL, Dest::Config("url")),
    from_pref(PREF_WEBDAV_USER, Dest::Config("user")),
    from_keychain(
        SECRET_ACCOUNT_WEBDAV,
        SecretSlot::Password,
        Dest::Secret(SecretSlot::Password),
    ),
];

const SFTP: &[Move] = &[
    from_pref(PREF_SFTP_HOST, Dest::Config("host")),
    from_pref(PREF_SFTP_PORT, Dest::Port("port", 22)),
    from_pref(PREF_SFTP_USER, Dest::Config("user")),
    from_pref(PREF_SFTP_PATH, Dest::Config("path")),
    // Both of these are the user's answer for THIS machine — which key file,
    // and therefore which way to authenticate. The schema marks them
    // device-local and the account row must not carry them.
    one_of(
        from_pref(PREF_SFTP_AUTH_METHOD, Dest::Local("auth_method")),
        &["password", "key"],
    ),
    from_pref(PREF_SFTP_KEY_PATH, Dest::Local("key_path")),
    // Both credentials come across, not just the one the current auth method
    // uses. They are already in the keychain side by side precisely so that
    // switching method does not mean retyping, and dropping the inactive one
    // here would take that away at the moment of migration. The plugin
    // dispatches on `auth_method` and ignores the other.
    from_keychain(
        SECRET_ACCOUNT_SFTP,
        SecretSlot::Password,
        Dest::Secret(SecretSlot::Password),
    ),
    from_keychain(
        SECRET_ACCOUNT_SFTP_KEY,
        SecretSlot::Password,
        Dest::Secret(SecretSlot::KeyPassphrase),
    ),
];

const FTP: &[Move] = &[
    from_pref(PREF_FTP_HOST, Dest::Config("host")),
    from_pref(PREF_FTP_PORT, Dest::Port("port", 21)),
    from_pref(PREF_FTP_USER, Dest::Config("user")),
    from_pref(PREF_FTP_PATH, Dest::Config("path")),
    // Carried verbatim, unlike the SFTP auth method above.
    //
    // Normalising an unrecognised mode to the first allowed value writes
    // `explicit` into the row, and the row is then the truth — nothing later
    // can tell it apart from a mode the user chose. A deliberate `plain` or
    // `implicit` that this build failed to recognise would come back as a
    // different transport, silently, which is the one outcome both the old
    // builder and the plugin refuse to produce.
    //
    // And there is no working sync to protect here, which is what makes the two
    // fields differ: the old builder answered `Unbuildable::Invalid` for an
    // unknown mode, so a target carrying one does not sync on this build today.
    // Passing the value through leaves it not syncing and moves the refusal to
    // the FTP plugin, which now names the offending value instead of falling
    // through to explicit FTPS.
    //
    // An ABSENT mode still ends up explicit exactly as before: nothing is
    // written into the row, and the plugin's own `default_mode` is `explicit`.
    from_pref(PREF_FTP_MODE, Dest::Config("mode")),
    from_keychain(
        SECRET_ACCOUNT_FTP,
        SecretSlot::Password,
        Dest::Secret(SecretSlot::Password),
    ),
];

/// Dropbox, and Drive below it, are the only kinds where a value changes
/// STORE: the OAuth client secret sits in a preference today and the schema
/// declares it a secret, so it is written to the keychain here. The preference
/// stays where it is like every other one — see the module doc.
const DROPBOX: &[Move] = &[
    from_pref(PREF_DROPBOX_CLIENT_ID, Dest::Config("client_id")),
    from_pref(
        PREF_DROPBOX_CLIENT_SECRET,
        Dest::Secret(SecretSlot::OauthClientSecret),
    ),
    // `base_path`, not `path`.
    from_pref(PREF_DROPBOX_PATH, Dest::Config("base_path")),
    from_keychain(
        SECRET_ACCOUNT_DROPBOX,
        SecretSlot::RefreshToken,
        Dest::Secret(SecretSlot::RefreshToken),
    ),
];

const GOOGLEDRIVE: &[Move] = &[
    from_pref(PREF_GOOGLEDRIVE_CLIENT_ID, Dest::Config("client_id")),
    from_pref(
        PREF_GOOGLEDRIVE_CLIENT_SECRET,
        Dest::Secret(SecretSlot::OauthClientSecret),
    ),
    from_pref(PREF_GOOGLEDRIVE_FOLDER_NAME, Dest::Config("folder_name")),
    from_keychain(
        SECRET_ACCOUNT_GOOGLEDRIVE,
        SecretSlot::RefreshToken,
        Dest::Secret(SecretSlot::RefreshToken),
    ),
];

/// Each stored kind, the adapter kind the account takes, and the values to move.
///
/// The account kind is the one the PLUGIN declares in its manifest, which is the
/// same string in five cases and not in the sixth: the local filesystem adapter
/// answers to `local_folder`, because `local` is [`AdapterKind::LOCAL`] — the
/// built-in store, host-internal, and an account of that kind already exists on
/// every device.
const KIND_TABLE: &[(&str, &str, &[Move])] = &[
    ("local", "local_folder", LOCAL),
    ("webdav", "webdav", WEBDAV),
    ("sftp", "sftp", SFTP),
    ("ftp", "ftp", FTP),
    ("dropbox", "dropbox", DROPBOX),
    ("googledrive", "googledrive", GOOGLEDRIVE),
];

fn table_for(stored_kind: &str) -> Option<(&'static str, &'static [Move])> {
    KIND_TABLE
        .iter()
        .find(|(stored, _, _)| *stored == stored_kind)
        .map(|(_, account_kind, moves)| (*account_kind, *moves))
}

// ── what the connect path reads out of the same table ────────────────────────
//
// [`super::connect`] writes an account from a CONNECT FORM rather than from
// this device's old preferences, but it has to reach the same row the migration
// would have written: same account kind, same field keys, same keychain slots.
// Everything below hands it one column of the table above, so the two paths
// cannot disagree about a rename or a slot — which is the one class of mistake
// here that produces an account nobody can open, silently.

/// The account kind a target of `stored_kind` takes.
///
/// The one the PLUGIN declares, which is the same string in five cases and not
/// in the sixth: the local filesystem adapter answers to `local_folder`,
/// because `local` is [`AdapterKind::LOCAL`], the host's own store.
pub(super) fn account_kind_for(stored_kind: &str) -> Option<&'static str> {
    table_for(stored_kind).map(|(account_kind, _)| account_kind)
}

/// The reverse, for a caller that has a row and needs the stored spelling.
///
/// Both frontends switch on `local`, not on `local_folder`, and the settings
/// card is rendered from that string.
pub(super) fn stored_kind_for(account_kind: &str) -> Option<&'static str> {
    KIND_TABLE
        .iter()
        .find(|(_, kind, _)| *kind == account_kind)
        .map(|(stored, _, _)| *stored)
}

/// The plugin's own field key for one of `stored_kind`'s FORM values, or `None`
/// where the form and the plugin already agree on the name.
///
/// Composed from the two tables that already state it — [`super::persist`] maps
/// a form key onto the preference it is written under, and [`KIND_TABLE`] maps
/// that preference onto the field the plugin declares — rather than restated. A
/// third copy of `path` → `remote_root` / `base_path` is a third thing to keep
/// in step, and the failure when it drifts is an adapter opened with a field it
/// ignores, which no plugin reports because none of them set
/// `deny_unknown_fields`.
pub(super) fn schema_field_for(stored_kind: &str, form_key: &str) -> Option<&'static str> {
    let pref = fields_for(stored_kind)?
        .iter()
        .find(|spec| spec.key == form_key)?
        .pref?;
    let (_, moves) = table_for(stored_kind)?;
    moves.iter().find_map(|step| match (step.from, step.to) {
        (Source::Pref(key), Dest::Config(field) | Dest::Local(field) | Dest::Port(field, _))
            if key == pref =>
        {
            Some(field)
        }
        _ => None,
    })
}

/// Every credential a target of `stored_kind` can hold, as `(legacy
/// pseudo-account, the slot it sits in there, the slot it takes on a row)`.
///
/// Three uses, all of them the connect path's: inheriting a credential the form
/// left out, carrying the OAuth refresh token onto the row the sign-in had no
/// way to write to, and deleting what is left behind afterwards. The two slots
/// differ exactly once — SFTP's key passphrase lives in a pseudo-account of its
/// own in the `Password` slot and belongs in `KeyPassphrase` on a row that holds
/// both credentials at the same time.
pub(super) fn credential_routes(stored_kind: &str) -> Vec<(&'static str, SecretSlot, SecretSlot)> {
    let Some((_, moves)) = table_for(stored_kind) else {
        return Vec::new();
    };
    moves
        .iter()
        .filter_map(|step| match (step.from, step.to) {
            (Source::Keychain(account, slot), Dest::Secret(target)) => {
                Some((account, slot, target))
            }
            _ => None,
        })
        .collect()
}

// ── reading the old target ───────────────────────────────────────────────────

/// One target, split the way the connect form would have split it.
///
/// No `Debug`: `secrets` holds cleartext credentials, and a type that can
/// format itself is one that eventually appears in a log line.
struct Planned {
    account_kind: &'static str,
    display_name: String,
    /// The half that travels with the account row.
    config: Map<String, Value>,
    /// The half that stays on this device.
    local: Map<String, Value>,
    /// Every device-local field the kind declares, so a value that is absent
    /// here is actively cleared rather than left over from somewhere else.
    local_fields: Vec<String>,
    secrets: Vec<(SecretSlot, String)>,
}

fn plan(
    prefs: &UserPrefsRepo<'_>,
    secrets: &dyn SecretStore,
    stored_kind: &str,
) -> Result<Planned, MigrateError> {
    let (account_kind, moves) =
        table_for(stored_kind).ok_or_else(|| MigrateError::UnknownKind(stored_kind.to_string()))?;

    let mut config = Map::new();
    let mut local = Map::new();
    let mut local_fields = Vec::new();
    let mut out_secrets = Vec::new();

    for step in moves {
        let stored = match step.from {
            // A preference that cannot be READ is not a preference that is not
            // set. Those two used to arrive here as the same `None`, and this
            // asks for the difference rather than trusting that nobody
            // reintroduces it: a database that stalls for one launch would
            // otherwise write an account missing whatever it could not read —
            // a URL, a user name, a key path — and then mark the migration
            // done, and the marker means no later launch ever looks again.
            Source::Pref(key) => {
                text_result(prefs, key).map_err(|err| MigrateError::Prefs(err.to_string()))?
            }
            // A credential that is NOT THERE is not an error — the old builder
            // treated most of them as optional too, and a target whose password
            // was never stored is one the user has to repair either way.
            //
            // A keychain that will not ANSWER is a different thing entirely,
            // and the two must not collapse into each other. Carrying on would
            // write an account with no credential and then mark the migration
            // done, so the one launch where the keystore was locked or busy
            // would cost the user their sync permanently. Refusing leaves the
            // old target whole and the marker unwritten, and the next launch
            // tries again.
            //
            // What comes back is moved BYTE FOR BYTE. The old builder handed
            // the stored bytes to the plugin untouched, and a credential is not
            // text to be tidied: a password with a leading or trailing space —
            // some generators emit them, and they survive a paste — matches
            // only in its exact form, so trimming it turns a working sync into
            // an authentication failure against what looks like the right
            // password. Emptiness is the one judgement left, and only a value
            // with no bytes at all counts: a secret that is nothing but spaces
            // is still a secret somebody stored.
            Source::Keychain(account, slot) => match secrets.retrieve(account, slot) {
                Ok(value) => Some(value).filter(|v| !v.is_empty()),
                Err(sync_engine::SecretError::NotFound) => None,
                Err(err) => return Err(MigrateError::Secret(err.to_string())),
            },
        };
        let value = match step.choice {
            // A normalised field always lands on a value, because the plugin
            // dispatches on it and its own default may not be the one the old
            // builder used. Everything else stays ABSENT when it is absent:
            // writing "" turns "the user did not say" into "the user said
            // nothing", and adapters read those differently.
            Some(allowed) => Some(
                stored
                    .filter(|v| allowed.contains(&v.as_str()))
                    .unwrap_or_else(|| allowed[0].to_string()),
            ),
            None => stored,
        };

        match step.to {
            Dest::Port(field, default) => {
                // `parse().unwrap_or(default)` and nothing stricter, on
                // purpose. It is what `build::port_of` does, so the `host:port`
                // the pin is looked up under cannot move — not for an absent
                // port, not for junk, and not for a value outside `u16`. A
                // stricter rule here would be a fingerprint prompt for a server
                // the user already confirmed.
                let port = value
                    .as_deref()
                    .and_then(|t| t.parse::<u16>().ok())
                    .unwrap_or(default);
                config.insert(field.to_string(), Value::Number(port.into()));
            }
            Dest::Config(field) => {
                if let Some(value) = value {
                    config.insert(field.to_string(), Value::String(value));
                }
            }
            Dest::Local(field) => {
                local_fields.push(field.to_string());
                if let Some(value) = value {
                    local.insert(field.to_string(), Value::String(value));
                }
            }
            Dest::Secret(slot) => {
                if let Some(value) = value {
                    out_secrets.push((slot, value));
                }
            }
        }
    }

    Ok(Planned {
        account_kind,
        display_name: display_name(prefs, stored_kind)?,
        config,
        local,
        local_fields,
        secrets: out_secrets,
    })
}

/// A name for the account list that says which machine, folder or service this
/// is.
///
/// Derived from the target rather than from the protocol alone, because a user
/// with two of anything cannot tell "WebDAV" from "WebDAV". The old target had
/// no name at all — there was only ever one — so there is nothing to carry over
/// and this is the first name it gets. It is an ordinary display name
/// afterwards and the user can rename it.
///
/// The bare protocol names are the fallback for a target so incomplete that the
/// old builder could not have opened it either — a target that HAS a path this
/// cannot read is not that, so a failed read is propagated like every other one
/// here rather than falling back to a name that says the value was missing.
fn display_name(prefs: &UserPrefsRepo<'_>, stored_kind: &str) -> Result<String, MigrateError> {
    let pref_key = name_source(stored_kind).and_then(|form_key| {
        fields_for(stored_kind)?
            .iter()
            .find(|spec| spec.key == form_key)?
            .pref
    });
    let named_after = match pref_key {
        Some(key) => text_result(prefs, key).map_err(|err| MigrateError::Prefs(err.to_string()))?,
        None => None,
    };
    Ok(name_from(stored_kind, named_after))
}

/// The FORM key holding the value a kind names itself after.
///
/// Stated as a form key rather than a preference so the connect path — which
/// has the form and not the preferences — asks the same question and gets the
/// same answer. The migration turns it into a preference key through the
/// [`super::persist`] table, which is where that correspondence already lives.
pub(super) fn name_source(stored_kind: &str) -> Option<&'static str> {
    Some(match stored_kind {
        "local" | "dropbox" => "path",
        "webdav" => "url",
        "sftp" | "ftp" => "host",
        "googledrive" => "folder_name",
        _ => return None,
    })
}

/// Turn the value a kind names itself after into the name the account gets.
pub(super) fn name_from(stored_kind: &str, named_after: Option<String>) -> String {
    match stored_kind {
        "local" => named_after
            .map(|path| last_segment(&path).unwrap_or(path))
            .unwrap_or_else(|| "Sync folder".to_string()),
        "webdav" => named_after
            .map(|url| url_host(&url).unwrap_or(url))
            .unwrap_or_else(|| "WebDAV".to_string()),
        "sftp" => named_after.unwrap_or_else(|| "SFTP".to_string()),
        "ftp" => named_after.unwrap_or_else(|| "FTP".to_string()),
        // The two OAuth services have no host to name, and their folder is
        // usually the provider default, so the service leads and the folder
        // qualifies it where there is one.
        "dropbox" => qualified("Dropbox", named_after),
        "googledrive" => qualified("Google Drive", named_after),
        other => other.to_string(),
    }
}

fn qualified(service: &str, detail: Option<String>) -> String {
    match detail {
        Some(detail) => format!("{service} ({detail})"),
        None => service.to_string(),
    }
}

/// The last component of a path, with either separator — the folder the user
/// picked, rather than the whole route to it.
fn last_segment(path: &str) -> Option<String> {
    path.split(['/', '\\'])
        .map(str::trim)
        .rfind(|part| !part.is_empty())
        .map(str::to_string)
}

/// The `host[:port]` of a URL.
///
/// Hand-rolled rather than pulled in with a URL parser: this decides a display
/// name and nothing else, so being wrong about an exotic input costs a name the
/// user can edit, and `host-core` does not otherwise depend on one. Userinfo is
/// dropped — a name that reads `anna@cloud.example.com` puts an account name in
/// a place people copy out of.
fn url_host(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .trim();
    (!host.is_empty()).then(|| host.to_string())
}

// ── the migration ────────────────────────────────────────────────────────────

/// Turn this device's sync target into an account, once.
///
/// Returns the id of the account it created, or `None` when there was nothing to
/// migrate — no target configured, or this already ran.
///
/// ## The order of the writes, and why it is that order
///
/// The row, then this device's half, then the credentials, then the pointer,
/// then the marker. Each step is only reachable once the state it depends on
/// exists:
///
/// - The pointer comes after everything the row needs, because a pointer at a
///   half-configured account is worse than no pointer at all —
///   [`super::build_selected`] would fail on it while [`super::build`] would
///   still have opened the old target. It is written only if this device has
///   not already chosen one.
/// - The marker comes LAST, so an interrupted run is RETRIED rather than
///   recorded as done. The cost of retrying is a duplicate row, unreferenced and
///   deletable; the cost of not retrying is a device whose sync is half-migrated
///   and never repaired. Between a spare row in a list and a sync that stopped,
///   the row is the answer.
///
/// And at every point before the last one, the old preferences and the old
/// keychain entries are untouched and complete, so the device can still sync the
/// way it did yesterday.
pub fn migrate_to_account(
    prefs: &UserPrefsRepo<'_>,
    accounts: &AccountsRepo<'_>,
    secrets: &dyn SecretStore,
) -> Result<Option<String>, MigrateError> {
    // Already done — including when the account it names has since been
    // deleted. Deleting it was a decision, and re-creating a target the user
    // threw away is not a migration.
    //
    // The read is propagated rather than swallowed: a database that cannot
    // answer whether this ran must not be answered for, because the wrong
    // answer here is a second account and a repointed sync.
    let marker = prefs
        .get(PREF_MIGRATED_ACCOUNT)
        .map_err(|err| MigrateError::Prefs(err.to_string()))?;
    if marker.is_some_and(|id| !id.trim().is_empty()) {
        return Ok(None);
    }

    let stored_kind = prefs
        .get(PREF_ADAPTER_KIND)
        .map_err(|err| MigrateError::Prefs(err.to_string()))?;
    if is_unconfigured(stored_kind.as_deref()) {
        // Nothing to migrate, and no marker written: a device that never
        // configured a target has nothing to record, and the check above costs
        // one preference read.
        return Ok(None);
    }
    let stored_kind = stored_kind.unwrap_or_default();
    let plan = plan(prefs, secrets, stored_kind.trim())?;

    let account = accounts
        .create(
            AdapterKind::new(plan.account_kind),
            &plan.display_name,
            &Value::Object(plan.config).to_string(),
        )
        .map_err(|err| MigrateError::Accounts(err.to_string()))?;

    crate::account_local::store(prefs, &account.id, &plan.local_fields, &plan.local)
        .map_err(|err| MigrateError::Prefs(err.to_string()))?;

    for (slot, value) in &plan.secrets {
        secrets
            .store(&account.id, *slot, value)
            .map_err(|err| MigrateError::Secret(err.to_string()))?;
    }

    // Only when this device has not already chosen. A pointer that is already
    // set was set by the user on this build, and a migration may not move where
    // a device syncs behind their back — the migrated account is in the list
    // and they can pick it. It also makes an interrupted run converge: the
    // second attempt leaves the pointer on the first attempt's account, which
    // is the one that was finished.
    //
    // Read through the fallible twin rather than `selected_account_id`, which
    // answers "nobody chose" for a database that would not say. Overwriting a
    // choice that exists is exactly the harm the check above is here to
    // prevent, and it is not the kind that announces itself: the device simply
    // starts syncing somewhere else.
    if selected_account_id_result(prefs)
        .map_err(|err| MigrateError::Prefs(err.to_string()))?
        .is_none()
    {
        select_account(prefs, Some(&account.id))
            .map_err(|err| MigrateError::Prefs(err.to_string()))?;
    }
    prefs
        .set(PREF_MIGRATED_ACCOUNT, &account.id)
        .map_err(|err| MigrateError::Prefs(err.to_string()))?;
    Ok(Some(account.id))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use plugin_core::account_schema::AccountSchema;
    use plugin_core::manifest::PluginManifest;
    use sync_core::SyncAdapter;

    use super::*;
    use crate::db::DbHandle;
    use crate::sftp_host_keys::UserPrefsHostKeyVerifier;
    use crate::sync_target::persist::FakeSecrets;

    /// The manifests as they ship. Read rather than restated, so the field keys
    /// below are checked against what the plugins actually declare — the whole
    /// failure mode of a migration is a key that is one character off, and a
    /// hand-written twin of a schema would drift the same way every other
    /// hand-written twin in this workspace has.
    fn manifest_for(stored_kind: &str) -> PluginManifest {
        let bytes: &[u8] = match stored_kind {
            "local" => include_bytes!("../../../sync-adapter-local-plugin/plugin.json"),
            "webdav" => include_bytes!("../../../sync-adapter-webdav-plugin/plugin.json"),
            "sftp" => include_bytes!("../../../sync-adapter-sftp-plugin/plugin.json"),
            "ftp" => include_bytes!("../../../sync-adapter-ftp-plugin/plugin.json"),
            "dropbox" => include_bytes!("../../../sync-adapter-dropbox-plugin/plugin.json"),
            // Adopted by the Google adapter, which is where its schema
            // lives now — see `adopts_adapter_kinds`.
            "googledrive" => include_bytes!("../../../cal-adapter-google-plugin/plugin.json"),
            other => panic!("no shipped manifest for {other}"),
        };
        PluginManifest::from_bytes(bytes).expect("the shipped manifest parses")
    }

    fn schema_for(stored_kind: &str) -> AccountSchema {
        manifest_for(stored_kind)
            .account
            .expect("the shipped manifest declares an account schema")
    }

    /// Serves the shipped schema for whatever kind it is asked about, and
    /// records the config the plugin would have been opened with.
    #[derive(Default)]
    struct Plugins {
        schema: Option<AccountSchema>,
        seen: std::sync::Mutex<Vec<String>>,
    }

    impl SyncPlugins for Plugins {
        fn resolve(&self, adapter_kind: &str) -> Option<(String, AccountSchema)> {
            self.schema
                .clone()
                .map(|s| (format!("com.aperio.sync-adapter-{adapter_kind}"), s))
        }
        fn open(
            &self,
            _plugin_id: &str,
            config_json: String,
        ) -> Result<Arc<dyn SyncAdapter>, String> {
            self.seen.lock().unwrap().push(config_json);
            // Nothing here calls the adapter; every assertion is about the
            // config that reaches `open`.
            Err("captured".into())
        }
    }

    /// The same, for the OLD path: `build` takes a `PluginOpener`.
    #[derive(Default)]
    struct Opener {
        seen: std::sync::Mutex<Vec<String>>,
    }

    impl PluginOpener for Opener {
        fn open(
            &self,
            _plugin_id: &str,
            config_json: String,
        ) -> Result<Arc<dyn SyncAdapter>, String> {
            self.seen.lock().unwrap().push(config_json);
            Err("captured".into())
        }
    }

    struct Fixture {
        db: DbHandle,
        secrets: FakeSecrets,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                db: DbHandle::open_in_memory().unwrap(),
                secrets: FakeSecrets::default(),
            }
        }
    }

    /// A complete target of every kind, as the old code would have stored it.
    fn configure(prefs: &UserPrefsRepo<'_>, secrets: &FakeSecrets, stored_kind: &str) {
        let set = |key: &str, value: &str| prefs.set(key, value).unwrap();
        let keep = |account: &str, slot: SecretSlot, value: &str| {
            secrets.store(account, slot, value).unwrap()
        };
        match stored_kind {
            "local" => set(PREF_LOCAL_PATH, "/srv/aperio"),
            "webdav" => {
                set(
                    PREF_WEBDAV_URL,
                    "https://cloud.example.test/dav/anna/aperio/",
                );
                set(PREF_WEBDAV_USER, "anna");
                keep(SECRET_ACCOUNT_WEBDAV, SecretSlot::Password, "hunter2");
            }
            "sftp" => {
                set(PREF_SFTP_HOST, "backup.example.test");
                set(PREF_SFTP_PORT, "2222");
                set(PREF_SFTP_USER, "anna");
                set(PREF_SFTP_PATH, "/srv/aperio");
                set(PREF_SFTP_AUTH_METHOD, "password");
                keep(SECRET_ACCOUNT_SFTP, SecretSlot::Password, "hunter2");
                prefs
                    .set(
                        "sync.adapter.sftp.knownHosts.backup.example.test:2222",
                        "SHA256:abc",
                    )
                    .unwrap();
            }
            "ftp" => {
                set(PREF_FTP_HOST, "ftp.example.test");
                set(PREF_FTP_PORT, "2121");
                set(PREF_FTP_USER, "anna");
                set(PREF_FTP_PATH, "/aperio");
                set(PREF_FTP_MODE, "implicit");
                keep(SECRET_ACCOUNT_FTP, SecretSlot::Password, "hunter2");
            }
            "dropbox" => {
                set(PREF_DROPBOX_CLIENT_ID, "dbx-client");
                set(PREF_DROPBOX_CLIENT_SECRET, "dbx-secret");
                set(PREF_DROPBOX_PATH, "/Apps/Aperio");
                keep(
                    SECRET_ACCOUNT_DROPBOX,
                    SecretSlot::RefreshToken,
                    "dbx-refresh",
                );
            }
            "googledrive" => {
                set(PREF_GOOGLEDRIVE_CLIENT_ID, "gd-client");
                set(PREF_GOOGLEDRIVE_CLIENT_SECRET, "gd-secret");
                set(PREF_GOOGLEDRIVE_FOLDER_NAME, "Aperio");
                keep(
                    SECRET_ACCOUNT_GOOGLEDRIVE,
                    SecretSlot::RefreshToken,
                    "gd-refresh",
                );
            }
            other => panic!("no fixture for {other}"),
        }
        prefs.set(PREF_ADAPTER_KIND, stored_kind).unwrap();
    }

    fn parsed(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).expect("the config is a JSON object")
    }

    /// The old builder writes an explicit `""` where the account path simply
    /// omits a key and lets the plugin's own `#[serde(default)]` produce the
    /// same empty string. Comparing the two means ignoring that difference,
    /// which is a difference in how the same value is spelled and not in what
    /// the plugin ends up with.
    fn without_empty_strings(map: Map<String, Value>) -> Map<String, Value> {
        map.into_iter()
            .filter(|(_, value)| value.as_str() != Some(""))
            .collect()
    }

    /// THE test. For every kind, the plugin must be opened with what it would
    /// have been opened with yesterday.
    ///
    /// Both paths are driven end to end — `build` from the preferences, and
    /// `from_account` from the row this migration wrote, through the shipped
    /// schema — and the two configs are compared key by key. Nothing here
    /// restates what a field is called or where it belongs; the manifest and the
    /// old builder are the two authorities, and the migration has to agree with
    /// both at once.
    #[test]
    fn every_kind_reaches_the_plugin_with_what_it_reached_it_with_before() {
        for (stored_kind, _, _) in KIND_TABLE {
            let f = Fixture::new();
            let shared = f.db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            let accounts = AccountsRepo::new(&shared);
            configure(&prefs, &f.secrets, stored_kind);

            // What the old path hands over today.
            let opener = Opener::default();
            let _ = build(&prefs, &f.secrets, &opener);
            let before = without_empty_strings(parsed(
                opener
                    .seen
                    .lock()
                    .unwrap()
                    .first()
                    .unwrap_or_else(|| panic!("{stored_kind}: the old builder refused")),
            ));

            let id = migrate_to_account(&prefs, &accounts, &f.secrets)
                .unwrap_or_else(|err| panic!("{stored_kind}: {err}"))
                .expect("a configured target migrates");
            let account = accounts.get(&id).unwrap().expect("the row exists");

            let plugins = Plugins {
                schema: Some(schema_for(stored_kind)),
                ..Default::default()
            };
            let pins = UserPrefsHostKeyVerifier::new(shared.clone());
            let _ = from_account(&account, &prefs, &f.secrets, &pins, &plugins);
            let after = without_empty_strings(parsed(
                plugins
                    .seen
                    .lock()
                    .unwrap()
                    .first()
                    .unwrap_or_else(|| panic!("{stored_kind}: the account path refused")),
            ));

            assert_eq!(
                after, before,
                "{stored_kind}: the plugin would be opened differently after migrating",
            );
        }
    }

    /// The row is split the way the connect form splits one: nothing device-
    /// local in the column that travels, and no credential in it at all.
    #[test]
    fn the_row_carries_only_what_the_schema_lets_travel() {
        for (stored_kind, account_kind, moves) in KIND_TABLE {
            let f = Fixture::new();
            let shared = f.db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            let accounts = AccountsRepo::new(&shared);
            configure(&prefs, &f.secrets, stored_kind);

            let id = migrate_to_account(&prefs, &accounts, &f.secrets)
                .unwrap()
                .unwrap();
            let account = accounts.get(&id).unwrap().unwrap();
            assert_eq!(
                account.adapter_kind.as_str(),
                *account_kind,
                "{stored_kind}: wrong adapter kind",
            );
            // `serves_kind`, not `adapter_kind`: the migration writes the kind
            // the row will carry forever, and what has to hold is that some
            // shipped plugin RESOLVES it. `googledrive` is served by the Google
            // adapter, which adopted it when Drive folded in — the row is
            // unchanged, only the plugin behind it moved.
            assert!(
                manifest_for(stored_kind).serves_kind(account_kind),
                "{stored_kind}: no shipped manifest serves `{account_kind}`",
            );

            let schema = schema_for(stored_kind);
            let config = parsed(&account.config_json);
            for (key, _) in &config {
                let field = schema
                    .fields
                    .iter()
                    .find(|f| f.key == *key)
                    .unwrap_or_else(|| panic!("{stored_kind}: {key} is not a declared field"));
                assert!(
                    !field.stays_on_this_device(),
                    "{stored_kind}: {key} stays on this device and must not be in the row",
                );
            }
            // And every device-local, non-secret field the migration knows
            // about really is one the schema marks.
            for step in *moves {
                if let Dest::Local(key) = step.to {
                    let field = schema
                        .fields
                        .iter()
                        .find(|f| f.key == key)
                        .unwrap_or_else(|| panic!("{stored_kind}: {key} is not a declared field"));
                    assert!(
                        field.device_local && !field.is_secret(),
                        "{stored_kind}: {key} is stored per device but not declared that way",
                    );
                }
                if let Dest::Secret(slot) = step.to {
                    let declared = schema.fields.iter().any(|f| {
                        f.secret_slot
                            .is_some_and(|s| crate::account_setup::host_slot(s) == slot)
                    }) || (slot == SecretSlot::RefreshToken
                        && schema
                            .oauth
                            .as_ref()
                            .is_some_and(|o| o.refresh_token_field.is_some()));
                    assert!(
                        declared,
                        "{stored_kind}: nothing in the schema reads the {:?} slot",
                        slot,
                    );
                }
            }
        }
    }

    /// Every value the plugin cannot open without has to come across.
    #[test]
    fn every_required_field_is_covered_by_the_table() {
        for (stored_kind, _, moves) in KIND_TABLE {
            let schema = schema_for(stored_kind);
            let covered: Vec<&str> = moves
                .iter()
                .filter_map(|step| match step.to {
                    Dest::Config(key) | Dest::Local(key) | Dest::Port(key, _) => Some(key),
                    Dest::Secret(_) => None,
                })
                .collect();
            for field in schema.fields.iter().filter(|f| f.required) {
                if let Some(slot) = field.secret_slot {
                    let slot = crate::account_setup::host_slot(slot);
                    assert!(
                        moves
                            .iter()
                            .any(|step| matches!(step.to, Dest::Secret(s) if s == slot)),
                        "{stored_kind}: nothing fills the required secret {}",
                        field.key,
                    );
                } else {
                    assert!(
                        covered.contains(&field.key.as_str()),
                        "{stored_kind}: nothing fills the required field {}",
                        field.key,
                    );
                }
            }
        }
    }

    /// The trap named in three places: a port is a JSON number.
    #[test]
    fn the_port_is_a_number_in_the_row() {
        for (stored_kind, port) in [("sftp", 2222u16), ("ftp", 2121)] {
            let f = Fixture::new();
            let shared = f.db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            let accounts = AccountsRepo::new(&shared);
            configure(&prefs, &f.secrets, stored_kind);

            let id = migrate_to_account(&prefs, &accounts, &f.secrets)
                .unwrap()
                .unwrap();
            let config = parsed(&accounts.get(&id).unwrap().unwrap().config_json);
            assert_eq!(config.get("port"), Some(&Value::from(port)));
            assert!(
                config["port"].is_number(),
                "{stored_kind}: a string port fails the plugin's own deserialisation",
            );
        }
    }

    /// Getting these two backwards makes an SFTP user's key passphrase their
    /// password — which fails to authenticate, and sends a passphrase to the
    /// server in a password prompt.
    #[test]
    fn an_sftp_key_passphrase_does_not_become_a_password() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        configure(&prefs, &f.secrets, "sftp");
        // A user who has used both methods has both stored, which is the
        // arrangement `persist` exists to keep.
        prefs.set(PREF_SFTP_AUTH_METHOD, "key").unwrap();
        prefs
            .set(PREF_SFTP_KEY_PATH, "/home/anna/.ssh/id_ed25519")
            .unwrap();
        f.secrets
            .store(SECRET_ACCOUNT_SFTP_KEY, SecretSlot::Password, "keypass")
            .unwrap();

        let id = migrate_to_account(&prefs, &accounts, &f.secrets)
            .unwrap()
            .unwrap();

        assert_eq!(
            f.secrets.retrieve(&id, SecretSlot::KeyPassphrase).ok(),
            Some("keypass".to_string()),
            "the key passphrase did not reach its own slot",
        );
        assert_eq!(
            f.secrets.retrieve(&id, SecretSlot::Password).ok(),
            Some("hunter2".to_string()),
            "the password did not survive, or the passphrase overwrote it",
        );
        // The path and the method are this machine's answer and stay here.
        let account = accounts.get(&id).unwrap().unwrap();
        assert!(
            !account.config_json.contains("id_ed25519"),
            "a key path reached the row that travels: {}",
            account.config_json,
        );
        let local = crate::account_local::load(
            &prefs,
            &id,
            &["auth_method".to_string(), "key_path".to_string()],
        );
        assert_eq!(
            local.get("auth_method").and_then(Value::as_str),
            Some("key")
        );
        assert_eq!(
            local.get("key_path").and_then(Value::as_str),
            Some("/home/anna/.ssh/id_ed25519"),
        );
    }

    /// A fingerprint the user already confirmed must still be found. The lookup
    /// string is built from the migrated host and port, and it has to come out
    /// byte-identical to the one the old builder used — including where the
    /// port was never stored, or stored as something that is not a port.
    #[test]
    fn an_sftp_host_key_lookup_string_is_unchanged() {
        for (stored_port, expected) in [
            (Some("2222"), "backup.example.test:2222"),
            // Absent: the old builder substituted 22 and pinned under that.
            (None, "backup.example.test:22"),
            // Junk, and out of range — same substitution, same lookup.
            (Some("not-a-port"), "backup.example.test:22"),
            (Some("70000"), "backup.example.test:22"),
        ] {
            let f = Fixture::new();
            let shared = f.db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            let accounts = AccountsRepo::new(&shared);
            prefs.set(PREF_ADAPTER_KIND, "sftp").unwrap();
            prefs.set(PREF_SFTP_HOST, "backup.example.test").unwrap();
            prefs.set(PREF_SFTP_USER, "anna").unwrap();
            prefs.set(PREF_SFTP_PATH, "/srv/aperio").unwrap();
            prefs.set(PREF_SFTP_AUTH_METHOD, "password").unwrap();
            if let Some(port) = stored_port {
                prefs.set(PREF_SFTP_PORT, port).unwrap();
            }
            f.secrets
                .store(SECRET_ACCOUNT_SFTP, SecretSlot::Password, "hunter2")
                .unwrap();

            // Confirmed the way the dialog confirms it, under the key the OLD
            // path computed.
            let pins = UserPrefsHostKeyVerifier::new(shared.clone());
            pins.record(expected, "SHA256:abc");

            let id = migrate_to_account(&prefs, &accounts, &f.secrets)
                .unwrap()
                .unwrap();
            let account = accounts.get(&id).unwrap().unwrap();
            let plugins = Plugins {
                schema: Some(schema_for("sftp")),
                ..Default::default()
            };
            let _ = from_account(&account, &prefs, &f.secrets, &pins, &plugins);

            let config = parsed(&plugins.seen.lock().unwrap()[0]);
            assert_eq!(
                config.get("pinned_fingerprint").and_then(Value::as_str),
                Some("SHA256:abc"),
                "{stored_port:?}: the pin was not found under {expected}",
            );
        }
    }

    /// Twice is once. The second call must not create a second account, must
    /// not move the pointer, and must not touch the row the first one wrote.
    #[test]
    fn running_it_twice_creates_one_account() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        configure(&prefs, &f.secrets, "webdav");

        let before = accounts.list().unwrap().len();
        let first = migrate_to_account(&prefs, &accounts, &f.secrets)
            .unwrap()
            .expect("the first run migrates");
        assert_eq!(accounts.list().unwrap().len(), before + 1);

        assert_eq!(
            migrate_to_account(&prefs, &accounts, &f.secrets).unwrap(),
            None,
            "the second run migrated again",
        );
        assert_eq!(accounts.list().unwrap().len(), before + 1);
        assert_eq!(selected_account_id(&prefs).as_deref(), Some(first.as_str()));

        // Even after the user unmakes the choice — which is why the pointer
        // cannot be what says whether this ran.
        select_account(&prefs, None).unwrap();
        assert_eq!(
            migrate_to_account(&prefs, &accounts, &f.secrets).unwrap(),
            None,
        );
        assert_eq!(accounts.list().unwrap().len(), before + 1);

        // And after they delete the account outright: that was a decision, not
        // an accident to repair.
        accounts.delete(&first).unwrap();
        assert_eq!(
            migrate_to_account(&prefs, &accounts, &f.secrets).unwrap(),
            None,
        );
    }

    #[test]
    fn nothing_configured_migrates_nothing() {
        for stored in [None, Some(""), Some("   "), Some("none")] {
            let f = Fixture::new();
            let shared = f.db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            let accounts = AccountsRepo::new(&shared);
            if let Some(kind) = stored {
                prefs.set(PREF_ADAPTER_KIND, kind).unwrap();
            }
            let before = accounts.list().unwrap().len();

            assert_eq!(
                migrate_to_account(&prefs, &accounts, &f.secrets).unwrap(),
                None,
                "{stored:?} is not a configured target",
            );
            assert_eq!(accounts.list().unwrap().len(), before);
            assert_eq!(selected_account_id(&prefs), None);
            // No marker either: nothing ran, and a device that configures a
            // target on an older build afterwards still gets migrated.
            assert_eq!(prefs.get(PREF_MIGRATED_ACCOUNT).unwrap(), None);
        }
    }

    /// The rule the whole design rests on. If the process dies after this, or
    /// the next release's reader is on the old path, the device must still have
    /// everything it needs to sync the way it did yesterday.
    #[test]
    fn the_old_prefs_and_the_old_keychain_still_hold_their_values() {
        for (stored_kind, _, _) in KIND_TABLE {
            let f = Fixture::new();
            let shared = f.db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            let accounts = AccountsRepo::new(&shared);
            configure(&prefs, &f.secrets, stored_kind);

            // Everything the old path can read, before.
            let before = restore(&prefs, &f.secrets).expect("configured");
            migrate_to_account(&prefs, &accounts, &f.secrets).unwrap();
            let after = restore(&prefs, &f.secrets).expect("still configured");

            assert_eq!(after, before, "{stored_kind}: the old target changed");
            assert_eq!(
                prefs.get(PREF_ADAPTER_KIND).unwrap().as_deref(),
                Some(*stored_kind),
                "{stored_kind}: the kind was cleared",
            );
            // And the old builder still opens it.
            let opener = Opener::default();
            let _ = build(&prefs, &f.secrets, &opener);
            assert!(
                !opener.seen.lock().unwrap().is_empty(),
                "{stored_kind}: the old path can no longer build the target",
            );
        }
    }

    /// The E2E key belongs to the dataset, not to the target. It stays in the
    /// pseudo-account it has always been in, the flag stays where it is, and
    /// neither is copied onto the account — an account-scoped copy would be a
    /// second answer to a question that has one.
    #[test]
    fn the_encryption_key_and_its_flag_do_not_move() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        configure(&prefs, &f.secrets, "webdav");
        prefs
            .set(sync_engine::whitelist::PREF_E2E_ENABLED, "true")
            .unwrap();
        f.secrets
            .store(
                SECRET_ACCOUNT_E2E,
                SecretSlot::SyncEncryptionKey,
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            )
            .unwrap();

        let id = migrate_to_account(&prefs, &accounts, &f.secrets)
            .unwrap()
            .unwrap();

        assert_eq!(
            f.secrets
                .retrieve(SECRET_ACCOUNT_E2E, SecretSlot::SyncEncryptionKey)
                .ok()
                .as_deref(),
            Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="),
        );
        assert!(e2e_enabled(&prefs), "the flag moved or was cleared");
        assert!(
            f.secrets
                .retrieve(&id, SecretSlot::SyncEncryptionKey)
                .is_err(),
            "the data key was copied onto the account",
        );
    }

    /// A name someone can pick out of a list, taken from the target itself.
    #[test]
    fn the_name_says_which_target_this_is() {
        for (stored_kind, expected) in [
            ("local", "aperio"),
            ("webdav", "cloud.example.test"),
            ("sftp", "backup.example.test"),
            ("ftp", "ftp.example.test"),
            ("dropbox", "Dropbox (/Apps/Aperio)"),
            ("googledrive", "Google Drive (Aperio)"),
        ] {
            let f = Fixture::new();
            let shared = f.db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            let accounts = AccountsRepo::new(&shared);
            configure(&prefs, &f.secrets, stored_kind);

            let id = migrate_to_account(&prefs, &accounts, &f.secrets)
                .unwrap()
                .unwrap();
            assert_eq!(
                accounts.get(&id).unwrap().unwrap().display_name,
                expected,
                "{stored_kind}",
            );
        }
    }

    /// Userinfo in a WebDAV URL must not become the account's name — it is the
    /// one part of a URL people are told not to leave lying around.
    #[test]
    fn a_url_name_drops_the_user_and_keeps_the_port() {
        assert_eq!(
            url_host("https://anna:pw@cloud.example.test:8443/dav/"),
            Some("cloud.example.test:8443".to_string()),
        );
        assert_eq!(
            url_host("cloud.example.test/dav"),
            Some("cloud.example.test".into())
        );
        assert_eq!(url_host(""), None);
    }

    /// An unrecognised SFTP auth method resolves the way the old builder
    /// resolved it, rather than being echoed into a plugin that refuses it.
    ///
    /// The normalisation is not tidiness — it is the difference between a
    /// device that keeps syncing and one that stops. Anything that was not
    /// `key` authenticated with the password, and still has to.
    #[test]
    fn an_unknown_sftp_auth_method_is_normalised_the_way_the_old_builder_normalised_it() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        configure(&prefs, &f.secrets, "sftp");
        prefs
            .set(PREF_SFTP_AUTH_METHOD, "gssapi-from-the-future")
            .unwrap();
        let id = migrate_to_account(&prefs, &accounts, &f.secrets)
            .unwrap()
            .unwrap();
        let local = crate::account_local::load(&prefs, &id, &["auth_method".to_string()]);
        assert_eq!(
            local.get("auth_method").and_then(Value::as_str),
            Some("password"),
        );
    }

    /// The opposite rule for the FTPS mode, and the opposite reason.
    ///
    /// The old builder REFUSED an unrecognised mode, so nothing is syncing with
    /// one and there is no working state to preserve — while normalising it to
    /// `explicit` would write that into the row, where it becomes
    /// indistinguishable from a mode the user picked. A `plain` or `implicit`
    /// this build failed to recognise must not come back as a TLS setting
    /// nobody chose; the plugin refuses it by name instead.
    #[test]
    fn an_unknown_ftps_mode_is_carried_through_rather_than_replaced() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        configure(&prefs, &f.secrets, "ftp");
        prefs.set(PREF_FTP_MODE, "implicid").unwrap();

        let id = migrate_to_account(&prefs, &accounts, &f.secrets)
            .unwrap()
            .unwrap();
        let config = parsed(&accounts.get(&id).unwrap().unwrap().config_json);
        assert_eq!(
            config.get("mode").and_then(Value::as_str),
            Some("implicid"),
            "the migration chose a transport the user did not",
        );
        // And the old path refuses it too, so nothing was laundered: the value
        // was unusable before the migration and is unusable after it, which is
        // the honest outcome.
        assert_eq!(
            build(&prefs, &f.secrets, &Opener::default()).err(),
            Some(Unbuildable::Invalid {
                field: "mode",
                value: "implicid".to_string(),
            }),
        );
    }

    /// An absent mode still ends up explicit, which is what the old builder
    /// wrote — by way of the plugin's own default rather than a value in the
    /// row, and those are the same thing to the adapter.
    #[test]
    fn an_absent_ftps_mode_leaves_the_plugins_default_to_answer() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        configure(&prefs, &f.secrets, "ftp");
        prefs.delete(PREF_FTP_MODE).unwrap();

        let id = migrate_to_account(&prefs, &accounts, &f.secrets)
            .unwrap()
            .unwrap();
        let config = parsed(&accounts.get(&id).unwrap().unwrap().config_json);
        assert_eq!(config.get("mode"), None);
        assert_eq!(
            schema_for("ftp")
                .field("mode")
                .and_then(|f| f.default.clone()),
            Some(plugin_core::account_schema::AccountFieldDefault::Text(
                "explicit".to_string()
            )),
            "the adapter's own default is what an absent mode now resolves to",
        );
    }

    /// A credential is moved byte for byte.
    ///
    /// Trimming one is the kind of helpfulness that costs a person their sync
    /// and tells them nothing: a password with a leading or trailing space —
    /// generators emit them, and a paste keeps them — authenticates only in its
    /// exact form, and what the user sees afterwards is the right password
    /// being rejected. A secret that is nothing but spaces is still a secret
    /// somebody stored, and it travels too.
    #[test]
    fn a_padded_secret_crosses_unchanged() {
        for password in ["  hunter2  ", "\tspaces\n", "   "] {
            let f = Fixture::new();
            let shared = f.db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            let accounts = AccountsRepo::new(&shared);
            configure(&prefs, &f.secrets, "webdav");
            f.secrets
                .store(SECRET_ACCOUNT_WEBDAV, SecretSlot::Password, password)
                .unwrap();

            let id = migrate_to_account(&prefs, &accounts, &f.secrets)
                .unwrap()
                .unwrap();
            assert_eq!(
                f.secrets
                    .retrieve(&id, SecretSlot::Password)
                    .ok()
                    .as_deref(),
                Some(password),
                "the migration altered a stored credential",
            );

            // And the plugin is handed the same bytes it would have been handed
            // yesterday — which is the assertion that matters, since that is
            // what reaches the server.
            let account = accounts.get(&id).unwrap().unwrap();
            let plugins = Plugins {
                schema: Some(schema_for("webdav")),
                ..Default::default()
            };
            let pins = UserPrefsHostKeyVerifier::new(shared.clone());
            let _ = from_account(&account, &prefs, &f.secrets, &pins, &plugins);
            let config = parsed(&plugins.seen.lock().unwrap()[0]);
            assert_eq!(
                config.get("password").and_then(Value::as_str),
                Some(password),
            );
        }
    }

    /// A kind with no field table cannot be translated, and guessing would
    /// write an account nobody can open. It is refused, and — because nothing
    /// is deleted and no marker is written — the old target is untouched and a
    /// build that knows the kind can still migrate it.
    #[test]
    fn a_kind_this_build_does_not_serve_is_refused_rather_than_guessed() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        prefs.set(PREF_ADAPTER_KIND, "nextcloud").unwrap();
        let before = accounts.list().unwrap().len();

        let err = migrate_to_account(&prefs, &accounts, &f.secrets).expect_err("must refuse");
        assert!(
            matches!(err, MigrateError::UnknownKind(ref k) if k == "nextcloud"),
            "{err}"
        );
        assert_eq!(accounts.list().unwrap().len(), before);
        assert_eq!(prefs.get(PREF_MIGRATED_ACCOUNT).unwrap(), None);
        assert_eq!(
            prefs.get(PREF_ADAPTER_KIND).unwrap().as_deref(),
            Some("nextcloud"),
        );
    }

    /// A kind this build can sync through is a kind this build can migrate.
    ///
    /// Walking [`KINDS`] rather than repeating it: a seventh adapter added
    /// there and forgotten here would refuse to migrate on every device that
    /// uses it, and the only symptom is a sync that stops the release after
    /// this one.
    #[test]
    fn every_kind_this_build_serves_has_a_field_table() {
        for (kind, _) in KINDS {
            assert!(
                table_for(kind).is_some(),
                "{kind} can be configured but not migrated",
            );
        }
        for (stored, _, _) in KIND_TABLE {
            assert!(
                plugin_id_for_kind(stored).is_some(),
                "{stored} is migrated but is not a kind this build serves",
            );
        }
    }

    /// A choice this device has already made is not overruled.
    ///
    /// The migrated account is created and recorded either way — it is the
    /// user's old target and they should be able to pick it — but where the
    /// device syncs is a decision, and a migration silently moving it would
    /// send the next round somewhere the user did not choose.
    #[test]
    fn an_account_this_device_already_syncs_through_is_left_selected() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        configure(&prefs, &f.secrets, "webdav");
        let chosen = accounts
            .create(AdapterKind::new("webdav"), "Chosen by hand", "{}")
            .unwrap();
        select_account(&prefs, Some(&chosen.id)).unwrap();

        let migrated = migrate_to_account(&prefs, &accounts, &f.secrets)
            .unwrap()
            .expect("the old target still becomes an account");
        assert_ne!(migrated, chosen.id);
        assert_eq!(
            selected_account_id(&prefs).as_deref(),
            Some(chosen.id.as_str()),
            "the migration moved where this device syncs",
        );
        assert_eq!(
            prefs.get(PREF_MIGRATED_ACCOUNT).unwrap().as_deref(),
            Some(migrated.as_str()),
        );
    }

    /// A keychain that will not answer must not be read as a keychain with
    /// nothing in it.
    ///
    /// The difference decides whether one launch with a locked keystore costs
    /// the user their sync: an account written without its password, plus a
    /// marker saying the migration is done, is a state nothing ever repairs.
    /// Refusing leaves everything as it was and the next launch tries again.
    #[test]
    fn a_keychain_that_will_not_answer_stops_the_migration() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        configure(&prefs, &f.secrets, "webdav");
        let before = accounts.list().unwrap().len();

        let err = migrate_to_account(&prefs, &accounts, &Unavailable(f.secrets))
            .expect_err("must refuse");
        assert!(matches!(err, MigrateError::Secret(_)), "{err}");
        assert_eq!(accounts.list().unwrap().len(), before);
        assert_eq!(prefs.get(PREF_MIGRATED_ACCOUNT).unwrap(), None);
        assert_eq!(selected_account_id(&prefs), None);
    }

    /// A keychain that is present but refuses to read — locked, or busy.
    /// Wraps the shared fake so that only the one behaviour under test differs
    /// from every other test in this file.
    struct Unavailable(FakeSecrets);

    impl SecretStore for Unavailable {
        fn store(
            &self,
            account_id: &str,
            slot: SecretSlot,
            value: &str,
        ) -> Result<(), sync_engine::SecretError> {
            self.0.store(account_id, slot, value)
        }
        fn retrieve(
            &self,
            _account_id: &str,
            _slot: SecretSlot,
        ) -> Result<String, sync_engine::SecretError> {
            Err(sync_engine::SecretError::Backend("locked".into()))
        }
        fn delete(
            &self,
            account_id: &str,
            slot: SecretSlot,
        ) -> Result<(), sync_engine::SecretError> {
            self.0.delete(account_id, slot)
        }
        fn delete_all(&self, account_id: &str) -> Result<(), sync_engine::SecretError> {
            self.0.delete_all(account_id)
        }
    }

    /// A preference store that will not ANSWER must not be read as a device
    /// that never configured the value.
    ///
    /// `UserPrefsRepo::get` used to return the same `None` for both, and this
    /// file was written under that rule. The primitive is fixed; the migration
    /// asks for the distinction itself rather than depending on nobody ever
    /// collapsing it again — because the cost here is not one failed launch. An
    /// account written without the URL, the user name or the key path that
    /// could not be read, plus a marker saying the migration is done, is a
    /// half-migrated device that nothing ever revisits.
    ///
    /// Dropping the table is the bluntest form of "the database will not
    /// answer", and it is what a locked or damaged one looks like from here.
    #[test]
    fn a_preference_that_cannot_be_read_stops_the_migration() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        configure(&prefs, &f.secrets, "webdav");
        let before = accounts.list().unwrap().len();

        shared
            .lock()
            .unwrap()
            .execute("DROP TABLE user_prefs", [])
            .unwrap();

        // The planning reads, where a swallowed failure would have produced an
        // account missing whatever could not be read. `display_name` reads the
        // same keys through the same helper and is reached from here.
        // Matched rather than unwrapped: `Planned` has no `Debug`, on purpose —
        // it holds cleartext credentials.
        match plan(&prefs, &f.secrets, "webdav") {
            Err(MigrateError::Prefs(_)) => {}
            Err(other) => panic!("wrong refusal: {other}"),
            Ok(_) => panic!("an unreadable preference was read as an absent one"),
        }

        // And end to end: nothing is created, and — the part that matters —
        // nothing is recorded as done, so the next launch tries again.
        let err = migrate_to_account(&prefs, &accounts, &f.secrets).expect_err("must refuse");
        assert!(matches!(err, MigrateError::Prefs(_)), "{err}");
        assert_eq!(accounts.list().unwrap().len(), before);
    }

    /// Every key this file writes is this device's own. The marker in
    /// particular: if it crossed, the second device would believe it had
    /// already migrated a target it has never seen and would never migrate its
    /// own.
    #[test]
    fn nothing_this_writes_crosses_devices() {
        assert!(!sync_engine::whitelist::is_synced_key(
            PREF_MIGRATED_ACCOUNT
        ));
        assert!(!sync_engine::whitelist::is_synced_key(
            PREF_SELECTED_ACCOUNT
        ));
    }
}
