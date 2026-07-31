//! Write down everything you would have to type back in.
//!
//! A developer tool, not part of the app. It exists for one workflow: wipe the
//! data directory and walk the whole first-run path again — every adapter, from
//! nothing — without regenerating the app-specific passwords that are still in
//! use by the sync targets on other devices.
//!
//! ## Why it reads the database too
//!
//! The keychain holds only the secret half. Which server that password belongs
//! to, the CalDAV collection URL, the EWS endpoint, an OAuth client id, the
//! mailbox address — all of that lives in `accounts.config_json` and in
//! `user_prefs`, both inside the data directory that this workflow deletes. A
//! dump of the keychain alone leaves you in front of a CalDAV dialog holding a
//! password and no URL, so this reads both and reports them together, per
//! account, in the order the connect form asks for them.
//!
//! ## What it never does
//!
//! Nothing is written, deleted or modified: the database is opened read-only
//! and the keychain only ever read. The report is the only output, and it never
//! goes to stdout — stdout gets counts and the file path, so that piping this
//! into a terminal that scrolls back, into a log, or past someone's shoulder
//! does not spill the values. Treat the report the way you would treat the
//! passwords themselves, and delete it when the experiment is over.
//!
//! ## Usage
//!
//! ```text
//! cargo run -p dev-secret-export
//! cargo run -p dev-secret-export -- C:\some\where\else.txt
//! ```
//!
//! Without an argument the report lands next to the data directory as
//! `aperio-credentials.txt`, which is outside the repository by construction.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use sync_engine::SecretSlot;

/// Every slot the app can write. Probed for each account id, because which
/// slots an account uses is the adapter's business and an account that changed
/// hands (a password login later moved to OAuth) can hold more than one.
const ALL_SLOTS: &[SecretSlot] = &[
    SecretSlot::Password,
    SecretSlot::ApiToken,
    SecretSlot::AccessToken,
    SecretSlot::RefreshToken,
    SecretSlot::OauthClientSecret,
    SecretSlot::SyncEncryptionKey,
];

/// The keychain naming scheme, mirrored from `src-tauri/src/secrets.rs`. Keep
/// the two in step: service is `Aperio:<slot wire name>`, user is the account
/// id. The wire names come from `SecretSlot` itself, so only the prefix is
/// duplicated here.
const SERVICE_PREFIX: &str = "Aperio";

fn read_slot(account_id: &str, slot: SecretSlot) -> Option<String> {
    let service = format!("{SERVICE_PREFIX}:{}", slot.wire_name());
    keyring::Entry::new(&service, account_id)
        .ok()?
        .get_password()
        .ok()
}

struct Account {
    id: String,
    adapter_kind: String,
    display_name: String,
    config_json: String,
}

fn main() -> Result<()> {
    let data_dir = host_core::paths::resolve_data_dir();
    let db_path = data_dir.path.join("aperio.sqlite");

    let out_path: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.path.join("aperio-credentials.txt"));

    // Read-only first, which is what this tool wants. It can legitimately fail
    // on a WAL database: recovering an uncheckpointed write-ahead log needs to
    // create the `-shm` file, and a read-only handle cannot. Falling back is
    // safe — that recovery is exactly what the app itself does on its next
    // start, and nothing here issues a statement that changes a row — but say
    // so rather than doing it quietly.
    let db = match Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(db) => db,
        Err(err) => {
            eprintln!("Nur-Lesen-Zugriff nicht möglich ({err}).");
            eprintln!("Zweiter Versuch mit Schreibrecht — nötig, um ein offenes");
            eprintln!("Write-Ahead-Log zu übernehmen. Es wird nichts verändert.");
            eprintln!("Aperio sollte dabei geschlossen sein.");
            Connection::open(&db_path).with_context(|| format!("opening {}", db_path.display()))?
        }
    };

    let accounts = load_accounts(&db)?;
    let prefs = load_sync_prefs(&db)?;

    // Sync targets are not accounts. They live in device-local prefs under
    // `sync.adapter.<name>.*` and keep their credential in a keychain
    // pseudo-account — deliberately divorced from any account row, so nothing
    // in the accounts table points at them.
    //
    // These ids are listed rather than derived from the pref keys, because
    // deriving them silently missed the two that matter most. `sync.adapter.e2e`
    // holds the SYNC ENCRYPTION KEY, and its only pref is `sync.adapter.e2e` +
    // "Enabled" — no dot, so splitting on the dot never produced the id. Losing
    // that key means losing the ability to rejoin an existing sync at all, which
    // is precisely the workflow this tool exists to make safe.
    // `sync.adapter.sftp.key` is a level deeper than the split produced.
    //
    // Mirrors the *_SECRET_ACCOUNT constants in src-tauri/src/commands/sync.rs.
    const SYNC_SECRET_ACCOUNTS: &[&str] = &[
        "sync.adapter.webdav",
        "sync.adapter.sftp",
        // The SSH key passphrase, kept apart from the password slot.
        "sync.adapter.sftp.key",
        "sync.adapter.ftp",
        "sync.adapter.dropbox",
        "sync.adapter.googledrive",
        // The E2E data key. Without this, a wiped install can start a NEW sync
        // but can never rejoin the existing one.
        "sync.adapter.e2e",
    ];

    let mut pseudo: BTreeSet<String> = SYNC_SECRET_ACCOUNTS
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    // Keep the derivation too, as a net for an adapter added after this list was
    // written: a pref key it left behind still gets its slots probed.
    for (key, _) in &prefs {
        if let Some(rest) = key.strip_prefix("sync.adapter.") {
            if let Some((name, _)) = rest.split_once('.') {
                pseudo.insert(format!("sync.adapter.{name}"));
            }
        }
    }
    // Older builds hung the encryption key off a bare `sync` id. Probing costs
    // one lookup and answering "(keine)" is cheaper than a lost key.
    pseudo.insert("sync".to_string());

    let mut report = String::new();
    let mut secrets_found = 0usize;

    writeln!(report, "Aperio — Zugangsdaten zum Wiedereintragen")?;
    writeln!(report, "========================================")?;
    writeln!(report)?;
    writeln!(report, "Datenverzeichnis: {}", data_dir.path.display())?;
    writeln!(report, "Datenbank:        {}", db_path.display())?;
    writeln!(report)?;
    writeln!(
        report,
        "Diese Datei enthält Passwörter und Token im Klartext. Nach dem"
    )?;
    writeln!(report, "Wiedereintragen löschen.")?;
    writeln!(report)?;
    writeln!(report, "Konten insgesamt: {}", accounts.len())?;
    writeln!(report)?;

    for (index, account) in accounts.iter().enumerate() {
        writeln!(report, "----------------------------------------")?;
        writeln!(report, "Konto {} von {}", index + 1, accounts.len())?;
        writeln!(report, "  Anzeigename: {}", account.display_name)?;
        writeln!(report, "  Adaptertyp:  {}", account.adapter_kind)?;
        writeln!(report, "  Konto-ID:    {}", account.id)?;
        writeln!(report)?;

        writeln!(report, "  Konfiguration (aus der Datenbank):")?;
        match serde_json::from_str::<serde_json::Value>(&account.config_json) {
            Ok(serde_json::Value::Object(map)) if !map.is_empty() => {
                for (key, value) in &map {
                    writeln!(report, "    {key} = {}", render(value))?;
                }
            }
            Ok(_) => writeln!(report, "    (leer)")?,
            // Not valid JSON is worth seeing verbatim rather than swallowing:
            // it is the only copy of these values you have left.
            Err(err) => {
                writeln!(report, "    (kein gültiges JSON: {err})")?;
                writeln!(report, "    Rohwert: {}", account.config_json)?;
            }
        }
        writeln!(report)?;

        writeln!(report, "  Geheimnisse (aus dem Schlüsselbund):")?;
        let mut any = false;
        for slot in ALL_SLOTS {
            if let Some(value) = read_slot(&account.id, *slot) {
                writeln!(report, "    {} = {value}", slot.wire_name())?;
                any = true;
                secrets_found += 1;
            }
        }
        if !any {
            writeln!(report, "    (keine)")?;
        }
        writeln!(report)?;
    }

    writeln!(report, "----------------------------------------")?;
    writeln!(report, "Sync-Ziele (keine Konten — geräte-lokal)")?;
    writeln!(report)?;
    if prefs.is_empty() {
        writeln!(report, "  (keine sync.*-Einstellungen gefunden)")?;
    } else {
        writeln!(report, "  Einstellungen:")?;
        for (key, value) in &prefs {
            writeln!(report, "    {key} = {value}")?;
        }
    }
    writeln!(report)?;
    writeln!(report, "  Geheimnisse dieser Pseudo-Konten:")?;
    let mut any_pseudo = false;
    for id in &pseudo {
        for slot in ALL_SLOTS {
            if let Some(value) = read_slot(id, *slot) {
                writeln!(report, "    {id} / {} = {value}", slot.wire_name())?;
                any_pseudo = true;
                secrets_found += 1;
            }
        }
    }
    if !any_pseudo {
        writeln!(report, "    (keine)")?;
    }

    std::fs::write(&out_path, report).with_context(|| format!("writing {}", out_path.display()))?;

    // Counts only. The values stay in the file, on purpose.
    println!("Konten gelesen:      {}", accounts.len());
    println!("Geheimnisse gefunden: {secrets_found}");
    println!("Sync-Einstellungen:   {}", prefs.len());
    println!();
    println!("Bericht geschrieben nach:");
    println!("{}", out_path.display());
    println!();
    println!("Die Datei enthält Klartext-Zugangsdaten. Nach Gebrauch löschen.");

    Ok(())
}

fn load_accounts(db: &Connection) -> Result<Vec<Account>> {
    let mut stmt = db.prepare(
        "SELECT id, adapter_kind, display_name, config_json \
         FROM accounts ORDER BY adapter_kind, display_name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Account {
            id: row.get(0)?,
            adapter_kind: row.get(1)?,
            display_name: row.get(2)?,
            config_json: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn load_sync_prefs(db: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt =
        db.prepare("SELECT key, value FROM user_prefs WHERE key LIKE 'sync.%' ORDER BY key")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Strings without their JSON quotes, everything else as written. A URL that
/// has to be retyped should not arrive wrapped in punctuation the form does not
/// want.
fn render(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
