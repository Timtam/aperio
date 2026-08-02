# Design-Dokument: Extraktion der Sync-Engine in ein wiederverwendbares Crate (`sync-engine`)

**Status:** Entwurf zur Freigabe.
**Bezug:** Ergänzt `DESIGN.md` §19 (Geräteübergreifende Datenbanksynchronisation) und §6 (Adapter-/Crate-Architektur). Voraussetzung für die mobile App (Voll-Sync wie am Desktop).
**Nach Freigabe:** in `DESIGN.md` einarbeiten (neuer Abschnitt bei §19) und das Dokument hier entfernen.

---

## 1. Motivation & Ziel

Die mobile App (React Native + geteilter Rust-Core via UniFFI) soll am **selben Event-Log-Sync** teilnehmen wie der Desktop (§19): Geräte schreiben Mutationen als append-only JSONL-Logs, ziehen fremde Logs, mergen **feldweise (Last-write-wins)** und kompaktieren über Snapshots.

Das **Modell** (`sync-core`) und die **Sync-Adapter** (WebDAV/SFTP/FTP/Dropbox/GoogleDrive/Local) sind bereits pures, portierbares Rust. Die **Orchestrierung** der Engine liegt aber heute in `src-tauri/src/event_log/` und ist an die Desktop-App gekoppelt:

- SQLite über `SharedConn` (`Arc<Mutex<rusqlite::Connection>>`) + `adapter-local`,
- Dateisystem über `tokio::fs`/`std::fs` (Pending-Logs, Sound-Assets) + `paths::resolve_data_dir()`,
- Secrets über das `keyring`-Crate (`src-tauri/src/secrets.rs`),
- Zeit über `Utc::now()`,
- UI-Statusmeldungen über Tauri-`State` + `app.emit(...)`.

**Ziel:** Die Orchestrierung in ein eigenständiges **`sync-engine`-Crate** ziehen, dessen plattformspezifische Abhängigkeiten hinter **Traits** liegen. Desktop und Mobile liefern je eine Implementierung; die Engine-Logik ist 1:1 geteilt.

**Wichtigste Randbedingung:** **verhaltenswahrend für den Desktop** — kein Funktionsunterschied, nur Umstrukturierung. Die bestehende Desktop-Testsuite ist das Sicherheitsnetz und muss bei jedem Schritt grün bleiben.

---

## 2. Crate-Schnitt

### 2.1 Zieht in `sync-engine` (pure Logik, ~5.500 Zeilen)

| Modul | Heute | Rolle |
| --- | --- | --- |
| `EventLogWriter` | `event_log/mod.rs` | hängt Mutationen als JSONL ans Session-Log; rotiert |
| `EventLogApplier` | `event_log/applier.rs` | wendet fremde Events auf den lokalen Speicher an, feldweiser Merge, Konflikterkennung |
| `SyncOrchestrator` | `event_log/orchestrator.rs` | eine Sync-Runde: push pending → fetch remote → apply → Cursor weiter → ggf. Snapshot/Kompaktierung |
| `Compactor` | `event_log/compactor.rs` | Snapshot-Generierung + Log-GC |
| `SnapshotBuilder` | `event_log/snapshot.rs` | Snapshot bauen/anwenden |

### 2.2 Bleibt in `src-tauri` (App-Glue)

- **`SyncScheduler`** (`event_log/scheduler.rs`) — `tokio::select!`-Loop (periodisch + Mutations-Debounce) + `app.emit(...)`. Runtime-/UI-spezifisch; **Mobile bekommt einen eigenen Treiber** (Vordergrund + `BGTaskScheduler`/`WorkManager`).
- **Tauri-Commands** (`commands/sync.rs`: `configure_sync_adapter`, `sync_now`, `get_sync_status`, `set_sync_interval`, OAuth/Host-Key-Flows …) — die IPC-Oberfläche; Mobile hat ein natives Pendant.
- **Wiring** (`lib.rs` `.setup()`, Tauri-`State`).
- **`secrets.rs`** (Keyring) — bleibt, aber hinter dem `SecretStore`-Trait.
- Die `user_prefs`/Repos bleiben als **Implementierungsdetail** des Desktop-`SyncStore`.

### 2.3 Begründung

Der Scheduler und die Commands sind plattform- und runtime-gebunden; die Engine selbst ist es nicht. Indem nur die reine Logik wandert und die Plattformpunkte hinter Traits liegen, läuft dieselbe Engine unter Tauri (Desktop) und unter UniFFI (Mobile), ohne Duplizierung.

---

## 3. Die Trait-Nahtstellen

Fünf Traits kapseln die plattformspezifischen Abhängigkeiten. Desktop und Mobile liefern je eine Implementierung. (Genaue Methodenlisten ergeben sich aus dem Scoping; unten die Kategorien + Schlüsselmethoden.)

### 3.1 `SyncStore` — Datenbank + lokaler Datenspeicher

Die größte Nahtstelle. Kapselt sowohl die sync-eigenen Tabellen als auch den lokalen Datenspeicher. **DB-Methoden bleiben synchron** (die `SharedConn` ist single-threaded mit `Mutex`); nur Adapter/Dateisystem sind async.

- **Idempotenz:** `is_event_applied(id)`, `mark_event_applied(id, now)` (Tabelle `sync_applied_events`).
- **Cursor/Watermark:** `get/set_sync_cursor`, `get/set_last_round_timestamp`.
- **Geräte-ID:** `load_or_mint_device_id`, `set_device_id` (in `user_prefs`).
- **Einstellungen:** `get/set/delete_user_pref`, `dump_synced_prefs` (Whitelist, §19.2.1).
- **Konflikte:** `record_conflict(...)` (Tabelle `sync_conflicts`).
- **Lokale Daten (Upsert/Get/Delete je Typ):** Events, Tasks, Task-Lists, Calendars, Color-Labels, Sections — spiegelt die `*_from_sync`-Helfer von `adapter-local`.
- **Accounts:** `upsert_account`, `delete_account`, `dump_accounts` (nur Nicht-Secrets).
- **Snapshot:** `dump_for_snapshot`, `apply_snapshot_dump`.
- **Kompaktierung:** Zähler lesen/erhöhen/zurücksetzen.
- **Audit:** `record_sync_log(...)` (Settings-Protokoll); `upsert_device_name(...)`.

**Desktop-Impl:** dünner Wrapper um `SharedConn` + `LocalAdapter` + die bestehenden Repos.
**Mobile-Impl:** eigenes `rusqlite` (gleiches Schema) im App-Sandbox.

### 3.2 `SyncBlobStore` — Arbeitsdateien

Die lokalen Sync-Arbeitsdateien (Pending-Logs unter `sync/log/pending/`, Sound-Assets unter `assets/sounds/`).

- `write(path, bytes)`, `read(path)`, `list(prefix) -> [(path, size)]`, `delete(path)`, `rename(from, to)`.

**Desktop:** `tokio::fs` über `resolve_data_dir()`.
**Mobile:** App-Sandbox-Pfade (Android `filesDir`, iOS Application Support).
**Hinweis:** Logs/Snapshots/`meta.json` auf der *Gegenstelle* gehören dem Sync-Adapter, nicht diesem Trait.

### 3.3 `SecretStore` — Keychain

- `store(account, slot, value)`, `retrieve(account, slot)`, `delete(account, slot)`, `delete_all(account)`.
- Die **Allowlist** (`secrets.rs`: nur `password`, `refresh_token`, `api_token` dürfen über Sync-Events reisen; `access_token` und `sync_encryption_key` nie) bleibt in dieser Schicht erhalten.

**Desktop:** `keyring`. **Mobile:** iOS-Keychain / Android-Keystore (über die UniFFI-Grenze nativ).

### 3.4 `Clock`

- `now() -> DateTime<Utc>`. Injizierbar wegen der Zeitstempel-Invarianten (siehe §4) und für Testbarkeit (Zeit einfrieren).

### 3.5 `SyncProgressReporter` — Status-/Fortschritts-Callbacks

Ersetzt die direkten `app.emit(...)`-Aufrufe.

- `on_status_changed(status, report?)`, `on_conflicts_detected(count)`, `on_sync_log_updated()`.

**Desktop:** `TauriReporter` → `app.emit("sync-status" | "sync-conflicts-changed" | "sync-log-changed")`.
**Mobile:** FFI-Callback an die JS-/RN-Schicht.

---

## 4. Zu wahrende Invarianten

1. **Session-Datei-Zeitstempel = `boot_at`** des Orchestrators (verhindert eine Windows-`FILE_SHARE_DELETE`-Race bei leeren Stub-Dateien). Über `Clock` konsistent halten.
2. **Sekunden-Granularität** bei der Stale-Stub-Erkennung (`orchestrator.rs`) — Nanosekunden werden abgeschnitten.
3. **Idempotenz** ausschließlich über `sync_applied_events` (kein Re-Apply bei Überlappung/Re-Fetch).
4. **Feldweise Konfliktauflösung:** ein Feld kollidiert nur, wenn `local_updated_at > envelope.timestamp`; sonst Auto-Merge (remote gewinnt). Kollisionen landen in `sync_conflicts`.
5. **E2E-Schicht sitzt ÜBER den Adaptern** (`EncryptingAdapter`): die Engine sieht nie Klartext-Secrets, wenn E2E an ist.
6. **Keychain-Allowlist** (siehe §3.3) bleibt unverändert.

---

## 5. Migrationsreihenfolge (Desktop-first, inkrementell, Tests grün)

1. **`sync-engine`-Crate anlegen** + die 5 Traits + die geteilten Typen (`SyncRoundReport`, `SyncStatus`, `ConflictKind`/`NewConflict`, …). Reine Definitionen, noch keine Verschiebung.
2. **Modulweise verschieben — kleinstes zuerst:** Writer → SnapshotBuilder → Compactor → Applier → Orchestrator. Pro Modul: Signaturen von `SharedConn`/`LocalAdapter`/`tokio::fs`/`keyring`/`Utc::now` auf die Traits umstellen, Desktop-Impl bereitstellen, **Suite nach jedem Modul grün**.
3. **src-tauri umstellen:** `SyncScheduler` + Commands treiben jetzt die Crate-Engine über die Trait-Impls (`DesktopSyncStore`, FS-`BlobStore`, `keyring`-`SecretStore`, echte `Clock`, `TauriReporter`).
4. **Abnahme:** komplette Desktop-Testsuite grün **und** CI-Parität (`cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`).
5. **(Folgeschritt, nicht Teil dieses Branches)** `cal-mobile` implementiert dieselben Traits (rusqlite/Sandbox/Keychain + JS-Callback-Reporter) + ein Mobile-Scheduler — die Engine läuft 1:1 am Handy.

---

## 6. Branching- & Integrationsstrategie (dritter Branch)

Der Refactor läuft auf einem **eigenen, dritten Branch**, getrennt von der Mobile-Arbeit:

- **Branch `refactor/sync-engine`**, abgezweigt von **`main`**. Inhalt: ausschließlich die verhaltenswahrende Engine-Extraktion (Desktop-only). Kein Mobile-Code.
- **Mergebar in `main`:** `main` bekommt die wiederverwendbare Engine; das Desktop-Verhalten ist unverändert → du kannst `main` **in Ruhe / im echten Betrieb auf Regressionen testen**, völlig getrennt von der (noch unfertigen) Mobile-Arbeit.
- **Mergebar/rebasebar in `feat/mobile-foundation`:** der Mobile-Branch zieht den Refactor-Stand ein und baut `cal-mobile` darauf auf.

**Empfohlene Reihenfolge:**
1. `refactor/sync-engine` umsetzen, grün (Tests + CI-Parität).
2. In **`main`** mergen → Desktop dort validieren (parallel, kein Mobile-Ballast).
3. `feat/mobile-foundation` auf den neuen `main`-Stand rebasen/mergen → die Mobile-Arbeit (cal-mobile, Provider-Maschine, UI) baut auf der bereits validierten Engine auf.

So bleibt die riskante, Desktop-berührende Umstrukturierung sauber isoliert und unabhängig prüfbar, und `feat/mobile-foundation` wird erst angefasst, wenn die Engine auf `main` steht.

---

## 7. Test- & Sicherheitsstrategie

- **Sicherheitsnetz:** die bestehenden Desktop-Sync-Tests (Applier-Merge, Orchestrator-Runde, Compactor, Snapshot, E2E). Erfolgskriterium = **kein Verhaltensunterschied**.
- **Trait-Conformance-Tests (optional, empfohlen):** ein Testset gegen die `SyncStore`/`SyncBlobStore`-Traits, das jede Implementierung erfüllen muss — jetzt die Desktop-Impl, später die Mobile-Impl. Fängt Drift früh.
- Pro Migrationsschritt ein eigener Commit; jeder Commit hält die Suite grün.

---

## 8. Außerhalb des Scopes (Folgeschritte, eigene Branches/Phasen)

- **Provider-Maschine** (cal-Adapter live auf Mobile — Vikunja/Todoist/… für das *treue* Desktop-Abbild). Zweite, separate Maschine; kommt **nach** der Engine.
- **Mobile-Plattform-Impls** (rusqlite cross-compiled, Sandbox-BlobStore, Keychain/Keystore) + **Mobile-Scheduler** (`BGTaskScheduler`/`WorkManager`).
- **Barrierefreie Aufgaben-UI** (tasks-first).
- **Build-Hygiene-TODOs** (`.easignore`, XCFramework via Git-LFS/Download) — unabhängig.

---

## 9. Offene Punkte / Risiken

- **Umfang:** der Applier (~1.900 Zeilen) und die breite `SyncStore`-Methodenliste machen den Refactor groß, aber **mechanisch**. Inkrementell + Tests grün entschärft das.
- **async vs sync an der Grenze:** DB-Operationen bleiben synchron (single-threaded `SharedConn`), Adapter + BlobStore sind async. Die Trait-Signaturen spiegeln das.
- **Typ-Umzug:** Konflikt-/Report-Typen (`ConflictKind`, `NewConflict`, `SyncRoundReport`, `SyncStatus`) müssen ggf. von `src-tauri` nach `sync-engine`/`sync-core` wandern; Einstiegspunkt für Schritt 1.
- **Finalisierung der `SyncStore`-Methoden:** die exakte Methodensignatur-Liste wird beim Verschieben des ersten Moduls final festgezurrt (das Scoping liefert die vollständige Ausgangsliste mit `file:line`).
