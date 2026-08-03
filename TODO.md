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
Schicht noch an die App-Sandbox. URSPRÜNGLICH geplanter Weg: der Rust-Kern wird
in die Extension mitgelinkt und liest die Datenbank direkt — dafür müsste sie aus
`applicationSupportDirectory` in einen App-Group-Container umziehen. Beides ist
in 2b/2c verworfen worden; was wirklich gebaut wurde, steht dort. Das
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
- [x] **Schritt 2a — ZURÜCKGENOMMEN.** Die Datenbank zog in den
      App-Group-Container und ist inzwischen wieder heraus
      (`modules/cal-ffi/ios/DatabaseLocation.swift`).
      Gegenstandslos wurde der Umzug schon durch 2b/2c: das Widget liest den
      Snapshot, nicht die Datenbank. Er war aber nicht bloß überflüssig, sondern
      tödlich — iOS beendet eine App mit `0xdead10cc`, wenn sie beim
      Suspendieren eine Dateisperre auf etwas im GETEILTEN Container hält, und
      genau das tut eine offene WAL-Verbindung, prozesslebenslang. Dieselbe
      Verbindung im eigenen Sandbox-Container wird gar nicht beobachtet.
      Im Absturzbericht steht kein Code von uns: der Haupt-Thread wartet
      untätig in seiner Run-Loop. `RUNNINGBOARD`-Code `3735883980` =
      `0xdead10cc` ist der ganze Befund.
      Der Rückweg trägt dieselbe Beweislast wie der Hinweg: kopieren, Größen
      vergleichen, testweise öffnen, DANN die Marker-Datei löschen (der
      Umschaltpunkt), zuletzt die Container-Kopie. Die Marker-Datei — und nicht
      die Existenz einer Datei — entscheidet weiterhin, welche Kopie lebt.
      ⚠️ Weiterhin einseitig: eine App-Version VOR dem Umzug sucht in
      Application Support, findet dort aber jetzt wieder alles.
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
      Widget 1 („Als Nächstes") liegt seit dem Gerätetest ebenfalls auf dem
      Sperrbildschirm: `.accessoryRectangular`, DREI EINZEILIGE Zeilen (Apples
      eigenes Beispiel für diese Familie sind „die drei obersten To-dos") — mit
      Abhak-Knopf (24pt statt 28pt). Interaktive Widgets laufen ab iOS 17 auch
      auf dem Sperrbildschirm; sie dort wegzulassen war eine Design-Entscheidung
      und keine Grenze, und sie war falsch: Apples eigene Erinnerungen können es.
      `containerBackground` hängt an der Familie und sitzt in der VIEW, nicht in
      der Configuration: ein Accessory wird im System-Material gezeichnet.
      „Nächster Termin" bleibt bewusst nur lesend — ein Countdown-Widget, das
      überwiegend Termine zeigt, braucht keinen Haken.
      ⚠️ Der Rest ungeprüft auf dem Gerät.
- [~] **Schritt 4** — Abhaken aus dem Widget (`targets/widget/Actions.swift`,
      `mobile/src/state/widgetActions.ts`). Die Extension schreibt NICHT die
      Antwort, sondern die FRAGE: eine Datei pro Tipp in die App Group, die die
      App über `setTaskStatusTo` abarbeitet — denselben Aufruf, den die
      Tagesstart-Übersicht nutzt. Abschließen kaskadiert auf Eltern und Kinder,
      weist auf geteilten Listen zu, schaltet Wiederholungen weiter und stellt
      einen Sync-Push ein; nichts davon ist aus einem Extension-Prozess
      erreichbar, und es hier nachzubauen wäre der Weg, auf dem Widget und App
      auseinanderlaufen.
      EINE Datei pro Aktion, nie eine gemeinsame: zwei Prozesse schreiben da
      hinein, und ein read-modify-write verliert den Tipp, der das Rennen
      verliert.
      Das Widget blendet bereits eingereihte Zeilen aus, sonst wirkt der Knopf
      tot. Eine Aktion wird nach dem VERSUCH gelöscht, nicht erst nach Erfolg —
      sonst bliebe eine unausführbare Aktion für immer stehen und mit ihr die
      unsichtbare Zeile.
      A11y: die Aufgabenzeile IST das Kontrollkästchen — `Toggle(isOn:intent:)`
      mit der ganzen Zeile als Label, nicht Text plus Knopf daneben. Ein Element
      statt zwei, und VoiceOver liefert Inhalt, Rolle und Zustand in einem Wisch.
      `Toggle(isOn:intent:)` nimmt einen NORMALEN `AppIntent` — `SetValueIntent`
      braucht nur `ControlWidgetToggle` (Kontrollzentrum). Der gewünschte
      Zielzustand steckt in den Intent-Parametern, nicht im Toggle.
      Eigener `ToggleStyle` (Kreis statt Schalter): die Interaktion hängt am
      `Toggle`, nicht am Style, also ändert ein Style, der nur zeichnet, nichts
      am Verhalten.
      `isOn` ist IMMER false — eine erledigte Aufgabe steht nicht im Snapshot,
      eine gerade abgehakte blendet das Overlay aus. Es gibt hier keinen Zustand,
      der mit der App auseinanderlaufen könnte.
      Der Tipp läuft über `applyTaskToggle`, NICHT über `setTaskStatusTo(…,
      'completed')`. Der Abhak-MODUS ist eine gesynct Einstellung: unter „cycle"
      geht ein Tipp offen → in Arbeit → erledigt. Hart auf erledigt zu schreiben
      hätte das Widget zur einzigen Oberfläche gemacht, die die Einstellung des
      Nutzers ignoriert. Die eingereihte Aktion heißt darum `toggle`, nicht
      `complete` — das Widget bittet um dasselbe wie ein Tipp in der App, und die
      App entscheidet, was das heißt.
      Der Zustand steht an der Zeile: leerer Kreis = offen, halb gefüllt = in
      Arbeit, plus das Wort in der Ansage. Eine schreibgeschützte Projektion
      bekommt das Wiederholungs-Symbol statt eines Kreises — ein Kreis, den man
      nicht anhaken kann, ist ein lügendes Bedienelement.
      Offen: der Intent-Titel („Check off task") ist unübersetzt, er taucht nur
      in der Kurzbefehle-App auf.
- [x] **Sprache der Widgets** — Gerätetest zeigte „in 17 hours" auf einem
      deutschen Telefon. URSACHE: `Locale.current` wird in einer Extension mit
      den Lokalisierungen ihres BUNDLES verschnitten, und ein Widget-Target ohne
      `.lproj`-Ordner deklariert keine — es fällt also auf die Entwicklersprache
      zurück, egal was das Telefon eingestellt hat.
      FIX: der Snapshot trägt Aperios Sprach-Tag; `localeFor` kombiniert die
      SPRACHE der App mit der REGION des Telefons und wird jedem Formatter
      explizit übergeben. Beides aus einem Tag zu ziehen würde die Wörter
      reparieren und die Zahlen kaputtmachen — ein Deutscher in den USA will
      12-Stunden-Zeiten.
      Betraf nicht nur den Countdown, sondern auch Wochentage und Monate im
      Listen-Widget.
- [x] **Sortierung: laufende Ganztagestermine sind nicht „als Nächstes"** —
      Gerätetest: auf dem Sperrbildschirm standen NUR ganztägige Termine, keine
      Aufgaben. Ursache: sortiert wurde nach `at`, und ein 42-Tage-Urlaub hat
      seinen Start Wochen in der Vergangenheit — also ganz vorn, sechs Wochen
      lang, und bei drei Zeilen verdrängt das alles Echte.
      FIX: unterminierte Einträge sortieren nach ihrem ENDE. Ein Ganztagestermin
      landet dort, wo er aufhört zu gelten: ein einzelner Tag bleibt bei den
      Terminen dieses Tages, ein sechswöchiger Urlaub rutscht sechs Wochen
      nach hinten. Dieselbe Lesart stellt eine unterminierte Aufgabe HINTER die
      Termine des Tages — ein Termin besitzt eine Stunde, die Aufgabe den Tag.
      Terminierte Einträge sortieren weiterhin nach Start, auch laufende: ein
      Termin, in dem man gerade sitzt, ist das Unmittelbarste, was es gibt.
      ⚠️ Ungeprüft auf dem Gerät.

### A8 · Sprachbefehle (Siri / Kurzbefehle, iOS zuerst) `[~]`
Termine und Aufgaben per Sprache anlegen, mit Kalender- bzw. Listenwahl.

WARUM NUR iOS: Google bietet **keinen** Built-in Intent für Kalendertermine
(Referenz geprüft — Produktivität kennt nur Listen). Der Assistant schreibt in
Google Kalender, eine Drittanbieter-App kann sich dafür nicht anmelden. Unter
iOS löst Siri getippte `Date`-Parameter dagegen selbst auf — kein NL-Parser
nötig. Für Deutsch gäbe es auch keinen: die Rust-Crates sind Englisch-only,
`chrono-node` führt Deutsch nur als TEILWEISE unterstützt.

GRENZE, die die Form bestimmt: ein `Date` darf NICHT in der Kurzbefehl-Phrase
stehen. „Termin morgen um 11 in Aperio" in einem Satz geht nicht; Siri fragt die
Parameter nach. Für einen Screenreader-Nutzer ist der geführte Dialog eher ein
Vorteil.

- [~] **Schritt 1** — beweisen, dass App-Target-Swift Siri überhaupt erreicht:
      `plugins/withAppShortcuts.js` kopiert `mobile/ios-app/AperioShortcuts.swift`
      ins generierte App-Target und trägt es ins Xcode-Projekt ein. Inhalt: EIN
      Kurzbefehl, der nur die App öffnet.
      Der `AppShortcutsProvider` MUSS im Haupt-App-Target liegen — Apples
      Framework-Ausweg (`AppIntentsPackage`) gilt nur für Frameworks, nicht für
      die statischen Bibliotheken, zu denen Expo-Module übersetzen. Ein Pod
      scheidet damit aus.
      ✅ Gerätetest: Kurzbefehl erscheint, „Hey Siri, öffne Aperio" startet die
      App. Der Weg trägt.
- [~] **Schritt 2** — `CreateEventIntent` mit Titel + `Date`. Siri löst die
      gesprochene Zeit selbst auf; wir reihen die Anfrage in dieselbe
      Aktions-Warteschlange wie der Widget-Haken und die App legt sie beim
      Hereinkommen an.
      `openAppWhenRun = true`, und das ist der Unterschied zum Widget: ein Haken,
      der eine Zeile verschwinden lässt, darf Minuten später wirken; ein
      gesprochenes „neuer Termin" nicht — man sagt es, schaut nach und findet
      nichts.
      Dauer fest 60 Minuten (Siri gibt einen Moment, keine Spanne), Kalender =
      zuletzt genutzter, sonst der erste beschreibbare.
      `state/widgetActions.ts` → `queuedActions.ts` umbenannt: es bedient jetzt
      zwei Absender, der alte Name hätte gelogen.
      ⚠️ Ungeprüft.
- [ ] **Schritt 2b** — deutsche Phrasen. Kurzbefehl-Phrasen lokalisieren über
      eine `AppShortcuts.strings` im App-Target, NICHT über unseren i18n-Katalog.
      Bewusst von Schritt 2 getrennt gehalten: ein eigener Mechanismus, dessen
      Fehlschlag sonst die Diagnose des Anlegens vernebelt hätte.
- [~] **Apple Intelligence (Assistant Schemas)** — der Weg, der Siri FREIE Rede
      erlaubt statt einer festen Phrase: `@AssistantIntent(schema:
      .calendar.createEvent)`. Es gibt eine Kalender-Domäne, `createEvent` ist
      Teil davon. Damit wäre „erstelle einen Termin am Montag von 11 bis 13 mit
      dem Titel Essen kochen" erreichbar — das, was Apples eigener Kalender kann.
      BLOCKER, warum es noch nicht gebaut ist: das Makro prüft die Form beim
      ÜBERSETZEN. Falsche Property-Namen sind ein Build-Fehler, und Apples Doku
      gibt die Form online nicht her (Xcode hat dafür ein Code-Snippet, das uns
      unter Windows nichts nützt). Blind raten kostet je Versuch einen vollen
      EAS-Bau, für ein Feature, das ohne Apple-Intelligence-Gerät nicht einmal
      beobachtbar ist.
      LÖSUNG: `.github/workflows/probe-app-intents-schema.yml` — fragt den
      iOS-SDK auf dem macOS-Runner nach den Deklarationen und lässt zusätzlich
      das Makro an einer ABSICHTLICH leeren Konformanz scheitern, damit seine
      Diagnose die verlangten Felder aufzählt. Zwei Minuten Runner-Zeit statt
      einer Rateschleife. Ergebnis hier eintragen, dann ist die Umsetzung
      Fleißarbeit.
      Zu erwarten ist außerdem ein ganzer Entitäten-Graph, nicht nur ein Makro:
      `perform()` muss das angelegte Objekt als `@AssistantEntity(schema:
      .calendar.event)` zurückgeben, was vermutlich eine Kalender-Entität nach
      sich zieht.
      Tonis Gerät hat noch KEIN Apple Intelligence — der Dialog aus Schritt 2
      bleibt also der Weg, der bei ihm wirkt; das hier ist für neuere Geräte.
- [ ] **Schritt 3** — Kalender und Aufgabenliste als `AppEntity` mit
      `EntityQuery`. Die Liste kommt über dieselbe Snapshot-Datei-Mechanik wie
      beim Widget (eine `calendars.json` in der App Group) — der Intent-Prozess
      kommt so wenig an die Datenbank wie die Widget-Extension.
- [ ] **Schritt 4** — tatsächlich anlegen. Der Intent reiht die Aktion ein wie
      der Widget-Haken; OFFEN ist, ob die App sich dabei öffnet
      (`openAppWhenRun`) oder still im Hintergrund abgearbeitet wird. Beim Haken
      ist Verzögerung unsichtbar, beim ANLEGEN nicht — man sagt etwas und findet
      minutenlang nichts.
      Den Rust-Kern direkt aus dem Intent zu öffnen ist bewusst KEINE Option:
      zweiter schreibender Prozess auf einer Datenbank, plus ein teurer
      Host-Start pro Sprachbefehl.

- [~] **Android** — „Als Nächstes" über **Glance**
      (`modules/cal-ffi/android/.../AperioWidget.kt`). Liest denselben Snapshot;
      kein App Group nötig, ein Android-Widget läuft im Prozess der App unter
      derselben uid, also reicht `filesDir/widget/`.
      GLANCE statt RemoteViews wegen EINER Sache: `CheckBox` trägt Rolle UND
      Zustand zu TalkBack. RemoteViews kann einen Kreis zeichnen und eine
      Beschreibung setzen, aber nicht sagen „das ist ein Kontrollkästchen, und
      es ist nicht angehakt".
      Der Compose-Compiler war KEINE neue Abhängigkeit: `expo-modules-core` legt
      `org.jetbrains.kotlin.plugin.compose` bereits in den buildscript-Klassenpfad
      und `expo-dev-launcher` zieht Compose ohnehin in den Baum. Angewendet im
      selben Muster wie expo-modules-core (`apply plugin:` nach einem
      buildscript-Block, nicht die `plugins {}`-DSL).
      Der Receiver steht im MODUL-Manifest, das der Build ins App-Manifest
      mergt — kein Config-Plugin nötig. `exported="true"`, sonst bindet der
      Launcher ihn nie.
      Sperrbildschirm-Widgets gibt es unter Android nicht (nach Android 11
      entfernt), das Countdown-Widget hat also kein Gegenstück.
      ⚠️ Ungeprüft — Toni hat kein Android-Gerät; ein Bau prüft nur, dass es
      übersetzt.
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
