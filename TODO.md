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

### A4 · Videokonferenz-Adapter (§11) — WebEx steht, der Rest ist offen
Die Rümpfe für Zoom / Teams / Meet wurden entfernt, statt sie als „geplant" zu
führen: drei leere Crates sind keine Roadmap. WebEx ist echt implementiert
(OAuth, Meetings anlegen/beitreten), die Join-Details stecken in
`cal-core::conferencing` und beide Frontends haben ihre `MeetingControls`.
- [x] Echte REST-/OAuth-Implementierung für WebEx statt `VcError::Unsupported` (§11.1)
- [x] Meeting-Link am Event; `create_meeting` erzeugt + speichert ihn (§11.2)
- [x] Frontend: „Meeting erstellen" + „Direkt beitreten"-Button (§11.2)
- [ ] Zoom / Teams / Meet: je ein Adapter, wenn sie gebraucht werden (§11.1)
- [ ] (optional) Raumverwaltung als zusätzliche Capability (§11.2)
- Einstieg: `crates/adapter-webex/src/lib.rs`, `crates/vc-core`, `EventDialog.tsx`.

### A5 · Benachrichtigungs-Aktionen (§14.3)
- [ ] Action-Buttons in Toasts: **Öffnen** / **Snooze** (konfigurierbare Dauer) / **Erledigt** (nur Aufgaben)
- [ ] Handler: snooze (neu planen), mark-done, open-from-notification
- Einstieg: `src-tauri/src/reminders.rs` (`fire()`), Notification-Builder um `.action()` erweitern.

### A7 · Mobile Widgets (iOS zuerst) `[~]`
Termine und Aufgaben auf dem Home- und Sperrbildschirm, mit Abhaken. Drei
Widgets statt eines mit Umschalter — ein Widget mit Modi ist per Screenreader
schlechter zu erfassen als drei mit je einem Zweck:

1. **Als Nächstes** — kommende Termine und fällige Aufgaben gemischt, nur Anzeige.
2. **Heute** — die heutigen Aufgaben, mit Abhak-Schalter.
3. **Nächster Termin** — eine Zeile plus Countdown (Sperrbildschirm).

Ein Widget läuft in einem eigenen Prozess und kommt weder an die React-Native-
Schicht noch an die App-Sandbox. Gewählter Weg: der Rust-Kern wird in die
Extension mitgelinkt und liest die Datenbank direkt — dafür muss sie aus
`applicationSupportDirectory` in einen App-Group-Container umziehen. Das
`CalFfi.xcframework` liegt bereits versioniert im Repo, es muss nichts Neues
gebaut werden.

Reihenfolge nach RISIKO, nicht nach Interesse: jeder Schritt kostet einen
EAS-Durchlauf und ist blind, also kommen die Fragen zuerst, deren Antwort alles
Übrige trägt.

- [x] **Schritt 0** — App-Group-Entitlement allein (`plugins/withAppGroup.js`),
      ohne Widget und ohne Datenbank-Umzug. Beantwortet: signiert die
      Capability überhaupt gegen unser Profil? Genau daran scheiterte Bau #5
      mit `aps-environment` (siehe `withoutPushEntitlement.js`).
- [~] **Schritt 1** — Widget-Target über `@bacons/apple-targets`
      (`mobile/targets/widget/`), feste Zeile, keine Daten. Beweist, dass das
      Target angelegt, signiert und installiert wird.
      **Eine Extension ist eine ZWEITE App-ID** (`com.aperio.mobile.widget`)
      mit eigenem Profil und eigener App-Groups-Berechtigung. EAS kann sie
      nicht unbeaufsichtigt anlegen — einmal `eas credentials` interaktiv,
      sonst: „Credentials are not set up".
      Ohne `ios/`-Verzeichnis liest eas-cli die Targets NICHT per Prebuild,
      sondern aus `extra.eas.build.experimental.ios.appExtensions` — fehlt der
      Eintrag, kennt es nur die App. Eine Target-Auswahl gibt es dabei nicht:
      „All: Set up all the required credentials" läuft über ALLE Targets, die
      es kennt. Erkennbar an zwei „Setting up credentials for target …"-Blöcken.
- [~] **Schritt 2a** — Datenbank-Umzug in den App-Group-Container
      (`modules/cal-ffi/ios/SharedDatabase.swift`): kopieren, testweise öffnen,
      erst dann die Originale löschen. Scheitert irgendetwas, bleibt alles am
      alten Platz und die App läuft weiter. Eigener Bau, weil er als einziger
      Schritt ECHTE Nutzerdaten anfasst — ein zweiter gleichzeitiger Umbau
      würde die Fehlersuche vernebeln.
      ⚠️ Einseitig: eine ältere App-Version sucht wieder in Application Support
      und findet nichts. Sieht aus wie Datenverlust, ist keiner.
- [~] **Schritt 2b/2c** — Widget 1 mit echten Daten, über eine SNAPSHOT-Datei
      statt über den mitgelinkten Rust-Kern. Beim Ausarbeiten von 2b fiel die
      Annahme, auf der der Linking-Plan stand:

      Die Datenbank ändert sich NUR, wenn die App läuft oder ihr
      Hintergrund-Sync läuft — ein anderer Schreiber existiert nicht. Ein Widget,
      das die Datenbank selbst liest, sähe also exakt dieselben Bytes wie eine
      Datei, die die App beim Hinausgehen schreibt. Der Linking-Weg kaufte
      dieselbe Aktualität für 21 MB Bibliothek ein ZWEITES Mal im Bundle, plus
      SQLite-Migrationen in einem Prozess, den iOS jederzeit abschießt.

      Gebaut ist deshalb: `shared/widgetSnapshot.ts` leitet ab (getestet),
      `mobile/src/state/widgetSnapshot.ts` sammelt an denselben Auslösern wie
      Erinnerungen und App-Badge, `WidgetSnapshotStore.swift` legt die Datei
      atomar in die App Group und stößt WidgetKit an, `targets/widget/` decodiert
      nur noch.

      Die Texte reisen MIT dem Snapshot: die Sprache ist die in der App
      gewählte, und die kann eine Extension nicht lesen.

      ⚠️ Noch ungeprüft auf dem Gerät. Offen bleibt außerdem: die Galerie-Namen
      („Als Nächstes" / „Up Next") können nicht aus dem Snapshot kommen — sie
      werden gelesen, bevor Daten existieren — und hängen deshalb an
      `Locale.preferredLanguages` statt an der App-Sprache.
- [~] **Schritt 3** — Widget 3 „Nächster Termin" (`targets/widget/NextUp.swift`):
      eine Zeile plus Countdown, Familien `.accessoryRectangular` und
      `.accessoryInline` (Sperrbildschirm). `.accessoryCircular` bewusst NICHT —
      es fasst einen Glyph oder eine Zahl, und beides kann nicht sagen, WAS
      ansteht.
      Der sichtbare Countdown tickt, die Sprachausgabe nicht: die Zeile ist EIN
      Element mit festem Label, damit VoiceOver nicht im Sekundentakt
      dazwischenredet.
      Angefangene Termine kippen von „in 25 Minuten" auf „Läuft bis 11:00" — ein
      Countdown allein würde negativ und damit unsinnig, genau in dem Moment, in
      dem die Zeile am meisten zählt.
      `RelativeDateTimeFormatter` formatiert in der GERÄTE-Sprache, nicht in der
      App-Sprache. Bewusste Ausnahme auf derselben Grundlage wie Uhrzeiten: es
      ist Zeitformatierung mit Pluralregeln für jede Sprache, die iOS mitbringt,
      und selbstgebaut wäre es in beiden schlechter.
      NUR terminierte Einträge — ganztägige sind gefiltert. Gerätetest 2026-08-03:
      ein 42-Tage-Urlaub als ganztägiger Termin besetzte das Widget für 42 Tage.
      Ein Countdown auf etwas Ganztägiges hat keinen Moment, auf den er zählt.
      Der leere Zustand ist deshalb ein eigener Satz („Nichts mit Uhrzeit.") und
      nicht „Nichts geplant." — letzteres wäre schlicht falsch, während jemand
      im Urlaub ist.
      ⚠️ Der Rest ungeprüft auf dem Gerät.
- [ ] **Schritt 3** — Countdown. `Text(timerInterval:)` rendert das System
      selbst, ohne Zeitachsen-Neuladen. Eigenes, gröberes `accessibilityLabel`
      („in etwa 20 Minuten"), sonst redet VoiceOver sekundenweise.
- [ ] **Schritt 4** — Abhaken per `AppIntent`. Der einzige Teil, der aus der
      Extension SCHREIBT. Vorher zu klären, ob Ereignis-Log und Sync-Warteschlange
      einen zweiten schreibenden Prozess vertragen — WAL kann Mehrprozess, das
      sagt nichts über die Schicht darüber.
- [ ] **Android** — dieselben drei Widgets über Glance. Zurückgestellt, nicht
      verworfen: es gibt kein Testgerät.
- Offen: Live Activities brauchen einen Start aus dem Vordergrund oder per Push;
  Aperio hat keinen Server und entfernt das Push-Entitlement bewusst. Ein
  Countdown „ohne Zutun" ist unter iOS damit nicht erreichbar, unter Android
  über eine dauerhafte Benachrichtigung aus dem Hintergrund-Worker schon.
- Einstieg: `mobile/plugins/`, `mobile/modules/cal-ffi/ios/CalFfiModule.swift`
  (Datenbankpfad), `crates/cal-ffi/src/host.rs` (Lesepfad).

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

### B3 · Sync-Restpunkte (§19) `[~]` (Toggle-UI bewusst zurückgestellt)
- [ ] Per-Einstellung-Sync-Umschalter-UI im `SyncPanel` (§19.2.1; Backend hat eine feste `SYNC_WHITELIST`) — **bewusst zurückgestellt** (auf Wunsch).
- [x] Schema-Versions-Nachlauf (§19.13): Der Compactor hebt `meta.json.schema_version` + `min_app_version` an, sobald diese App ein neueres **Sync-Wire-Format** schreibt (der frisch erzeugte Snapshot *ist* das migrierte Artefakt). **Klarstellung:** Der Audit hatte lokale SQLite-Migrationen (`db::CURRENT_SCHEMA_VERSION = 26`) mit dem Sync-Format (`sync_core::SCHEMA_VERSION = 1`) verwechselt — getrennte Dinge; die Versions-*Prüfung* (`ensure_compatible` beim Sync-Start) war bereits implementiert.
- [x] `plugin.updated`-Event wird beim Upgrade emittiert (vorher immer `plugin.installed`; der `is_upgrade`-Flag existierte bereits).

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

### C3 · All-Day-Datumsbehandlung in Google / Graph / EWS ✅ erledigt
Alle drei Adapter hatten den UTC-Kalendertag-Bug des CalDAV-Adapters; gefixt auf
die Referenz-Konvention (intern = `[lokale Mitternacht Start, lokale Mitternacht
Tag-nach-Ende)`), mit TZ-agnostischen Tests:
- [x] Google: Schreiben über lokalen Tag (`with_timezone(&Local).date_naive()`), Lesen verankert `date` auf lokale Mitternacht; Read→Write-Round-Trip-Test.
- [x] Microsoft Graph: Schreiben formatiert den lokalen Tag; Lesen nimmt den **Datums-Teil des Wire-Strings** (tz-unabhängig) und verankert lokal; Round-Trip-Test.
- [x] EWS: Schreiben pinnt All-Day-Grenzen auf UTC-Mitternacht des lokalen Tages (Create + Update); Lesen rekonstruiert den gemeinten Tag per **+12 h-Sampling** (robust für jede Zonen-Offset-Quelle in (−12 h, +12 h]) und verankert lokal; Schreib- + Round-Trip-Tests.

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
