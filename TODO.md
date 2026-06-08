# Aperio — Offene Baustellen / Backlog

> Abgeleitet aus einem systematischen Abgleich von [`DESIGN.md`](DESIGN.md)
> (Spezifikation) gegen den tatsächlichen Code + Commit-Historie
> (Stand: 2026-06-08). Jede Lücke wurde **adversarial gegengeprüft** — ein
> zweiter Durchgang versuchte, sie durch Auffinden der Implementierung zu
> widerlegen. Mehrere zunächst vermutete Lücken wurden dabei als *doch
> umgesetzt* verworfen (siehe „Bestätigt umgesetzt" am Ende).
>
> Abschnittsnummern (§) verweisen auf `DESIGN.md`.
> Status: `[ ]` offen · `[~]` teilweise · `[x]` erledigt.
> Diese Datei ist die laufende, code-abgeglichene Ergänzung zu DESIGN.md §25.

---

## 🔴 A. Große fehlende Subsysteme

### A1 · Self-Update-System (§21, §22.1) — komplett nicht vorhanden
- [ ] `tauri-plugin-updater` als Dependency + `updater`-Feature in `src-tauri/Cargo.toml`
- [ ] Update-Check beim App-Start via GitHub Releases API (§21.1)
- [ ] Bestätigungsdialog „Version X verfügbar" — Jetzt installieren / Später / Überspringen (§21.1)
- [ ] Portable-Update-Flow: Binary + `plugins/bundled/` ersetzen, `data/` + `plugins/user/` erhalten, Neustart (§21.1)
- [ ] Update-Manifest generieren + bereitstellen (version / notes / pub_date / platforms+signature) (§21.2)
- [ ] Code-Signing in `release.yml`: macOS ad-hoc (`codesign --force --deep -s -`), Linux optional GPG (§21.3, §22.1)
- Einstieg: `src-tauri/Cargo.toml`, `.github/workflows/release.yml`, neues `src-tauri/src/commands/update.rs`.

### A2 · System-Integration: Datei-/URL-Verknüpfungen (§17) — nahezu vollständig offen
- [ ] `.ics`-Dateiverknüpfung + Import-Dialog (Vorschau Titel/Datum/Beschreibung, Kalenderwahl, Batch- oder Einzelauswahl) (§17.1)
- [ ] `.aperio`-Verknüpfung → startet automatisch die Plugin-Installation (§17.1, §20.7)
- [ ] `webcal://` + `calendar://` URL-Handler (Feed-Abo vs. Einzeltermin unterscheiden) (§17.2)
- [ ] Plattform-Registrierung per-User ohne Admin: Windows-Registry (HKCU), macOS `CFBundleDocumentTypes`, Linux `.desktop` MimeType (§17.1/§17.2)
- [ ] Erst-Start-Assistent „Systemintegration einrichten" (Checkboxen .ics / webcal / .aperio + optional Desktop-Verknüpfung; tastatur- + screenreader-bedienbar) (§17.3)
- [ ] CLI-Argument-/Deep-Link-Handling für „mit Datei/URL geöffnet"
- Einstieg: `src-tauri/tauri.conf.json` (fileAssociations / deep-link), `src-tauri/src/lib.rs` (argv + deep-link), neuer Import-Dialog im Frontend.

### A3 · Tastaturkürzel-Anpassung + Overlay (§15.8, §15.9, §15.10)
- [ ] `ShortcutOverride` / `KeyCombo` DB-Schema + CRUD-Commands
- [ ] Rebind-Dialog mit Capture, Konflikterkennung, „Alles zurücksetzen" (§15.10)
- [ ] Kürzel-Overlay (Ctrl+H / Ctrl+/): durchsuchbar, gruppiert, zeigt aktuelle Belegung, „Anpassen"-Button (§15.8)
- [ ] Plattform-Modifier-Substitution Ctrl ↔ Cmd bei Cross-Device-Sync (§15.9)
- [ ] `shortcut.set` / `shortcut.reset` / `shortcut.cleared`-Events tatsächlich **emittieren** (Applier-Handler existieren bereits als Forward-Compat-No-op) (§19.2)
- [ ] Fehlende Einzelkürzel implementieren: **Ctrl+R** (Sync), **Ctrl+E** (Fokussiertes bearbeiten), **Ctrl+H**, **Ctrl+Q** (§15.7)
- Einstieg: `src/state/useDialogShortcuts.ts`, `src-tauri/src/event_log/applier.rs` (shortcut.*), neue Settings-Sektion „Tastaturkürzel".

### A4 · Videokonferenz-Adapter (§11) — alle vier sind Stubs
- [ ] Echte REST-/OAuth-Implementierung für Zoom / Teams / Meet / WebEx statt `VcError::Unsupported` (§11.1)
- [ ] `vc_meeting_id`-Feld am Event-Model; `create_meeting` erzeugt + speichert den Meeting-Link (§11.2)
- [ ] Frontend: „Meeting erstellen" + „Direkt beitreten"-Button (§11.2)
- [ ] (optional) Raumverwaltung als zusätzliche Capability (§11.2)
- Einstieg: `crates/vc-adapter-{meet,teams,webex,zoom}/src/lib.rs`, `crates/vc-core`, `EventDialog.tsx`.

### A5 · Benachrichtigungs-Aktionen (§14.3)
- [ ] Action-Buttons in Toasts: **Öffnen** / **Snooze** (konfigurierbare Dauer) / **Erledigt** (nur Aufgaben)
- [ ] Handler: snooze (neu planen), mark-done, open-from-notification
- Einstieg: `src-tauri/src/reminders.rs` (`fire()`), Notification-Builder um `.action()` erweitern.

### A6 · Offline-Queue für externe APIs (§18.2)
- [ ] SQLite-Queue, die Mutationen an externe Kalender/Aufgaben (create/update/delete event+task) offline puffert
- [ ] Retry bei Reconnect inkl. ETag-Prüfung
- Hinweis: Der **lokale** Sync-Log existiert; gemeint ist die Pufferung von Schreibzugriffen auf **externe** Provider.
- Einstieg: neue Migration + die Mutationspfade in `src-tauri/src/commands/`.

---

## 🟠 B. Teilweise umgesetzt / kleinere Lücken

### B1 · Woche-Start konfigurierbar (§5.2) ✅ erledigt
`view.weekStart` lebt jetzt als synchronisierte Pref im ViewState-Context; Wochen-,
Monats- und Jahresansicht (Spalten + Home/End-Navigation) richten sich danach;
KW-Nummern bleiben ISO 8601. Auswahl in **Einstellungen → Allgemein → Ansichten**.
- [x] UI-Auswahl (lokalisierte Wochentage) + `view.weekStart` in `WeekView` / `MonthView` / `YearView` / `viewMath.ts` gelesen und angewandt.

### B2 · Serien-Verschieben/Kopieren-Scope (§7.5) ✅ erledigt
Bei wiederkehrenden Vorkommen bietet der `MoveCopyDialog` jetzt „Nur diesen Termin /
Gesamte Serie". Einzel-Vorkommen → eigenständiger Termin am Ziel (ohne Serie); beim
Verschieben zusätzlich EXDATE auf die Quell-Serie (Create-then-exclude, kein Datenverlust).
- [x] „Nur dieser Termin / Gesamte Serie"-Auswahl im `MoveCopyDialog`; Logik in `moveOrCopyEvent` (`moveActions.ts`) mit Tests für alle vier Kombinationen.

### B3 · Sync-Restpunkte (§19) `[~]`
- [ ] Per-Einstellung-Sync-Umschalter-UI im `SyncPanel` (§19.2.1; Backend hat eine feste `SYNC_WHITELIST`)
- [ ] Schema-Migration-Nachlauf: Migration erkennen → Snapshot erzwingen → `meta.json.schema_version` / `min_app_version` aktualisieren (§19.13; SQLite-Migration selbst läuft bereits)
- [ ] `plugin.updated`-Event emittieren (definiert, wird nie gesendet) (§19.2)

### B4 · Mini-Kalender-Sidebar-Widget (§5.3) `[ ]`
- [ ] Optionales, ein-/ausblendbares Datums-Widget für schnelle Navigation in der Sidebar.

### B5 · Fenster-Status-Persistenz (§15.3) ✅ erledigt
Fenstergröße + -position werden beim Schließen in `app_config.json` (im aufgelösten
Data-Dir via `resolve_data_dir()`, **gerätlokal — nicht** synchronisiert) gespeichert
und beim Start wiederhergestellt — inkl. Maximiert-Status und Schutz gegen
Off-Screen-Positionen (z. B. getrennter Zweitmonitor).
- [x] `window_state.rs` (Store + load/save/remember/flush/restore, mit Tests) in `lib.rs` verdrahtet: Move/Resize → merken, Close → schreiben, Setup → wiederherstellen.

### B6 · Anhang-Suche (§13.1) `[ ]` — nur falls Anhänge überhaupt Feature werden
- [ ] `attachments`-Feld am Event-Model + `attachments`-Spalte in `events_fts` + Trigger.

### B7 · Erinnerungs-Feinheiten (§14) `[~]`
- [ ] E-Mail-Reminder: UI-Option + (Adapter-)Versand — lokaler Scheduler überspringt sie derzeit bewusst.
- [ ] Per-Vorkommen-Sound-Override, ohne das Vorkommen aus der Serie herauslösen zu müssen.

---

## 🟡 C. Bewusste Deferrals (dokumentiert, niedrigere Priorität)

### C1 · Task-Recurrence in EWS & Todoist (§9.1)
- [ ] EWS: Recurrence lesen/schreiben (aktuell beim Schreiben verworfen, „Phase 6f.2-Follow-up")
- [ ] Todoist: `due_string` ↔ `TaskRecurrence` (aktuell out of scope)

### C2 · Task-Detailpunkte (§9 — geringere Konfidenz, in Agent-Notizen erwähnt)
- [ ] Recurrence-Template nach Abschluss generieren (für alle Adapter out of scope)
- [ ] TaskView-Filter-UI
- [ ] Move/Copy-Prompts (Subtasks mitnehmen / Recurrence-Instanz vs. Regel / Reminder-Kompatibilität)
- [ ] `role="group"` + `aria-label` an Subtask-Eltern (a11y)

---

## ⚪ D. Geplant / Optional (DESIGN.md §25)

- [ ] `.ics`-**Export** (§25, §17)
- [ ] Druckfreundliche Kalenderansichten (§25)
- [ ] Visual Design / Farbpalette / Theming / Icon-Set — wird vom Auftraggeber nachgeliefert (§25)
- [ ] Mobile Companion App (Szenario A) — strategisch verschoben (§25.1)
- [ ] Thunderbird-Integration — optional, via CalDAV möglich (§25)

---

## ✅ Bestätigt umgesetzt (keine Lücke — vom Audit adversarial verifiziert)

Damit klar ist, was *nicht* offen ist und nicht erneut untersucht werden muss:
Views day/week/month/year/agenda + Task-View · Event-Formular, Teilnehmer,
Free/Busy, RSVP, Quick-Add · Farb-Labels inkl. Per-Event-Color-Capability-Gate ·
Aufgaben-Datenmodell, Backlog, verpasste-Aufgaben-Review, Subtasks, Sektionen ·
Kontakte/CardDAV inkl. Geburtstagskalender · Feiertage (iCal-Abo) · Volltextsuche
(FTS5) + Filter · Adapter-Matrix (CalDAV, iCal, Google, Microsoft Graph, EWS,
Vikunja, Todoist, lokal) · Sync-Kern: Event-Log, Snapshot, Kompaktierung, E2E
(+ Credential-Sync), Konfliktauflösung, Sync-Trigger, Statusanzeige, Stale-Device-
Recovery, Onboarding, feldweises Merge · Plugin-System (ABI, Manager, SDK,
bundled/community, enable/disable/uninstall) · Build/Release (CI, Portable-Binary,
17 Plugins, universal macOS) · Doku (vier mdBooks).

---

*Erzeugt aus einem Multi-Agent-Audit (DESIGN.md ↔ Code ↔ Commits). Beim Abhaken
bitte den jeweiligen DESIGN.md-Abschnitt mitpflegen, falls sich die Spezifikation
ändert.*
