# Aperio — Offene Baustellen / Backlog

> Abgeleitet aus einem systematischen Abgleich von [`DESIGN.md`](DESIGN.md)
> (Spezifikation) gegen den tatsächlichen Code + Commit-Historie
> (Stand: 2026-06-08). Jede Lücke wurde **adversarial gegengeprüft** — ein
> zweiter Durchgang versuchte, sie durch Auffinden der Implementierung zu
> widerlegen. Mehrere zunächst vermutete Lücken wurden dabei als *doch
> umgesetzt* verworfen (siehe „Bestätigt umgesetzt" am Ende).
>
> Abschnittsnummern (§) verweisen auf `DESIGN.md`.
> Status: `[ ]` offen · `[~]` teilweise · `[x]` erledigt · `[-]` hinfällig.
> Diese Datei ist die laufende, code-abgeglichene Ergänzung zu DESIGN.md §25.
>
> **Nachgeführt am 2026-09-07** — nach 1199 Commits war die Liste erheblich
> veraltet. Derselbe Abgleich noch einmal, gegen den heutigen Code: von 49
> offenen bzw. teilweisen Punkten sind **16 längst erledigt**, 7 weiter
> teilweise (jetzt mit dem konkreten Rest), einer **hinfällig**, 25 wirklich
> offen. Wo ein Punkt kippte, steht unter ihm eine `↳`-Zeile mit dem Befund —
> vor allem, wie es gelöst wurde, damit der nächste Leser nicht dieselbe Suche
> wiederholt. Punkte, die weiter offen sind, wurden bewusst nicht kommentiert:
> ihre Beschreibung stimmt ja noch.

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
- [~] `.aperio`-Verknüpfung → startet automatisch die Plugin-Installation (§17.1, §20.7)  
  ↳ Der Installer (inspect → bestätigen → entpacken) ist FERTIG und über Einstellungen ▸ Plugins erreichbar. Offen ist nur die OS-Verknüpfung plus das Start-Argument in diesen bestehenden Flow zu geben.
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

### A7a · Wie oft die Daten hinter den Widgets frisch werden `[x]`
Gerätebefund: das Widget hing tagelang hinterher, geschätzt ein bis zwei
Aktualisierungen am Tag. ZWEI unabhängige Ursachen, beide behoben.

**Erstens holte die Hintergrundrunde die Kalender überhaupt nicht.** Sie rief
`syncNow` — das ist die GERÄT-ZU-GERÄT-Maschine, die die Änderungen eines Peers
über WebDAV trägt und von iCloud oder Google nichts weiß. `refreshExternalCache`
hing ausschließlich an manuellen Knöpfen. Eine Hintergrundrunde hat also nie
etwas von den Konten geholt und danach einen Widget-Snapshot aus einem
unberührten Cache geschrieben — sieht richtig aus, ist immer einen Warm-Durchlauf
alt. Jetzt: Warm-Durchlauf anstoßen, Peer-Sync währenddessen, dann höchstens
15 Sekunden auf den Durchlauf warten, dann erst Erinnerungen und Snapshot. Ein
BUDGET, kein Abwarten bis zum Ende: die kurze Task-Klasse hat rund 30 Sekunden
insgesamt, und ein abgelaufener Task zählt bei iOS als Fehlschlag und kostet
künftige Zeit. Was liegt, wird geschrieben; der Rest kommt in der nächsten Runde,
weil der Warm-Durchlauf je Container speichert und nicht erst am Ende.

**Zweitens war es die falsche Task-Klasse.** `expo-background-task` reicht einen
`BGProcessingTaskRequest` ein — im Quelltext nachgelesen, `BackgroundTaskScheduler.swift`,
Kennung `com.expo.modules.backgroundtask.processing`, Info.plist nur
`UIBackgroundModes: ["processing"]`. Das ist Apples Klasse für lange
Wartungsarbeit („can take minutes to complete"), die das System bei UNTÄTIGEM
Gerät ausführt, bevorzugt am Ladekabel, praktisch also nachts; nimmt man das
Telefon in die Hand, bricht sie ab. `BGAppRefreshTask` ist die andere Klasse —
rund 30 Sekunden, dafür über den Tag verteilt nach Nutzungsmuster, und von Apple
ausdrücklich für „fetching new content, updating widgets" vorgesehen.
Gebaut ist deshalb ein ZWEITER Wecker unter eigener Kennung
(`modules/cal-ffi/ios/BackgroundRefresh.swift`): der Processing-Task macht
weiter die schwere Nachtrunde, der neue ist die kurze Aufholrunde. Eine gemeinsame
Kennung ginge nicht — ein zweites `submit` ersetzt den offenen Request, die
beiden Klassen würden sich gegenseitig löschen.
Er führt DIESELBE Arbeit aus: `runTasks` startet jede registrierte
TaskManager-Aufgabe für diesen Launch-Grund, also unsere Hintergrund-Sync-Aufgabe.
Kein zweiter Codepfad.
`AperioTaskServiceHelper.h/.m` findet Expos `EXTaskService` per Namen zur
LAUFZEIT, damit dieses Modul nicht gegen ExpoTaskManager linken muss; fehlt es,
kommt nil zurück und der Wecker tut still nichts.
Alles scheitert weich: kein Task-Service, keine erlaubte Kennung, ein abgelehntes
`submit` — jeweils eine Logzeile, und die App ist wie vorher.
⚠️ Ungeprüft, und der native Teil ist hier nicht übersetzbar.
GRENZE, die zu kennen wichtig ist: auch BGAppRefreshTask ist keine Zusage. Apple
schreibt, eine selten benutzte App bekommt unter Umständen GAR KEINE Refresh-Zeit.
Realistisch mehrmals täglich statt ein- bis zweimal — nicht alle 15 Minuten.
Was NICHT geht: stille Push-Nachrichten bräuchten einen Server und sind auf
wenige pro Gerät und Tag gedrosselt; iOS 26 bringt mit `BGContinuedProcessingTask`
nur eine Klasse für vom Nutzer angestoßene Vordergrundarbeit.

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
in 2b/2c verworfen worden; was wirklich gebaut wurde, steht dort. (Das
`CalFfi.xcframework` liegt nicht im Repo; der iOS-Workflow baut es bei jedem
Lauf frisch.)

Reihenfolge nach RISIKO, nicht nach Interesse: jeder Schritt kostet einen
EAS-Durchlauf und ist blind, also kommen die Fragen zuerst, deren Antwort alles
Übrige trägt.

- [x] **Schritt 0** — App-Group-Entitlement allein (`plugins/withAppGroup.js`),
      ohne Widget und ohne Datenbank-Umzug. Beantwortet: signiert die
      Capability überhaupt gegen unser Profil? Genau daran scheiterte Bau #5
      mit `aps-environment` (siehe `withoutPushEntitlement.js`).
- [x] **Schritt 1** — Widget-Target über `@bacons/apple-targets`  
  ↳ Erledigt: Target an drei Stellen verdrahtet; das Widget wurde seither AUF dem Telefon beobachtet, was nur eine installierte, signierte Extension kann.
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
- [x] **Schritt 2b/2c** — Widget 1 mit echten Daten, über eine SNAPSHOT-Datei  
  ↳ Erledigt inkl. der dreisprachigen Neusortierung zur Renderzeit. Was bleibt, sind benannte Randfälle, keine fehlende Implementierung.
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

      NACHTRAG (Gerätebefund): ein laufender Termin verdeckte alles, was IN ihm
      lag. Ein Blocktermin „Arbeitszeit 10–16" gewann ab 10:00 gegen jede
      Besprechung darin, und je länger er lief, desto sicherer — die Sortierung
      nahm den START. Jetzt ordnet ein LAUFENDER Termin nach seinem ENDE, womit
      der innerste zuerst kommt; dieselbe Begründung wie bei ganztägigen, nur
      eine Stufe früher.
      Das musste an DREI Stellen: der Schlüssel hängt jetzt an der Uhr, und die
      App friert ihre Antwort Stunden vor dem Zeichnen ein. Also sortieren
      `Snapshot.swift` und `WidgetStore.kt` zur RENDERZEIT neu — beide Zeitachsen
      legen ohnehin je einen Eintrag auf jeden Start und jedes Ende, also genau
      auf die Momente, an denen sich die Reihenfolge ändern kann.
      `widgetSnapshotWire.test.ts` bewacht, dass beide es weiter tun.
      ⚠️ Noch ungeprüft auf dem Gerät. Offen bleibt außerdem: die Galerie-Namen
      („Als Nächstes" / „Up Next") können nicht aus dem Snapshot kommen — sie
      werden gelesen, bevor Daten existieren — und hängen deshalb an
      `Locale.preferredLanguages` statt an der App-Sprache.
- [x] **Schritt 3** — Widget 3 „Nächster Termin" (`targets/widget/NextUp.swift`):  
  ↳ Erledigt. Zwei Stellen der Prosa sind ungenau statt unvollständig: der Countdown tickt NICHT sekündlich — NextUp.swift:107 baut pro Timeline-Eintrag einen String.
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

### A9 · Termingruppen `[x]` — Stufen 0 bis 3 stehen
Derselbe Termin liegt heute mehrfach in Aperio (Outlook für die Kollegen, Kopie
im Privatkalender für Alexas Erinnerungen, dazu die Kalender der Kollegen), und
Aperio weiß nichts davon: mehrfache Zeilen, mehrfaches Nachziehen bei jeder
Verschiebung, und ein Meeting-Link an einem zufällig gewählten davon.
Eine Gruppe ist eine Aussage von Aperio über fremde Daten — „diese Termine meinen
dieselbe Verabredung" —, lebt im eigenen Datensatz und erreicht keinen Anbieter.
Sie ersetzt die heutige Ja/Nein-Rückfrage beim Meeting-Verknüpfen ersatzlos.
Vollständiger Entwurf samt der schwierigen Teile (woher die Mitgliedschaft kommt,
wie eine Gruppe anbieterseitige Kennungswechsel überlebt, was bei fehlenden
Schreibrechten passiert) und einem Stufenplan: `DESIGN-event-groups.md`.

Gebaut sind die Stufen 0 bis 3: Modell und Synchronisation, das Falten in allen
sechs Ansichten, das Mitziehen einer Bearbeitung in allen drei Umfängen (ganze
Serie, nur dieses Vorkommen, dieses und alle folgenden), Erkennung samt
Vorschlagszeile mit endgültigem „Nein", das Meeting an der Gruppe und die
Selbstheilung nach Kennungswechseln. Nichts davon lief bisher auf einem Gerät.

Als **Entwurf** liegt daneben Stufe 4: die Meeting-Kalender als echte Kalender
sichtbar machen und ihre Zeilen automatisch mit dem Termin gruppieren, zu dem
sie gehören — über die Beitritts-URL, also über eine Kennung statt über eine
Ähnlichkeit. Das ersetzt die einzige Heuristik im Projekt, die still Daten
unterdrückt (`withoutDuplicateMeetings`). Entscheidungsgrundlage im Entwurf.

**Stufe 0 und 1 sind gebaut**: Migration 0035/0036, `EventGroupsRepo`, Sync über
`event_group.updated`/`.dissolved` samt Snapshot und Auflösungs-Marken, die
Aktion „Gehört zusammen mit…" in beiden Oberflächen, und das Zusammenfalten auf
allen sechs Ansichten (Desktop Tag/Woche/Monat/Agenda, mobil Tagesliste und
Agenda). Offen bleiben Stufe 2 (Bearbeitungsumfang über die Mitglieder, mit
ehrlicher Meldung über das, was nicht ging), Stufe 3 (Erkennung + Vorschlag; das
Meeting wandert an die Gruppe) und die Selbstheilung über die gespeicherte
Signatur, wenn ein Anbieter eine Kennung neu vergibt.

### A8 · Sprachbefehle (Siri / Kurzbefehle, iOS zuerst) `[x]` — code-vollstaendig
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

- [x] **Schritt 1** — beweisen, dass App-Target-Swift Siri überhaupt erreicht:  
  ↳ Erledigt: zwei Config-Plugin-Mods (dangerous mod schreibt die Dateien, Xcode-Mod hängt sie in die Build-Phasen). Gerätetest bestätigt.
      `plugins/withAppShortcuts.js` kopiert `mobile/ios-app/AperioShortcuts.swift`
      ins generierte App-Target und trägt es ins Xcode-Projekt ein. Inhalt: EIN
      Kurzbefehl, der nur die App öffnet.
      Der `AppShortcutsProvider` MUSS im Haupt-App-Target liegen — Apples
      Framework-Ausweg (`AppIntentsPackage`) gilt nur für Frameworks, nicht für
      die statischen Bibliotheken, zu denen Expo-Module übersetzen. Ein Pod
      scheidet damit aus.
      ✅ Gerätetest: Kurzbefehl erscheint, „Hey Siri, öffne Aperio" startet die
      App. Der Weg trägt.
- [x] **Schritt 2** — `CreateEventIntent` mit Titel + `Date`. Siri löst die  
  ↳ Erledigt (feste 60 Minuten; Kalender fällt auf zuletzt genutzt → erster schreibbarer zurück). Durch den Gerätetest von Schritt 2b end-to-end belegt.
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
      ⚠️ Ungeprüft. Das beim Gerätetest gemeldete „Aperio crashed" kam NICHT von
      hier — es war `0xdead10cc` (siehe Schritt 2a der Widgets). Der Kurzbefehl
      startet die App in den Hintergrund und lässt sie kurz darauf suspendieren,
      also traf er das Muster nur besonders zuverlässig.
- [x] **Schritt 2b** — deutsche Phrasen.  
  ↳ Erledigt: Phrasen in `de.lproj/AppShortcuts.strings`, geschlüsselt über den englischen Originalstring; ein vitest hält beide Dateien synchron.
      GERÄTEBEFUND, der die Notwendigkeit belegt: „erstelle einen neuen Termin
      mit Aperio" landete im APPLE-Kalender — erkennbar daran, dass dessen
      Rückfrage bei Terminüberschneidung kam.
      Zwei Ursachen, beide behoben. Erstens stand die deutsche Phrase als
      LITERAL im Swift-Array. Das ist die Entwicklungssprache; Siri sucht den
      Phrasensatz für SEINE Sprache in `<lang>.lproj/AppShortcuts.strings`,
      verschlüsselt nach der ENGLISCHEN Phrase. Ein deutscher String im Array
      wird als englischer registriert und einem deutschen Siri nie angeboten.
      Zweitens gab es je Intent nur EINE Formulierung. Siri muss den gesprochenen
      Satz fast wörtlich treffen; „mit" statt „in" genügt zum Verfehlen.
      Gebaut: `mobile/ios-app/de.lproj/AppShortcuts.strings` (nur Phrasen — der
      Dateiname ist fest, in `Localizable.strings` wirken sie nicht) und
      `de.lproj/Localizable.strings` (Titel, Beschreibungen, Parameternamen,
      Siris Rückfragen). Sechs Formulierungen für den Termin-Intent, „in"- UND
      „mit"-Formen. `withAppShortcuts.js` kopiert das `.lproj` und trägt es als
      Bundle-Ressource ein; das `.lproj` im PFAD macht die Lokalisierung, keine
      Variant-Group — dieselbe Form, die Expos eigenes `locales` benutzt.
      `${applicationName}` ist in JEDER Phrase Pflicht, sonst bricht der Bau ab.
      `src/intl/shortcutLocalization.test.ts` hält beide Seiten zusammen und
      schlägt nachweislich fehl, wenn eine Phrase ohne ihren Schlüssel wandert.
      ✅ Gerätetest: Termine per Sprache landen jetzt in Aperio statt im
      Apple-Kalender. `INAlternativeAppNames` wird also nicht gebraucht.
- [x] **Apple Intelligence (Assistant Schemas) — GESCHLOSSEN, mit Beleg.**
      Das wäre der Weg gewesen, der Siri FREIE Rede erlaubt statt einer festen
      Phrase: `@AssistantIntent(schema: .calendar.createEvent)`, und damit
      „erstelle einen Termin am Montag von 11 bis 13 mit dem Titel Essen kochen".
      **Es gibt im iOS-SDK keine Kalender-Domäne.** Nicht „falsch geschrieben",
      nicht „später dazugekommen" — sie ist nicht da.
      BEWEIS (Lauf 30996369787, Xcode 26.6 / iPhoneOS26.5.sdk):
      `.mail.createDraft` übersetzt im selben Lauf sauber durch — die Probe
      taugt also. `.calendar.createEvent` scheitert bei iOS-18- UND
      iOS-26-Ziel gleichermaßen an `type 'AssistantSchemas.Intent' has no
      member 'calendar'`. `AppSchema` existiert überhaupt nicht („cannot find
      'AppSchema' in scope"). Zählung im Modul-Interface: `calendar` 0,
      `CalendarIntent` 0, `AppSchema` 0, `createEvent` 0 — bei
      `AssistantSchemas` 422. Nur EIN Interface im ganzen SDK nennt die
      Namensräume; PrivateFrameworks: nichts.
      Apples Dokumentationsseite führt eine Kalender-Domäne (createEvent,
      deleteEvent, updateEvent, dazu Entitäten und Enums) und nennt sie
      `AppSchema.CalendarIntent`. Ausgeliefert ist davon nichts. Die Doku
      beschreibt hier etwas, das im SDK nicht steht.
      NEBENFUND: `@AssistantIntent(schema:)` ist VERALTET, umbenannt zu
      `@AppIntent`. Falls je eine Domäne benutzt wird, unter dem neuen Namen.
      Der Probe-Workflow BLEIBT liegen — nicht als offene Aufgabe, sondern als
      Nachprüfung: er beantwortet dieselbe Frage in zwei Minuten neu, sobald
      Apple ein SDK nachlegt. Ein Lauf ersetzt eine Rateschleife aus
      25-Minuten-Bauten.
      LEHRE, die über diesen Punkt hinausgeht: Runde 1 stellte EINE Frage in
      EINER Schreibweise bei EINEM Ziel-Betriebssystem und ohne Kontrollfall —
      und ihr Ergebnis war deshalb nicht auswertbar, weil „Domäne fehlt" und
      „Frage falsch gestellt" identisch aussahen. Erst der Kontrollfall machte
      die Antwort zu einem Befund.
      Für Toni ändert sich ohnehin nichts: sein Gerät meldet
      `deviceNotCapable`. Der geführte Dialog aus Schritt 2 ist der Weg, der
      bei ihm wirkt.
- [x] **Schritt 3** — Kalender und Aufgabenliste als `AppEntity` mit  
  ↳ Erledigt, jede Auflage wörtlich eingehalten (EntityStringQuery statt EntityQuery, Snapshot-Datei statt DB-Handle im Intent-Prozess, `__default__` zuerst).
      `EntityStringQuery` (nicht `EntityQuery`: nur die String-Variante kann
      einen GESPROCHENEN Namen auflösen, und genau darum geht es).
      AUSLÖSER war ein Gerätebefund: Termine per Sprache landeten immer im
      falschen Kalender. Kein Zufall — die App nahm den zuletzt im EDITOR
      benutzten. Für einen Knopf neben einer Kalenderauswahl ist das ein guter
      Standard, für einen quer durch den Raum gesprochenen Satz ein schlechter.
      Die Liste reist über dieselbe Snapshot-Mechanik wie beim Widget
      (`pickers.json` in der App Group, `VoicePickerStore.swift`) — der Intent
      kommt so wenig an die Datenbank wie die Widget-Extension, und das ist nach
      `0xdead10cc` kein Stilfrage mehr.
      ERSTER Eintrag ist immer „Standardkalender" / „Standardliste" mit der
      reservierten id `__default__`. Ein Pflichtparameter mit leerer Liste wäre
      eine Sackgasse — Siri fragt etwas, das nicht beantwortbar ist —, und leer
      IST sie auf einem Telefon, das noch keinen Durchlauf hatte. Die App liest
      eine fehlende id weiterhin als „such du aus".
      Nur beschreibbare und nicht ausgeblendete Kalender; bei Aufgabenlisten
      dagegen ALLE, weil die Listenauswahl in der Aufgabenansicht ein Fokus-
      Werkzeug ist und keine Aussage darüber, was existiert.
- [x] **Schritt 4** — tatsächlich anlegen, jetzt auch Aufgaben.  
  ↳ Code-vollständig inkl. der zwei feinen Regeln (Tag ohne Uhrzeit; nie den Rust-Kern im Intent-Prozess öffnen). 🚩 `CreateTaskIntent` ist der einzige Intent ohne Gerätebericht.
      `CreateTaskIntent`: Titel + Liste werden gefragt, der Tag NICHT — optionale
      Parameter fragt Siri nicht ab, und das ist richtig so: die meisten
      gesprochenen Aufgaben sind „merk dir das" und gehören ins Backlog, nicht
      auf einen Tag. Über die Kurzbefehle-App ist der Tag trotzdem setzbar.
      Gespeichert wird nur das DATUM, nie die Uhrzeit: ein iOS-Datumsparameter
      trägt immer eine mit, und eine per Tagesauswahl gebaute Aktion liefert
      Mitternacht — das würde „irgendwann am Dienstag" zu „fällig um 0:00".
      `openAppWhenRun` bleibt bei beiden: beim Haken ist Verzögerung unsichtbar,
      beim ANLEGEN nicht — man sagt etwas und findet minutenlang nichts.
      ⚠️ Ungeprüft.
      Den Rust-Kern direkt aus dem Intent zu öffnen ist bewusst KEINE Option:
      zweiter schreibender Prozess auf einer Datenbank, plus ein teurer
      Host-Start pro Sprachbefehl.

- [x] **Android** — „Als Nächstes" über **Glance**  
  ↳ Erledigt und im Baum nachprüfbar (Glance für die TalkBack-lesbare CheckBox, `filesDir/widget` statt App Group, Receiver mit exported=true). 🚩 Ohne Android-Hardware ungeprüft.
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
- [~] Retry bei Reconnect inkl. ETag-Prüfung  
  ↳ Die ETag-Hälfte existiert überall (bedingte Writes, 412-Erkennung). Es fehlt nur der Puffer aus Punkt 1 — ohne Queue gibt es nichts zum Nachspielen.
- Hinweis: Der **lokale** Sync-Log existiert; gemeint ist die Pufferung von Schreibzugriffen auf **externe** Provider.
- Einstieg: neue Migration + die Mutationspfade in `src-tauri/src/commands/`.

### A10 · Adapter in eigene Repositories herauslösen (§20) `[~]`

Entscheidung vom 2026-09-07: Kern + UI bleiben ein Repo, die **Adapter** ziehen
aus — sie waren von Anfang an eigenständige Projekte, dafür gibt es das
Plugin-System. Ablösbar sind **12**: `adapter-local` und
`adapter-device-calendar` haben keine `-plugin`/`-cdylib`-Schicht, sie sind über
`host_core::builtin_adapters` fest eingebaut.

Erst entkoppeln, dann umziehen. Die ersten fünf Pakete sind für sich wertvoll,
auch falls nie ein Repo entsteht.

- [x] **Paket 1 — Manifeste gehören ihrer Kiste.** Jede der 14 Kisten mit
  `plugin.json` exportiert `pub const MANIFEST: &[u8]`; alle 35 relativen
  `include_bytes!`-Zugriffe lesen jetzt diese Konstante. Fünf bis dahin gar
  nicht deklarierte cargo-Kanten sind nachgetragen. Zwei Wächter in
  `crates/host-plugins/tests/manifest_reach.rs`.
- [x] **Paket 2 — Tests über die Registry statt den Dateibaum.**  
  ↳ `manifests_parse.rs` fragt jetzt einen von `register_all_static` gefüllten
  `PluginManager` plus `builtin_adapters::builtin_manifests()`; die Tabellen
  schlüsseln nach Plugin-Id statt Verzeichnisname. Am simulierten
  Nach-Umzug-Baum gemessen: vorher fielen 4 der 7 Tests um, jetzt bleiben alle
  8 grün. Übrig bleiben zwei Baum-Fragen, beide in einem Test: kein
  `plugin.json`, das kein Build deklariert, und keine zwei Kisten mit derselben
  Plugin-Id — Letzteres kann die Registry gar nicht sehen, weil der Manager die
  zweite Registrierung ablehnt.
- [x] **Paket 3 — Ein Adapter kann sich selbst benennen.**  
  ↳ `kind_names` im Manifest (langer + kurzer Name, wörtlich + Katalog-Schlüssel),
  aufgelöst in `adapter_kinds(lang)` und `builtin_adapter_kinds(lang)`. Die 68
  Locale-Einträge sind weg, die Frontends lesen den Namen aus `AdapterKindInfo`.
  Der a11y-Fehler ist damit erledigt: keine Aufrufstelle kann mehr einen
  Punkt-Schlüssel vorlesen lassen. Der Name reitet auf der KONTOZEILE mit
  (`list_accounts` liefert `kind_name`), weil eine Zeile anders fragt als ein
  Picker: sie wird gezeichnet, auch wenn das Plugin deaktiviert ist, und ohne
  zweite Anfrage, die zu spät kommen oder scheitern kann. Wächter: jedes Kind in
  jeder Sprache seines Adapters benannt (Eigenschaft EINES Manifests), die App
  trägt keine Adapternamen mehr, und ein deaktiviertes Plugin behält den Namen
  seiner Konten.
- [x] **Paket 4 — Staging aus `build.rs` in ein xtask.**  
  ↳ `cargo xtask stage-plugins`, nach dem Workspace-Build statt während. Damit
  ist das Zwei-Build-Rennen weg (auch der zweite `cargo build -p aperio` in
  `tauri.conf.json`), eine fehlende cdylib ist ein Fehler statt einer
  `cargo:warning`, und die Plugin-Liste ist abgeleitet: ein Mitglied, das eine
  cdylib erzeugt und von einer `*-plugin`-Kiste abhängt. Vier Ableitungen
  derselben Zahl sind auf eine geschrumpft — die Tabelle in `build.rs`, drei
  `sed`/`grep`-Stellen in den Workflows und der
  `include_str!("../build.rs")`-Scrape sind alle weg.
- [x] **Paket 5 — `.aperio`-Packer.**  
  ↳ `plugin_core::pack_archive` liegt neben dem Leser, damit beide Hälften
  dieselbe Vorstellung vom Format haben: Packen ist „dieses Verzeichnis zippen",
  Installieren „dieses Zip auspacken", und die gestagete Ablage IST die Ablage
  im Archiv. `cargo xtask pack-plugins` erzeugt die zwölf; ein Test packt,
  inspiziert, installiert und **lädt** jeden echten Adapter. Verweigert werden
  ein Verzeichnis ohne Manifest und eines ohne Bibliothek. Die Autoren-Doku
  beschreibt jetzt das Format, statt „irgendwie in ein `.aperio` packen" zu
  sagen. OFFEN bleibt die Architektur-Dimension: das Archiv unterscheidet
  windows-x64 nicht von windows-arm64, der Triple steht nur im Dateinamen.
- [x] **Paket 6 — ABI-Vorwärtsfähigkeit (`struct_size`).**  
  ↳ ABI 4 hängt **nichts** an: jede der sechs Vtables bekommt ein `struct_size`
  in die vier Byte Padding, die auf 64-Bit ohnehin hinter `vtable_version`
  lagen. Kein Slot ist gewandert, keine Vtable gewachsen — der bestehende
  `vtable_sizes_match_c_header` beweist es, weil er unverändert grün bleibt.
  Der Host dereferenziert eine fremde Vtable nicht mehr, sondern kopiert über
  `read_vtable` genau die Bytes, die das Plugin nach eigener Angabe geschrieben
  hat, in eine genullte eigene Struktur; ein Slot, den das Plugin noch nicht
  kannte, kommt als `None` an. Damit lädt ein Plugin für ABI 3 weiter, und
  **Anhängen kostet ab jetzt keine ABI-Erhöhung mehr** — genau die Bremse, die
  einen ausgelagerten Adapter sonst bei jeder Methode am Kern festgenagelt
  hätte. Verifiziert nicht nur im Test: eine echte, aus `a6559338` gebaute
  ABI-3-cdylib lädt im ABI-4-Host (rot bewiesen per Sabotage).

  Der Review fand die Falle dabei: v3 trägt keine Länge, und die naheliegende
  Ersatzgröße — `size_of::<T>()` des Hosts — ist genau bis zum ersten
  angehängten Slot richtig. Danach läse der Host jedes v3-Plugin über sein Ende
  hinaus und riefe auf, was dahinter liegt; simuliert und reproduziert
  (`lock_meeting = Some(0x8000...)`). Die Größen von Revision 3 stehen jetzt
  je Vtable als Konstante da (`ForeignVtable::REVISION_3_SIZE`), sind eine
  historische Tatsache und wandern beim Anhängen nicht mit.
- [x] **Paket 7 — Wächter, die auch anschlagen können.**
  ↳ Drei Löcher, alle in der Kategorie „grün, weil nichts geprüft wurde“:
  (1) `stage-plugins` fragte allein den Workspace, also hätte ein Adapter, der
  ihn VERLÄSST, elf statt zwölf ergeben — gestaget, gemeldet, grün, Artefakt
  ohne Adapter. Geprüft wird jetzt gegen die Adapter-Features von
  `host-plugins`, die der Mobile-Host ohnehin trägt; nebenbei müssen Desktop
  und Mobile damit dieselben zwölf führen. (2) Der Reach-Wächter fragte
  `arg.contains("plugin.json")` und konnte deshalb nur bei einem Dateinamen
  anschlagen; er fragt jetzt, ob der Pfad die eigene Kiste verlässt, und fand
  sofort den Zugriff, der die ganze Zeit dalag. (3) `TASK_ASSIGNMENT` hörte
  still auf zu fragen, wenn ein Plugin nicht mehr da war. Dazu: `shared/contracts/**`
  löst jetzt die Rust-Jobs aus (beide Rust-Hälften des Wire-Contracts wurden
  bei genau dem Commit übersprungen, der ihn ändert), und die toten Features
  `dynamic-plugins`/`static-plugins` sind weg — kein `cfg` hat sie je gelesen,
  DESIGN §20.6 beschrieb sie trotzdem als den Desktop/Mobile-Schalter.

  Der Review des Branches fand danach drei weitere, zwei davon in genau dieser
  Arbeit: (a) der verbreiterte Reach-Wächter las nur nackte String-Literale,
  liess also `concat!(env!("CARGO_MANIFEST_DIR"), "/../../shared/…")` durch —
  ausgerechnet die Schreibweise, die dieses Repo 14-mal benutzt — und behauptete
  im Doc-Kommentar, sie koenne gar nicht entkommen; (b) `KNOWN_REACHES`
  entschuldigte eine DATEI statt eines Zugriffs, die drei genannten Leser
  durften also alles einbinden, und die Verschwinde-Pruefung war mit
  irgendeinem Zugriff zufrieden. (c) Und das Versprechen „Rust-Plugins bekommen
  das Panik-Fangen geschenkt" galt fuer sechs Dispatch-Helfer, waehrend Header
  und Doku „jeden Slot, jeden Export" sagten — 39 Eintrittspunkte, davon 17
  lebende Vtable-Slots, brachen weiter ab. Jetzt faengt jeder, und
  `panic_boundary.rs` laesst keinen neuen durch.
- [x] **Zwei blinde Flecken in CI geschlossen.**
  ↳ `locales/**` steht jetzt im `rust`- UND im `frontend`-Filter: der Wächter
  `the_app_no_longer_carries_names_for_adapters` LIEST
  `locales/{en,de}/translation.json` zur Laufzeit, und `src/i18n.ts` importiert
  dieselben Dateien ins ausgelieferte Bundle. Ein Commit, der nur eine
  Übersetzung ändert, kompilierte vorher gar nichts.
  ↳ Neuer `mobile`-Job mit `tsc --noEmit`. Der eigentliche Befund war schärfer
  als der Review-Vorwurf: das Root-`tsconfig.json` schließt `src`, `locales`,
  `shared` ein — **`mobile` nicht**. Mobiles TypeScript war also auf KEINEM
  Commit je typgeprüft, nicht bloß auf mobile-only-Commits. Der Job braucht
  BEIDE `npm ci` (Root für `shared/`s `rrule`/`linkify-it`, dann mobile), sonst
  meldet tsc vier Fehler in Dateien, die in Ordnung sind.
  ↳ Ehrlich dazu: gemessen hat **keines** der beiden Löcher je gefeuert — in
  der ganzen Historie gibt es null locales-only- und null mobile-only-Commits,
  weil die Desktop-Mobile-Parität dafür sorgt, dass nie eine Seite allein
  wandert. Es waren Löcher, keine Wunden. Der Mobile-Typcheck dagegen fehlte
  wirklich immer.
- [x] **Probe-Umzug einmal komplett durchgespielt** (vikunja, 2026-09-08,
  danach restlos abgebaut). Es geht, und es fällt überall LAUT aus: Pfad-Deps
  überleben keine git-Konsumtion; ohne `[patch]` zwei `plugin-core` und E0308 an
  `register_static`; mit `[patch]` vereinheitlicht cargo sauber (nachgemessen);
  die cdylib-Hülle muss im App-Repo bleiben. Der Wächter aus 6dabd91b hat den
  fehlenden Adapter namentlich gemeldet. Gefehlt hat genau eine Zeile — sie ist
  jetzt drin: `metadata()` fragt mit Abhängigkeiten, `discover` filtert die
  Hüllen über `workspace_members`. App 2213→2106 Tests, Adapter-Repo 107.
  OFFEN: ob der iOS-Build dieselbe Vereinheitlichung erreicht (nur CI kann das).
  Preis je Repo: ~40 Zeilen materialisiertes `[workspace.*]`; `reqwest`
  WÖRTLICH kopieren, sonst kippt die globale Feature-Vereinigung den TLS-Stack.
- [x] **Eine Tür statt vier.** `plugin-sdk` re-exportiert jetzt `cal_core`,
  `sync_core` und `vc_core` neben dem `plugin_core`, das es ohnehin schon
  weiterreichte; die zwölf `-plugin`- und zwölf `-cdylib`-Kisten gehen darüber
  (64 Ersetzungen in `src/`, 29 in `tests/`) und haben ihre Abhängigkeiten
  abgelegt. Ein Adapter-Tripel nennt damit **zwei** Aperio-Kisten statt drei,
  und die Plugin-Hälfte nur noch eine. Nebenbei repariert: die SDK-Makros
  expandierten zu `::cal_core::` — ein absoluter Pfad in die Kiste des
  AUFRUFERS —, jetzt `$crate::cal_core::`, also selbsttragend.
  `one_door.rs` fragt die Manifeste (ein `use` lässt sich umschreiben, eine
  Abhängigkeit muss deklariert werden), rot bewiesen. 2213 Tests unverändert.
  OFFEN, Tonis Entscheidung: die `adapter-X`-Logikkiste bleibt bei ihrer
  Domänen-Kiste — auf eins käme man nur, wenn auch der reine HTTP-Client das
  FFI-SDK einbindet.
- [ ] Danach erst: Versionierung der fünf Vertragskisten, `adapter-caldav`s
  Zugriff auf `shared/contracts/` auflösen — und ein erster Umzug mit **einem**
  Adapter als Probe. **Vikunja, nicht webdav**: `host-core` greift an sechs
  Stellen in `adapter_webdav_plugin` hinein und die App kodiert webdavs
  Init-Config-Schema an drei lebenden Stellen (`sync_target/build.rs`); vikunja
  nennt `host-core` gar nicht. Beide Zugriffe von `adapter-caldav` sind für die
  Probe irrelevant — weder vikunja noch webdav haben einen.
- Offene Entscheidung: behält Mobile git-Abhängigkeiten auf alle zwölf
  `-plugin`-Kisten? iOS verbietet dlopen, „eigenständig" kann dort nie mehr
  heißen als „gepinnte Quell-Revision".

### A11 · Kern und Oberflächen trennen `[~]`

Vorbereitung dafür, dass die UIs (Desktop, Mobile, später reMarkable) in
eigenen Repositories liegen und ein Adapter **nur den Kern** anfordern kann.
Siehe DESIGN §4.2.

- [x] **Schritt 1: die Domänentypen werden erzeugt, nicht abgeschrieben.**
  ↳ `shared/types.ts` war 285 Zeilen Handschrift mit dem Satz „die Kiste
  `cal-core` ist die Quelle der Wahrheit; ändert sich dort ein Feld, spiegle es
  hier" im eigenen Kopf — geprüft hat das nichts, und sie war abgedriftet.
  Jetzt tragen 31 Typen in `cal-core`/`plugin-core`/`host-core` den ts-rs-Derive
  hinter einem standardmäßig **ausgeschalteten** `ts-export`-Feature (cargo
  vereinheitlicht Features global; zwölf Adapter-Repos übersetzen gegen diese
  Kisten), `cargo xtask ts-types` schreibt `shared/generated/`, und
  `shared/types.ts` ist nur noch die Tür. Kein Konsument musste seinen Import
  ändern.
  ↳ Nebenbei aufgelöst: `BackendRecurrence` in `shared/taskRecurrence.ts` (eine
  zweite Handkopie von `TaskRecurrence`) und `DEFAULT_CAPS` in
  `src/state/taskMoves.ts` (eine dritte von `TaskCapabilities::default()`, der
  zwei Felder fehlten). `Task.recurrence` war `unknown` und ist jetzt der echte
  Typ.
  ↳ Der Wächter (`cargo xtask ts-types --check` im Rust-Job) wurde an drei
  Sabotagen rot bewiesen: Handeditierung einer erzeugten Datei
  (`outdated: Section.ts`), gelöschte + verwaiste Datei (`missing:` / `stale:`),
  und ein umbenanntes `ts-export`-Feature. Er **nennt** die Dateien, er zählt
  sie nicht.
- [x] **Die Behälter-Zeilen liegen einmal, in `host_core::wire`.**
  ↳ `CalendarRow`, `TaskListRow` und `ContactListRow` waren je zweimal
  deklariert (Tauri-Befehle + cal-ffi), von Hand in Schritt gehalten. Die
  Task-Zeile war auseinandergelaufen: Mobile trug ein `recurrence_capabilities`,
  der Desktop nicht — also fragten die beiden Aufgaben-Editoren
  **verschiedene** Manifestfelder ab (`tasks.recurrence` gegen das top-level
  `recurrence`, das Termin-Wiederholungen beschreibt). Bei elf Adaptern sagen
  die dasselbe; **bei Vikunja nicht**, und Mobile bot deshalb Wiederholungen an,
  die Vikunja nicht speichern kann. Das Feld ist von der Aufgaben-Zeile weg (es
  gehörte nie dorthin), Mobile liest `caps?.recurrence` wie der Desktop, und
  `TaskListRow` wird jetzt mit nach TypeScript erzeugt — `shared/types.ts` setzt
  `TaskList` nicht mehr von Hand zusammen.
  ↳ Der Wächter für die Aufgaben-Zeile ist der Codegen-Check selbst: ein Feld
  mehr oder weniger an `TaskListRow` macht `cargo xtask ts-types --check` rot,
  bis es neu erzeugt ist, und die erzeugte TS-Datei steht dann im Diff.
  `CalendarRow` und `ContactListRow` haben den nicht — `cal_core::Calendar` und
  `ContactList` tragen noch keinen ts-rs-Derive. Sie mitzuerzeugen wäre der
  natürliche nächste Codegen-Schritt und würde nebenbei den handgeschriebenen
  `Calendar`-Typ in `src/api/types.ts` (559 Zeilen Handschrift, von Schritt 1
  nicht angefasst) ablösen.
  ↳ Nebenbei repariert: `ts-export` schaltete das Feature nicht in den Kisten
  an, deren Typen es nennt. `cargo build -p host-core --features ts-export`
  allein war eine Wand aus „trait bound `X: TS` is not satisfied"; nur weil die
  xtask immer alle drei Features zusammen übergab, fiel es nicht auf.
- [x] **Das Kontoformular wird einmal gebaut, in `host_core::account_form`.**
  ↳ Zweiter Fall desselben Musters wie die Behälter-Zeilen: der Desktop baute
  die Spec in `src-tauri/src/commands/accounts.rs`, `cal-ffi` baute sie
  daneben von Hand mit `serde_json::json!` — und die beiden waren **in beide
  Richtungen** auseinandergelaufen. Mobile fehlten `options`, `default_bool`,
  `default_text` und `device_local`; Mobile schickte umgekehrt ein
  `app_redirect_uri`, das der Desktop nie sendete und kein Frontend liest.
  ↳ **Das war ein Absturz, kein Schönheitsfehler.**
  `mobile/src/components/AccountSchemaForm.tsx:97` macht bei `kind === 'choice'`
  ein `field.options.map(…)`, und **FTP und SFTP deklarieren choice-Felder** —
  ein FTP-Konto auf dem Telefon anzulegen nahm das Formular mit. Gefunden von
  der Neumessung (siehe unten), selbst nachgeprüft.
  ↳ **Warum nichts es gefangen hat:** die mobile Seite parst die
  Brücken-Antwort mit einem nackten `as AccountFormSpec`-Cast. Der
  handgeschriebene TS-Typ sagte, der Schlüssel sei da, weil ein Mensch ihn dort
  hingetippt hatte. Jetzt sind die Typen erzeugt, der Cast beschreibt also
  etwas, das eine Maschine aus dem Rust abgeleitet hat.
  ↳ Nebenbei: `kind` war beidseitig ein handgetippter String-Union. Die Zeile
  trägt jetzt `plugin_core::AccountFieldKind` selbst; eine neue Feldart ist
  damit ein Übersetzungsfehler in beiden Frontends, bis sie behandelt ist.
  ↳ Wächter: `crates/host-plugins/tests/account_form_wire.rs`, rot bewiesen mit
  `#[serde(skip)]` auf `options` — „field String("transport") has no `options`".
  Das Fixture-Manifest steht IM Test, nicht in einer Nachbarkiste: der Test muss
  den Adapter-Auszug überleben.
- [x] **Die Eigentums-Regel liegt im Kern.** „Ist das meine Aufgabe?" — die
  einzige Regel im ganzen `shared/`-Satz, die schon zweimal geschrieben war:
  in `shared/taskAssignment.ts` und **privat** in `host_core::reminders`, wo
  Rust nicht einmal seine eigene Kopie wiederverwenden konnte. Jetzt
  `cal_core::is_mine_or_unassigned`.
  ↳ Die TypeScript-Hälfte bleibt (sie wird synchron im Render gebraucht), die
  beiden sind daher gegeneinander festgenagelt durch
  `shared/contracts/taskOwnership.json` — kein Wire-Format, sondern eine
  **Entscheidung**, die beide Seiten unabhängig auf denselben Daten treffen.
  Läuft sie auseinander, klingelt das Telefon für die Aufgabe einer Kollegin
  oder eine eigene bleibt stumm; nichts stürzt ab, nichts protokolliert.
  ↳ Vier Sabotagen rot bewiesen (Rust-Regel gebrochen, TS-Regel gebrochen,
  Fixture geleert → beide Anti-Stille-Wächter), und der Reach-Wächter meldete
  die neue `include_str!`-Zeile von sich aus, bevor sie in `KNOWN_REACHES`
  stand.
- [ ] **`reparent_task_list` antwortet mit der nackten `cal_core::TaskList`**,
  ohne `account_id` und ohne Fähigkeiten, während `list_task_lists` sie
  anstempelt. Beide Aufrufer (Desktop-Sidebar, mobiler Listeneditor) verwerfen
  die Antwort, es ist also latent; der Typ heißt jetzt `TaskListCore` und sagt
  die Wahrheit. Sauber wäre, die Zeile auch dort anzustempeln.
- [~] **Schritt 2 vermessen (2026-09-09), und die Fragestellung war falsch.**
  Acht Module wurden einzeln durchgemessen (Reinheit, jede Aufrufstelle,
  Rust-Gegenstück, Kosten). Befund: **es gibt keine Einheit in Modulgröße, die
  umziehen könnte** — jedes der acht zerfällt in eine Domänen-Hälfte, die in den
  Kern gehört, und eine Ansichts-Hälfte, die nicht weg kann. Und der Blocker ist
  bei sechs von acht **nicht** die IPC-Runde, sondern das Fehlen einer
  **synchronen** Kern-Bindung; kein Umbau eines Befehls behebt das.
  Sieben von acht „large"-Schätzungen sind derselbe eine Befund.
  ↳ Nur EINE echte Doppelung existierte: `is_mine_or_unassigned`, in
  `host-core/src/reminders.rs` **privat** und in `shared/taskAssignment.ts`.
  Erledigt — siehe den Eintrag oben.
  ↳ Echte Divergenz, unabhängig vom Umzug: `shared/recurrence.ts` und
  `host-core`s `expand_occurrences` dokumentieren **verschiedenes**
  DST-Randverhalten, und Rust deckelt bei `RRULESET_LIMIT=500`, JS gar nicht.
  Braucht eine sprachübergreifende Fixture, bevor irgendetwas
  Wiederholungs-Förmiges umzieht — und die läuft mangels Test-Runner nicht auf
  Mobile.
  ↳ Gemessen 2026-09-14: `eventOccurrences.json`, siehe den Eintrag
  „Termin-Wiederholung zieht in den Kern".
  ↳ Messlücke, ehrlich benannt: `collapseEventGroups` (202), `dayGridLayout`
  (269), `taskRecurrence` (178) und `taskAssignment` (87) liegen INNERHALB der
  vermessenen Abhängigkeiten und wurden nicht vermessen.
- [x] **Fehlersemantik einer Kaskade ENTSCHIEDEN** (Toni, 2026-09-09):
  **alles versuchen, dann berichten.** Kein Abbruch beim ersten Fehler; die
  Ansicht wird auch im Fehlerfall neu geladen; und es wird immer angesagt, was
  wirklich passiert ist. Gilt schon jetzt für die Frontend-Kaskade (siehe
  DESIGN §9.1) und ist damit auch die Vorgabe, falls die Kaskade später in
  `update_task` wandert.
- [x] **Die Bindungsfrage ist beantwortet — es geht** (Toni: „lass uns das mit
  wasm probieren", 2026-09-09). `crates/cal-core-wasm` ist gebaut und im echten
  Renderpfad: 26,7 KB, 1,5 s inkrementeller Bau, und der Härtefall läuft — ein
  `Array.sort`-Vergleicher ruft `priorityRank` synchron aus Rust.
  ↳ Die Frage war **nur** die des Desktops. Mobile kann es längst
  (`Function("parseAttendee")` statt `AsyncFunction`, ausgeliefert), eine
  native reMarkable-Oberfläche bekommt es geschenkt. Hermes' fehlendes WASM ist
  damit kein Blocker: eine Rust-Quelle, zwei Bindungen.
  ↳ Gewächter durch `src/wasm/coreRules.parity.test.ts` — Rust gegen
  TypeScript über den **vollständigen** Eingaberaum. Rot bewiesen: verdrehte
  zweistufige Rangfolge in Rust → `priorityRank(low, two): expected +0 to be 1`.
  ↳ Damit müssen die „large"-Schätzungen der Messung neu gemacht werden: sieben
  von acht waren derselbe eine Befund („async ist die falsche Form"), und der
  ist jetzt aufgelöst. Siehe DESIGN §4.3.
- [x] **Die drei offenen Fragen sind entschieden** (Toni, 2026-09-09):
  ↳ **Vorarbeit erledigt:** zehn der einundzwanzig `localeCompare`-Aufrufe
  verglichen Maschinen-Zeichenketten (ISO-Tage, RFC-3339-Zeitstempel) und
  brauchten nie eine Kollation; sie gehen jetzt über `compareMachineStrings`.
  Damit ist die ICU-Fläche EINE Kollationsfunktion für ~10 Textstellen, nicht
  „jede Liste".
  ↳ **GEMESSEN, und die Zahl ist unangenehm:** `icu_collator` mit
  eingebackenen Daten kostet im WASM-Modul **1.139,7 KB** statt 26,7 KB —
  Faktor 43 (die fertige Kiste mit beiden Vergleichsfunktionen:
  1.154,3 KB). Ein auf de+en gekürzter Datensatz ist NICHT gemessen; Erwartung
  (keine Messung): bringt wenig, weil die CLDR-Wurzeltabelle für jede Sprache
  gebraucht wird. Toni hat den Preis genommen („zieh icu in den kern, das
  wiegt nicht so schwer") — siehe den Eintrag unten.
  ↳ Unbeantwortet und relevant: honoriert Hermes `{ numeric: true }` auf iOS
  UND Android? Falls nicht, sortiert die mobile Aufgabenliste **heute schon**
  anders als der Desktop — dann repariert ICU im Kern etwas Bestehendes statt
  nur Zukunft abzusichern. Braucht einen Geräte-Test.
  ↳ **Sortierung: `icu_collator` in den Kern.** Damit verhält sich die Ordnung
  überall wie heute, auch bei Umlauten und gemischten Zahlen — und
  `taskGrouping`s untere Hälfte kann überhaupt erst umziehen, weil
  `naturalCompare` ihr Tiebreaker ist. Preis bewusst in Kauf genommen: die
  ICU-Daten wiegen auf Mobile. Beim Bauen prüfen, wie viel genau, und ob eine
  gekürzte Datensammlung reicht.
  ↳ **reMarkable bekommt den VOLLEN Wiederholungs-Editor.** Damit muss
  `shared/rrule.ts` (256 Zeilen, kein Rust-Gegenstück — `cal-core`s
  `recurrence.rs` lässt die relativen Wochentags-Achsen bewusst weg) in den
  Kern, und die synchrone Bindung ist dort Pflicht, weil `parseRRule` pro
  Tastendruck läuft. Das ist der größte Einzelposten der Trennung, und er ist
  jetzt eingeplant statt offen.
  ↳ **Die Glyphen bleiben vorne** (`○ ◐ ● ⊘`, `★`, `!!!`). Der Kern gibt einen
  ZUSTAND zurück, jede Oberfläche wählt ihr Zeichen — eine e-ink-Anzeige will
  plausibel andere. Damit ist auch klar, wie `taskStatus` zerfällt: die acht
  reinen Funktionen können in den Kern, die Marken-Funktionen nicht.
- [x] **Die Prioritäts-Regel liegt im Kern** (2026-09-09), erster Umzug unter
  der Linie „`cal-core` ist DER Kern". `cal_core::task_priority` hält
  `priority_rank`, `normal_priority`, `TaskPriority::is_important` und die
  erzeugte `PriorityScale`.
  ↳ **Der Ausgangszustand war schlechter als er aussah:** die Regel lag in
  `crates/cal-core-wasm` — der Kiste des DESKTOPS. Erreichbar von genau einer
  Oberfläche, während Mobile und `shared/taskGrouping.ts` die TypeScript-Kopie
  fuhren. Drei Implementierungen einer Reihenfolge, die sich nicht
  unterscheiden darf.
  ↳ Beide Türen gebaut: `#[wasm_bindgen]` für den Desktop (delegiert jetzt,
  statt selbst zu rechnen), drei `#[uniffi::export]`-freie Funktionen plus neu
  erzeugte Kotlin-Bindungen und beide Expo-Brücken für Mobile.
  `shared/taskStatus.ts` hält die Tür (`installTaskPriorityRules`), die
  TS-Kopie ist weg, und `src/intl/taskStatus.ts` ist wieder ein nacktes
  `export *`.
  ↳ **Der FFI-Wächter hat sich selbst bewährt:** kaum standen die drei freien
  Funktionen, meldete er von sich aus rot („the bindings are stale, and the
  Android build will fail on it") — genau die Lücke, die heute früh noch GRÜN
  gemeldet hätte. Er nennt jetzt acht freie Funktionen statt fünf.
  ↳ **Ein Test gelöscht statt angepasst:** `coreRules.parity.test.ts` fragte
  „antwortet Rust dasselbe wie TypeScript?". Mit einer Implementierung hätte er
  Rust gegen Rust verglichen. Ersetzt durch `src/intl/taskPriority.test.ts`,
  das die Ränge AUSGESCHRIEBEN prüft; rot bewiesen an der verdrehten
  Zwei-Stufen-Rangfolge (die wichtige Aufgabe landet hinten).
  ↳ Nebenbei: die TS-Fassung tolerierte eine Aufgabe OHNE Priorität still
  (`undefined - undefined` = NaN, `NaN || …` fällt durch); Rust wirft. Kein
  Produktionspfad kann das erzeugen — `priority` ist auf `Task` Pflicht —, aber
  ein Test-Fixture konnte es, und der lautere Fehler hat es gefunden.
- [x] **Die Textordnung liegt im Kern** (Toni: „zieh icu in den kern, das wiegt
  nicht so schwer", 2026-09-09). `cal_core::collation` bietet genau zwei
  Regeln — `compare_names` (Namen von Dingen: Konten, Behälter, Kontakte,
  Tagesmarkierungen; Groß-/Kleinschreibung und Akzente trennen nicht) und
  `compare_titles` (selbstgeschriebener Text; Ziffernläufe nach Wert, Fälle
  trennen). Beide Oberflächen gehen durch ihre eigene Tür: der Desktop über
  WebAssembly, Mobile über eine synchrone Expo-`Function`. In `shared/` ist
  kein `localeCompare` mehr. Siehe DESIGN §4.4.
  ↳ **Drei stille Fehler nebenbei behoben:** die Regel lag nur in JavaScript
  (reMarkable hätte eine dritte Ordnung gebraucht); jeder Aufruf übergab
  `undefined` als Locale und folgte damit dem BETRIEBSSYSTEM statt der in
  Aperio gewählten Sprache; und Webview, Hermes/iOS und Hermes/Android bringen
  je ihre eigene Kollation mit.
  ↳ **Installiert statt durchgereicht:** `installTextCollation` in
  `shared/ordering.ts`, von jeder Oberfläche beim Start gesetzt und beim
  Sprachwechsel neu gebunden. `taskOrder`/`sectionOrder` werden aus neun
  Schichten Ansichtscode gerufen — eine Sprache durchzufädeln hätte die Wahl
  vor jeden künftigen Aufrufer gestellt. Ohne Installation wird laut gefehlt,
  nicht auf Codepunkte zurückgefallen.
  ↳ **Preis, gemessen:** das WASM-Modul geht von 26,7 KB auf **1.154,3 KB**.
  `collation` ist wie `ts-export` ein standardmäßig AUSgeschaltetes Feature —
  `cargo tree -p adapter-vikunja -e normal` bestätigt, dass `icu_collator`
  im Adapter-Graphen nicht vorkommt (andere `icu_*` schon, die zieht
  `url`/`idna` seit jeher).
  ↳ **Beinahe-Unfall, der eine Prüflücke aufdeckte:** `pub mod collation;` stand
  ungeschützt in `lib.rs`, während seine Abhängigkeiten optional waren. Alles
  blieb grün, weil `cargo build --workspace` Features vereinheitlicht — die
  zwölf Adapter-Repos, die `cal-core` ALLEIN übersetzen, wären an
  `unresolved import icu_collator` gescheitert. Der Rust-Job prüft die
  Vertragskisten jetzt einzeln (`cargo check -p cal-core|plugin-core|plugin-sdk`),
  an genau diesem Fehler rot bewiesen.
  ↳ **Drei Wächter, jeder rot bewiesen:** die Regeln in `cal_core::collation`;
  die KETTE in `src/intl/collation.test.ts` (das Test-Setup installiert das
  echte WASM-Modul, also läuft dort der Kern — `numeric_ordering` in Rust
  abgeschaltet ⇒ „Übung 10" vor „Übung 2" im Desktop-Test); und die beiden
  Türen über den FFI-Wächter.
  ↳ **Wächter-Lücke gefunden und geschlossen:**
  `mobile/scripts/check-ffi-bridges.mjs` las nur `host.rs` und verfolgte nur
  `host.foo(…)`; `#[uniffi::export]`-**freie** Funktionen waren auf beiden
  Seiten unsichtbar (`parseAttendee` kreuzte seit jeher so). Er meldete GRÜN,
  während den eingecheckten Kotlin-Bindungen `compareNames`/`compareTitles`
  fehlten — das wäre erst als `:cal-ffi:compileReleaseKotlin`-Fehler minutentief
  in einem EAS-Bau aufgefallen. Erweitert und an genau diesem Fall rot
  bewiesen.
  ↳ **Zweite Wächter-Lücke (2026-09-14, #72):** Der Wächter folgte nur Aufrufen
  IN Rust hinein. Ob eine in `CalFfiModule.ts` erklärte Funktion in beiden
  nativen Modulen registriert ist, sah er nicht: Swift-`seriesShift` entfernt,
  und er meldete GRÜN. Auf dem iPhone wäre das erst beim Aufruf als „is not a
  function“ aufgefallen.

  Jetzt hält er jede erklärte Funktion an beide Module fest: Sie muss
  registriert sein, als dieselbe Art (`AsyncFunction` genau dann, wenn sie ein
  Promise liefert) und mit derselben Parameterzahl. Auskommentierte
  Registrierungen zählen nicht, und eine Erklärung, die er nicht lesen kann,
  nennt er mit ihrem Text. Die drei iOS-Funktionen stehen mit Grund in `ONLY_ON`.

  17 Proben: 13 Sabotagen rot und 4 frühere Fehlalarme grün.
  ↳ Offen bleibt: honoriert Hermes `{ numeric: true }`? Die Frage ist für die
  Zukunft entschärft (der Kern sortiert jetzt), aber falls Hermes es nicht tat,
  ändert sich mit dem nächsten Mobile-Build eine bestehende Reihenfolge sichtbar
  — das ist die Reparatur, nicht der Regress.
- [x] **Die zwei Verträge stehen fest** (2026-09-09), siehe DESIGN §4.5.
  (a) Der Kern antwortet mit einem **Schlüssel plus Variablen** oder einem
  Zustand, nie mit fertigem Text (Vorbild `ConferenceProvider::i18n_key`).
  (b) Der Kern liest **nie** die Uhr und **nie** die Gerätezone — Tagesschlüssel
  und Offset sind immer Parameter.
  ↳ **(b) ist ein Wächter**, `crates/cal-core/tests/core_contracts.rs`: er liest
  die Quellen von `cal-core` und `plugin-core` und **nennt** Datei und Zeile.
  Vier Sabotagen rot bewiesen (falsche Wurzel, kaputte Nadel, wieder eingebaute
  Uhr, eingebundene Lokalisierungs-Kiste). Keine Ausnahme für Tests: ein Test,
  der die Wanduhr liest, fällt an einem Tag im Jahr um.
  ↳ **(a) ist NICHT maschinell prüfbar, und der Wächter tut auch nicht so.**
  Ein Schlüssel und ein Satz sind beide `String`, und `cal_core::Error` trägt
  legitim englische Sätze, die über die Container-Fehleranzeige einen Menschen
  erreichen. Geprüft wird nur die Abhängigkeitskante: keine
  Lokalisierungs-Kiste in `cal-core`/`plugin-core`. Der Rest ist Review-Sache,
  mit `i18n_key` als Vorlage.
  ↳ **Gefunden dabei:** genau EINE Verletzung, seit jeher da. `DayLog::empty`
  stempelte `Utc::now()` auf einen Tag, an dem nichts angehakt war — ein
  Zeitstempel, den niemand gesetzt hat, an einer Zeile, die zurück in den
  Speicher wandern kann. Jetzt Parameter; die Uhr liest der lokale Adapter.
  Der TS-Zwilling `emptyDayLog` behält seinen Vorgabewert (Oberflächen-Code,
  vier Dialog-Aufrufer) — die Abweichung steht an der Funktion.
- [x] **Die Konferenz-Erkennung hat wieder eine Implementierung** (2026-09-09).
  `shared/conferencing.ts` waren 384 Zeilen Regel, eine zweite Fassung von
  `cal_core::conferencing`, das die ganze Zeit in Produktion lief. Die beiden
  nebeneinander zu lesen förderte **sechs** auseinandergelaufene Stellen zutage;
  drei davon wurden zugunsten der TypeScript-Antwort im Kern korrigiert (bei
  gleichwertigen DTMF-Kandidaten gewinnt wieder der erste, nicht der letzte;
  `sip:` wird EINMAL und ohne Rücksicht auf Groß-/Kleinschreibung abgeschnitten
  statt wiederholt; die längere URL wird nach Zeichen gemessen, nicht nach
  Bytes — ein Umlaut zählte doppelt). Die Datei ist jetzt eine Tür.
  ↳ **Zuerst festgenagelt, dann gelöscht.** Der Vertrag
  (`crates/cal-core/tests/fixtures/conferencing.json`) kam in einem eigenen
  Commit VOR dem Löschen und hält pro Zeile fest, was die verschwundene
  TypeScript-Seite antwortete und warum die überlebende Antwort gewählt wurde —
  damit das Geänderte lesbar ist statt archäologisch.
  ↳ `ConferenceDetail` trägt jetzt `{ label, value }` statt eines `(String,
  String)`-Paares, und `src/intl/conferencing.test.ts` prüft die KETTE: das
  Test-Setup installiert das echte WASM-Modul, jede Zeile kreuzt also dieselbe
  JSON-Grenze wie die laufende App.
  ↳ Nebenbei: `conferenceDetailRows` zeigt jetzt **beide** Quellen — die
  hergeleiteten Zeilen zuerst, dann die Einladungszeilen ohne Dubletten,
  entdoppelt auf dem getrimmten WERT statt auf der Beschriftung (Toni).
- [x] **Die Termingruppen-Formen werden erzeugt** (2026-09-09). `EventGroup` und
  `EventGroupMember` tragen den ts-rs-Derive; `shared/eventGroups.ts` fiel von
  125 auf 87 Zeilen, und `groupForEvent`/`otherMemberCount` gingen ganz — sie
  hatten keinen Aufrufer.
- [x] **Die Titel-Regel liegt im Kern** (Toni: „mach das.", 2026-09-10).
  „Meinen diese zwei Titel denselben Termin?" stand **fünfmal**: dreimal in
  `shared/` (byte für byte gleich, in `groupSuggestions.ts`,
  `suggestGroupMate.ts` und `healEventGroups.ts`) und zweimal in `host-core`
  (`event_anchor::plan_repairs` und die Ankerreparatur des
  Erinnerungs-Planers). Jetzt `cal_core::normalized_title` plus **eine**
  TypeScript-Hälfte in `shared/eventTitle.ts`.
  ↳ **Die zwei Sprachen waren sich uneinig**, und zwar genau um die inneren
  Leerzeichen: TypeScript faltete Läufe zusammen, Rust trimmte nur die Enden.
  Jede Kopie war für sich stimmig — sie schickt ja BEIDE Titel eines Vergleichs
  durch sich selbst —, also bekam kein einzelner Vergleich je zwei Antworten.
  Der Schaden war eine KETTE über die Sprachgrenze: TypeScript bietet eine
  Gruppe an, weil es die Titel als gleich liest; ändern sich später die inneren
  Abstände, findet Rusts Reparatur das Mitglied nicht wieder und es fällt
  **still** aus der Gruppe. Gewählt hat die großzügigere Lesart, absichtlich:
  sie irrt Richtung „Mitglied bleibt drin" statt Richtung „verschwindet
  wortlos".
  ↳ **Beide Hälften mussten ausgeschrieben werden, weil die eingebauten
  Funktionen sich nicht einig sind** — siehe DESIGN §4.5 (c). „Leerraum" ist in
  JavaScript und Rust nicht dieselbe Menge (`\s` zählt U+FEFF mit und U+0085
  nicht, `char::is_whitespace` genau andersherum), und Kleinschreibung **pro
  Zeichen** beantwortet das griechische Schluss-Sigma anders als
  Kleinschreibung der ganzen Zeichenkette. Der erste Entwurf der Rust-Hälfte
  tat Letzteres und hätte eine frische Uneinigkeit in genau dem Commit
  ausgeliefert, der die alte abschafft.
  ↳ **Der Wächter** ist `crates/cal-core/tests/fixtures/normalizedTitle.json`,
  gelesen von Rust (`include_str!` aus der eigenen Kiste) und von
  `src/intl/eventTitle.test.ts` — dieselbe Tabelle, zwei unabhängige
  Implementierungen. Drei Sabotagen rot bewiesen: die alte trimm-nur-Fassung in
  Rust, die alte naive Fassung in TypeScript (fiel über NEL UND die BOM), und
  Kleinschreibung pro Zeichen (fiel über das Sigma).
  ↳ **Keine Tür, und das mit Absicht.** Die drei Aufrufer ziehen selbst noch in
  den Kern und nehmen die TypeScript-Hälfte dann mit; eine Tür nur für den
  Zwischenschritt wäre beim nächsten wieder wegzuwerfen.
- [x] **Die Anker-Entscheidung liegt im Kern** (Toni: „du kannst sie
  herunterziehen, ja", 2026-09-10), Teil 1 von 3.
  ↳ „Welche Zeile, die einen fremden Termin benennt, findet ihn wieder — und
  was tut man dagegen?" lag in `host-core`. Jetzt `cal_core::event_anchor` mit
  `Anchored`, `Repair`, `plan_repairs` und `series_master_id`. Der Umzug ist
  vertragsrein: keine Uhr, keine Gerätezone (`range` ist ein Parameter bis
  hinauf zum Tauri-Befehl und zum UniFFI-DTO), kein Menschentext, und **keine
  neue Abhängigkeit** in `cal-core` — chrono war längst da.
  ↳ **Es war eine SECHSTE Kopie, nicht die fünfte.**
  `reminders::heal_local_reminders_for_calendar` schrieb dieselbe Entscheidung
  noch einmal von Hand aus. Sie geht jetzt durch `plan_repairs`; die drei
  Unterschiede habe ich einzeln geprüft, zwei sind folgenlos (Kalender-Klausel
  im Refresh greift nie, weil vorgefiltert wird; der Bereichs-Vorabbruch ist
  eine Abkürzung, kein Sicherheitsnetz — ein Kandidat muss den Start ohnehin
  exakt treffen).
  ↳ **Der dritte war ein echter Fehler.** Die Handfassung nahm den ERSTEN
  Termin im Stapel, dessen Serien-Id passte. Schickt der Anbieter die Ausnahme
  eines Vorkommens vor ihrem Master, wurde die Zeile mit dem Start des
  VORKOMMENS gestempelt — obwohl sie auf die Serie lautet. Der nächste Scan
  sucht dann einen Master, der dann beginnt, findet nichts, und die Erinnerung
  verstummt endgültig. Rot bewiesen (`a_series_row_is_refreshed_from_the_master_
  not_an_override`, Sabotage: die alte „erster Treffer gewinnt"-Karte).
  ↳ **Ein Schreibvorgang, den niemand wollte, nebenbei abgestellt.** Refresh
  verglich Startzeiten als ZEICHENKETTE, und derselbe Zeitpunkt hat in diesem
  Code drei Schreibweisen: `to_rfc3339()` sagt `+00:00`, serde sagt `Z`, ein
  Frontend sagt `.000Z`. Eine Signatur schreibt aber, wer die Zeile zuletzt
  angefasst hat — der Desktop schickt `Event.start` über serde nach vorn und
  bekommt die `Z`-Form zurückgeschrieben. Jede so geschriebene Zeile las sich
  beim ersten Scan als veraltet und wurde einmal grundlos neu geschrieben, auf
  allen drei Tabellen. Jetzt wird der ZEITPUNKT verglichen; eine unlesbare
  Signatur zählt weiter als abweichend, weil eine lesbare darüber die Reparatur
  ist. Rot bewiesen mit vier Schreibweisen desselben Moments.
  ↳ **Ein Doc-Kommentar korrigiert**, weil er in die Irre führt: an
  `series_master_id` stand „Mirrors the frontend's `seriesIdOf`". Tut es nicht.
  `seriesIdOf` beantwortet ZWEI Fälle, `series_master_id` nur einen — die
  aufgefaltete Wiederholung trägt ein `series_id`-Feld, das `cal_core::Event`
  gar nicht hat, weil Kern und Host nie auffalten. Der Unterschied ist tragend
  und jetzt ausgeschrieben plus mit einem eigenen Test festgenagelt.
- [x] **Anker Teil 2: die Ganztags-Regel liegt jetzt auch im Kern** (2026-09-10).
  `plan_repairs` verglich Startzeiten immer als Zeitpunkt. Ein GANZTÄGIGER
  Termin beginnt aber um LOKALE Mitternacht, ausgedrückt als UTC-Zeitpunkt
  (`adapter-caldav`s Mapping sagt es im Kopf: „DTSTART VALUE=DATE ->
  all_day = true, LOCAL midnight as a UTC instant") — derselbe Geburtstag ist also diesseits und jenseits einer
  Zeitumstellung ein anderer Zeitpunkt, und nach einem Umzug in eine andere
  Zone erst recht. Als Zeitpunkt verglichen hörte so eine Zeile beim nächsten
  Neu-Vergeben der Id einfach auf, auffindbar zu sein — still, also genau das
  Versagen, gegen das die Signatur existiert. Ein ganztägiger Kandidat
  antwortet jetzt auf den TAG, ein getakteter weiter auf den Zeitpunkt.
  ↳ Portiert aus `shared/healEventGroups.ts`, wo die Regel als einzige der drei
  Anker-Anwendungen schon stand — die drei Rust-Tabellen (Farbe, Meeting,
  private Erinnerung) hatten sie NIE. Zwei mitgeschleppte Eigenheiten stehen
  jetzt ausgeschrieben statt als Zufall da: der Kandidat entscheidet für
  BEIDE Seiten (eine Signatur kann nicht sagen, ob SIE ganztägig war), und der
  Tag ist der UTC-Tag, also nicht immer der, den der Nutzer sieht — er taugt
  als SCHLÜSSEL, weil beide Seiten ihn gleich ableiten, und den echten
  Kalendertag auszurechnen bräuchte die Gerätezone, die der Kern nie lesen darf.
  ↳ Beide Richtungen rot bewiesen: ohne die Regel (der Zustand davor) findet
  die ganztägige Kopie sich nicht wieder; mit der Regel auf GETAKTETE Termine
  angewandt wird ein Termin eine Stunde später fälschlich derselbe.
- [x] **Anker Teil 3: die Gruppen werden im HOST geheilt** (Toni entschieden,
  2026-09-10: „Im Host, wie die drei Schwestern"). Damit ist der Umzug fertig.
  ↳ `host_core::event_groups::heal_event_group_anchors` steht jetzt neben
  `heal_event_color_anchors` und `heal_event_meeting_anchors` und wird aus
  denselben zwei Stellen gerufen (`src-tauri/.../calendars.rs` und
  `cal-ffi/src/host.rs`), wo die Termine des Kalenders ohnehin in der Hand
  sind. Der Modulkopf von `event_anchor` nannte `event_groups` seit jeher als
  vierte Tabelle dieser Art — sie war die einzige, die nicht über
  `plan_repairs` ging.
  ↳ **Keine Tür gebaut, keine gebraucht.** Alle drei Aufrufer waren schon
  asynchron. Und sie machten je EINE Host-Runde pro Befund — eine zum
  Auffrischen jeder veralteten Signatur, eine zum Umhängen jedes Mitglieds,
  danach noch eine zum Zurücklesen. Jetzt keine.
  ↳ **Was gelöscht ist:** `shared/healEventGroups.ts` (187 Zeilen Regel),
  `src/state/healEventGroups.test.ts`, die zwei Tauri-Befehle, die zwei
  UniFFI-Methoden und ihre vier API-Hüllen. Der FFI-Wächter zählt jetzt 153
  Brücken-Aufrufe statt 155 und hat die neu erzeugten Kotlin-Bindungen
  angenommen.
  ↳ **Der Kollisionsschutz ist mitgekommen**, sonst wäre er mit der Datei
  verschwunden. `EventGroupsRepo::heal_member` hatte keinen, und der
  UNIQUE-Index `event_group_members(calendar_id, event_id)` sagt „ein Termin
  gehört zu höchstens einer Gruppe" — trägt eine Gruppe die veraltete UND die
  schon geheilte Id, lief das UPDATE in den Index. Rot bewiesen, mit genau dem
  Constraint-Fehler. Der Schutz fragt jetzt tabellenweit, nicht nur innerhalb
  der Gruppe: der Index ist global, also kollidiert ein Mitglied einer ANDEREN
  Gruppe genauso hart, was die TypeScript-Fassung nicht abgedeckt hatte.
  ↳ **Bewusst NICHT übernommen:** `findStaleSignatures` verglich Titel
  normalisiert, `Repair::Refresh` vergleicht sie roh. Roh ist hier richtig —
  die Signatur soll buchstäblich beschreiben, was dasteht; gesucht wird ohnehin
  normalisiert. Und die calendar_id-Hälfte von `Refresh` läuft bei Gruppen ins
  Leere, weil `refresh_signature` nur Titel und Start schreibt; das steht an
  der Aufrufstelle, damit es nicht als Versehen gelesen wird.
  ↳ **Was sich sichtbar ändert:** die Signatur eines wiederkehrenden Mitglieds
  wandert vom Start des VORKOMMENS auf den der SERIE. `memberFromEvent`
  speichert die Serien-Id zusammen mit dem Start des Vorkommens, das der Nutzer
  offen hatte; das Mitglied ist auf die Serie gebucht, also beschreibt es
  künftig die Serie — dieselbe Entscheidung wie bei den privaten Erinnerungen.
- [x] **Das Erkennen einer Kopie liegt im Kern** (2026-09-10).
  `shared/groupSuggestions.ts` und `shared/suggestGroupMate.ts` waren zwei
  TypeScript-Fassungen einer Entscheidung, die der Kern auch treffen muss.
  Jetzt `cal_core::group_suggestion` mit `find_group_suggestions`,
  `suggest_group_mate` und `is_meeting_calendar`; die zwei Dateien sind zu
  EINER Tür geworden, `suggestGroupMate.ts` ist weg.
  ↳ **Zwei Türen, synchron, und das ist der Punkt.** Beide Aufrufer fragen in
  einem `useMemo` — die Vorschlags-Notiz und der Gruppierungs-Dialog, auf
  beiden Oberflächen. Da kann nichts warten, also `Function` statt
  `AsyncFunction`, wie bei der Kollation. Der FFI-Wächter meldete von sich aus
  rot, kaum standen die zwei freien Funktionen da, und nennt jetzt elf.
  ↳ **Die Signaturen sind unverändert.** Die Tür baut die Anfrage und bildet
  die Antwort auf die Zeilen des Aufrufers zurück, also musste keine
  Aufrufstelle etwas über JSON oder über Positionen lernen. Die Antwort sind
  POSITIONEN in der Eingabe: der Aufrufer hält die Termine ohnehin, und sie
  zurückzuschicken hätte die Nutzlast verdoppelt, um nichts Neues zu sagen.
  ↳ **Die Serien-Id reist als DATUM mit**, nicht als Rückruf. Der Kern kann sie
  nicht ausrechnen: eine aufgefaltete Wiederholung weiß als einzige, zu welcher
  Serie sie gehört, und `series_master_id` beantwortet nur den Override-Fall.
  Statt zu raten, fragt die Regel nach der Antwort.
  ↳ **Noch eine Doppelung mitgelöst:** das Suffix `::meetings` stand als
  Konstante in `host_core::vc_calendar` UND als Literal in
  `shared/meetingEvents.ts`. Es liegt jetzt einmal im Kern — zwei Leser, die
  es verschieden verstehen, hießen: eine Zeile, die der eine wegwirft und der
  andere nie paart.
  ↳ **Ein Fall, den TypeScript nie hatte:** ein Beginn, aus dem sich nichts
  machen lässt. Der alte Schlüssel war `new Date(wert).toISOString()`, was bei
  unlesbarer Eingabe WIRFT. Im Kern passt so ein Beginn jetzt auf NICHTS,
  auch nicht auf einen zweiten unlesbaren — zwei Zeilen, die Aperio nicht
  einordnen kann, sind kein Beleg dafür, dass sie dasselbe meinen.
  ↳ **Die zwei Testdateien blieben und prüfen jetzt die KETTE**, nicht mehr
  eine zweite Implementierung: `src/test-setup.ts` installiert das echte
  WASM-Modul. Rot bewiesen, indem ich die Kalender-Bedingung im RUST-Kern
  gestrichen habe — der Desktop-Test fiel um.
  ↳ **Zwei Zwillinge bleiben vorerst**, benannt statt stillschweigend:
  `isDeclineInForce` und `suggestionPairKey`. Ihr letzter Aufrufer ist
  `shared/meetingLinkGrouping.ts`, das synchron ist und noch nicht umgezogen
  ist; sie gehen mit ihm.
- [x] **Der Meeting-Dubletten-Filter liegt im Kern** (2026-09-10).
  `withoutDuplicateMeetings` entschied in `shared/meetingEvents.ts`, welche
  Zeilen eines Fensters übrig bleiben — eine Regel über eine MENGE, die eine
  reMarkable-Oberfläche sonst neu schreiben müsste. Jetzt
  `cal_core::meeting_events`.
  ↳ **Das ganze Fenster kreuzt auf einmal.** Vorher las der Filter den Link
  jeder Zeile durch die Erkennungs-Tür — einmal beim Sammeln, einmal beim
  Filtern. Eine Tagesansicht mit vierzig Zeilen kreuzte die Grenze achtzigmal,
  um eine Frage über vierzig Zeilen zu beantworten. Jetzt einmal, mit denselben
  Bytes.
  ↳ **Ein Test hat eine Eigenschaft gerettet, an die ich nicht gedacht hatte:**
  die alte Fassung gab DASSELBE Array zurück, wenn nichts zu tun war, und
  `toBe` hielt das fest. Die Aufrufer stecken das Ergebnis in ein `useMemo` —
  ein frisches Array bei jedem Durchlauf hätte dessen Identität für alles
  Nachgelagerte wertlos gemacht. Wiederhergestellt, und etwas großzügiger:
  dasselbe Array, wann immer nichts wegfällt.
  ↳ **Eine Lücke im Kettentest, aufgedeckt durch die Sabotage:** die
  Gruppen-Ausnahme (`isGrouped`) kam in keinem Desktop-Fall vor — der Kern
  wurde rot, die Kette blieb grün. Fall nachgetragen und selbst rot bewiesen.
  ↳ **Und eine Falle beim Prüfen:** `npx vitest run` benutzt das ZULETZT
  gebaute WASM-Modul. Wer Rust ändert und direkt vitest ruft, misst ein
  veraltetes Modul. `npm test` baut vorher — der Direktaufruf nicht.
  ↳ `isMeetingCalendarEvent` bleibt als benannter Zwilling: für einen
  Suffix-Test pro Zeile im Render eine Tür zu kreuzen wäre das Gegenteil
  dessen, was der Umzug gerade gebracht hat. Geht mit
  `meetingLinkGrouping.ts`.
- [x] **Die Meeting-Verknüpfung liegt im Kern** (2026-09-10), inklusive der
  URL-Faltung — Toni: „ich habe kein problem damit, wenn du hierfür ein
  url-crate importierst".
  ↳ `shared/meetingLinkGrouping.ts` waren 258 Zeilen sehr dicht begründeter
  Regel (Serien zählen einmal, Gruppen zählen einmal, ein Meeting pro Konto pro
  Termin, was eine Ablehnung abdeckt und was sie nicht überlebt). Jetzt
  `cal_core::meeting_link_grouping`; die Datei ist eine Tür von 144 Zeilen.
  ↳ **Erst gemessen, dann entschieden.** Ich wollte die Faltung draußen lassen,
  weil `cal-core` bewusst keinen URL-Parser hat. Also ließ ich die HEUTIGE
  JavaScript-Fassung über eine Tabelle echter Beitrittslinks laufen, statt zu
  raten — und sie tut drei Dinge, die eine Nachbildung nicht kann: `:443` fällt
  weg, `/a/./b/../c` wird zu `/a/c`, und `münchen.example.com` wird zu
  `xn--mnchen-3ya.example.com`. Toni hat das Crate freigegeben, also zog die
  Faltung mit.
  ↳ **Die gemessene Tabelle ist jetzt der Wächter**
  (`crates/cal-core/tests/fixtures/normalizeJoinUrl.json`): 21 Zeilen, deren
  Erwartungen aus der abgelösten Implementierung STAMMEN statt gewählt zu sein.
  Alle 21 stimmen.
  ↳ **Preis, ehrlich gemessen:** das WASM-Modul geht von 1.362.793 auf
  1.537.283 Bytes, also +170 KB (+12,8 %). Die erste Messung sagte +166 Bytes
  und war wertlos — die Tür fehlte noch, also warf der Linker den Code weg.
  ↳ **Ein Test, der eine falsche Eigenschaft behauptete, umgeschrieben statt
  grün gebogen.** Volle Idempotenz gilt NICHT: `mailto:someone@example.com`
  faltet zu `mailto://someone@example.com`, und ein zweiter Durchlauf liest
  `someone` als Benutzerinfo und wirft es weg. Die TypeScript-Fassung tat
  dasselbe. Gebraucht wird nur, dass ein Bucket-Schlüssel stabil ist — und der
  Detektor liefert ohnehin nur http(s). Genau das prüft der Test jetzt.
  ↳ **Drei Zwillinge sind damit frei geworden und gelöscht:**
  `isDeclineInForce`, `suggestionPairKey` und `meetingJoinUrl`.
  `isMeetingCalendarEvent` bleibt — `collapseEventGroups.ts` braucht es noch.
  ↳ Die 23 bestehenden Testfälle blieben und prüfen jetzt die KETTE. Rot
  bewiesen, indem ich im RUST-Kern die Regel strich, die ein bereits
  gruppiertes Meeting in Ruhe lässt.
- [x] **Die Faltung liegt im Kern** (2026-09-10). `collapseEventGroups` — was
  eine Tagesansicht überhaupt zeigt, wenn eine Gruppe im Spiel ist — ist
  `cal_core::event_group_fold`. Sieben Aufrufstellen, beide Oberflächen plus der
  Widget-Schnappschuss; keine musste geändert werden.
  ↳ **`groupBadge` bleibt vorn.** „3× ≠" ist Text fürs Auge, und der Kern
  antwortet mit einem Zustand, nie mit fertigem Text (DESIGN §4.5 (a)). Der Kern
  liefert `diverged` und `other_members`; die Marke baut die Oberfläche.
  ↳ **Der `actionable`-Haken hat den Umzug überlebt**, obwohl ihn keine
  Produktions-Aufrufstelle setzt — ein Test tut es. Ihn stillschweigend zu
  streichen hätte eine geprüfte Fähigkeit entfernt. Er reist jetzt als
  `Option<bool>` mit: fehlt er, antwortet der KERN (er erkennt die
  Meetings-Kalender selbst), was jede Aufrufstelle will.
  ↳ **Damit ist der letzte Zwilling weg.** `isMeetingCalendarEvent` hatte nur
  noch die Faltung als Aufrufer; jetzt kennt allein
  `cal_core::MEETINGS_CALENDAR_SUFFIX` das Suffix.
  ↳ Die 13 bestehenden Testfälle blieben und prüfen die KETTE — dazu die 31
  Widget-Schnappschuss-Fälle, die durch dieselbe Tür gehen. Rot bewiesen, indem
  ich im RUST-Kern die Startzeiten wieder als TEXT vergleichen liess: genau der
  Fehler, wegen dessen eine Serie früher dauerhaft „auseinandergelaufen" hiess.
  ↳ **Nebenbei geprüft, statt angenommen:** der Widget- und Hintergrund-Pfad
  hängt schon heute an einer installierten Tür (`buildWidgetSnapshot` ruft
  `compareTitles`), eine weitere fügt dort also keine neue
  Abhängigkeitsklasse hinzu.
- [x] **Der Übertrag liegt im Kern** (2026-09-10), in zwei Schritten — erst
  festnageln, dann umziehen, wie bei der Konferenzerkennung.
  ↳ **Schritt 1 (PR #35): neun Testfälle für `futureCarryRow`**, das als
  einzige Funktion im Modul gar keinen hatte — ausgerechnet die, deren Doc den
  Fehler beschreibt, gegen den sie geschrieben wurde. Kein Produktionscode
  angefasst; jede Erwartung GEMESSEN, nicht gewählt.
  ↳ **Der Fund, der den Umzug gerettet hat:** ganztägige Kopien schieben in
  ganzen Tagen, also muss eine Halbtags-Verschiebung gerundet werden.
  JavaScripts `Math.round` bricht den Gleichstand Richtung PLUS UNENDLICH
  (+12 h = +1 Tag, −12 h = 0 Tage, −36 h = −1 Tag); Rusts `f64::round` bricht
  ihn VON DER NULL WEG. Eine geradlinige Portierung hätte jede exakte
  Halbtags-Rückverschiebung einen Tag zu weit geschoben, lautlos. Im Kern steht
  dafür `round_half_up`, und der Gleichstand ist auf beiden Seiten festgenagelt
  und rot bewiesen.
  ↳ **Schritt 2 (dieser PR): `cal_core::group_carry`.** Der Kern antwortet mit
  FELDWERTEN, nie mit Zeilen — die Zeile einer Kopie trägt weit mehr als die
  sechs Felder, und was der Kern nicht kennt, kann er auch nicht
  überschreiben. Die Tür legt die Antwort über das, was der Aufrufer schon
  hält.
  ↳ **Der Wurf ist ENTSCHIEDEN worden statt geerbt.** Ein unlesbarer
  Schnittpunkt ließ `new Date(NaN).toISOString()` werfen, mitten in der
  Schleife — eine kaputte Zeile brach den ganzen Übertrag ab. Rust kann nicht
  werfen, also ist die Antwort `None`, und die zwei Aufrufer melden die Kopie
  über ihre `failed`-Liste. Genau das Muster stand dort schon für Kopien, die
  nach dem Schnitt nichts mehr haben.
  ↳ `worthCarrying` ist als Feld in den Plan gewandert statt als eigene Tür:
  zwei Vergleiche, für die eine Überfahrt albern wäre — aber sie draußen
  auszuschreiben wäre ein Zwilling gewesen.
  ↳ `CarryScope` bleibt vorn: der Aufrufer entscheidet damit, WELCHE Regel er
  fragt, das ist keine Regel.
- [x] **Tote FFI-Fläche abgetragen** (2026-09-10/11), bevor der nächste Bogen
  auf ihr aufsetzt. `LocalStore` (PR #38): 22 Methoden, drei DTOs, sieben
  Wert-Typen, zwölf Tests — elf davon Zeile für Zeile Doppelungen der Tests in
  `adapter-local` und am Host; die eine Behauptung, die nur er machte
  (kaputtes Aufgaben-JSON wird ein getippter Fehler, kein Absturz), lebt jetzt
  am Host. Die eingecheckten iOS-Artefakte (PR #39): `cal_ffi.swift` lag 40
  Symbole hinter dem Rust, und nichts merkte es, weil kein Bau sie las und der
  Wächter nur Rust und Kotlin liest. Jetzt gitignored; der eas-Job streift die
  Regel vor dem Archiv ab und schaut INS Archiv, bevor er zahlt — das
  Android-Muster, denn eas-cli liest nie den Index, sondern den Arbeitsbaum
  durch `.gitignore`. Das rrule-Paar (PR #40): exportiert, deklariert, von
  keiner Brücke gerufen; der Wächter duldet das mit Absicht.
- [~] **Die Aufgaben-Gruppierung zieht in den Kern** (Tonis Wahl 2026-09-10),
  in denselben zwei Schritten wie der Übertrag.
  ↳ **Schritt 1 (PR #41): die Tabelle.** `buildEntries` (611 Zeilen, auf
  BEIDEN Oberflächen aus `useMemo` gerufen — die Modul-Suche zeigte nur den
  Desktop, weil Mobile über das Barrel importiert) hatte 41 Desktop-Tests und
  keine Fixture, die Rust lesen könnte. Jetzt steht
  `crates/cal-core/tests/fixtures/taskGrouping.json`: 46 Fälle, GEMESSEN durch
  Ausführen des heutigen TypeScripts, dazu die Zeilen, die die 41 Tests nie
  hatten (Zwei-Stufen-Skala, Listenordnung nach Name, ein eingeklappter Task,
  eine eingeklappte Liste, eine Liste ohne Namen, „alles auf einmal").
  `taskGrouping.contract.test.ts` spielt sie zurück; rot bewiesen, indem
  „heute" für einen Lauf zu „überfällig" gemacht wurde.
  ↳ **Was die Fixture festnagelt, ist die ENTSCHEIDUNG, nicht die Wortwahl:**
  eine Kopfzeile trägt Art, Zähler und die Ids, auf die sie zeigt — nie den
  Titel. „Erledigt (3)" baut der Aufrufer; `mine`/`others` erscheinen nur beim
  Split. Genau der Schnitt, den der Kern braucht, weil er `t` nicht halten
  kann (DESIGN §4.5 a).
  ↳ **Die synthetischen Kopfzeilen-Ids sind Teil des Vertrags:**
  `grp:bl:list:L1`, `grp:sec:bl:L1:s1`, `__aperio_done_group__` … sind die
  Klapp-Schlüssel, die beide Oberflächen persistieren. Ein Umzug, der sie
  ändert, klappt beim Nutzer still alles wieder auf.
  ↳ **Schritt 2 (dieser PR): `cal_core::task_grouping`.** Antwortet mit dem Wald aus
  Schlüsseln, Zählern und Positionen; die Türen sind synchron (WASM,
  `Function`); die Hülle in `shared/taskGrouping.ts` hydratisiert die Zeilen
  aus dem, was sie hält, und wortet die Köpfe mit `t`. Die Portierung bestand
  alle 46 Fälle der Fixture beim ERSTEN Lauf — das ist, was die Tabelle
  vorher wert war. Die Sprache reist im Eingang mit (`languageTag()` am
  Installationsort, pro Aufruf gelesen); der Kern liest keine Gerätesprache.
  ↳ **Ein Unterschied mit Absicht, von drei Gegenlesern gefunden:** Erledigt und
  Abgebrochen ordnen nach dem ZEITPUNKT, nicht nach der Schreibweise des
  Zeitstempels. Das TypeScript verglich RFC-3339-Text; bei zwei Stempeln in
  derselben Sekunde (lokaler Adapter mit Bruchteil, Provider mit ganzen
  Sekunden) stand `…00Z` hinter `…00.250Z` — der frühere Zeitpunkt zuerst. Ein
  Artefakt, keine Entscheidung; die Fixture pinnt die Zeitpunkt-Ordnung als
  eigenen Fall mit `wasTypeScript`-Vermerk. Dazu ein Anti-Stille-Wächter:
  `cargo test -p cal-core` OHNE das Feature meldet jetzt laut, dass der
  Vertrag nicht lief, statt grün zu schweigen.
  ↳ **Die Wire-Typen sind ERZEUGT, nicht gespiegelt:** `GroupingInput`,
  `GroupableTask`, `GroupingRow`, `GroupHead`, `GroupKind`, `TaskGroupBy`
  kommen per `cargo xtask ts-types` aus Rust und werden von `shared/types.ts`
  exportiert; die Hülle importiert sie. Dafür liegen die Typen AUSSERHALB des
  `collation`-Gates (der xtask baut nur mit `ts-export`), gegatet ist nur die
  Gruppierung selbst.
- [x] **Nachzug für die älteren Türen (PR #43):** sechs Türen — Übertrag,
  Faltung, Vorschläge, Meeting-Filter, Meeting-Link-Paarung, Konferenz —
  bauten ihre Anfragen und parsten ihre Antworten mit handgeschriebenen
  Formen, während `shared/generated/CarryPlan.ts` & Co. ungenutzt daneben
  lagen; der `ts-types --check` prüfte dort ins Leere. Jetzt ist alles, was
  über die Leitung geht, mit dem erzeugten Typ typisiert. Wo der Name von
  der hydratisierten Hüllen-Form (`CollapsedRow<E>`, `GroupSuggestion<E>`,
  `MeetingLinkPair<E>`) oder vom Aufrufer-Minimum (`SuggestibleEvent`,
  `LinkableEvent`) belegt ist, trägt der Wire-Typ den Alias `…Wire`; kein
  öffentlicher Name bewegt sich. Reine Typänderung.
  Nebenbefund für danach: `useTasks.ts` hat auf Desktop
  UND Mobile je eine identische lokale `taskOrder` (Datums-Eimer → Datum →
  Erstellzeit, eine ANDERE Ordnung als die geteilte) — ein zweiter Zwilling.
- [~] **Die Kalendertag-Regeln ziehen in den Kern** (`taskDay`), in denselben
  zwei Schritten.
  ↳ **Schritt 1 (PR #44): die Tabelle.** `filterTasksOnDay` & Co. (362
  Zeilen, auf beiden Oberflächen aus `useMemo` gerufen) hatten 28 + 9 Tests
  und keine Fixture. Jetzt steht `crates/cal-core/tests/fixtures/taskDay.json`:
  24 Tages-, 7 Wochen- und 4 Aufteilungs-Fälle, GEMESSEN am heutigen
  TypeScript, dazu die Zeilen, die die Tests nie hatten — der
  Eigentümer-Filter `meFor` hatte GAR KEINEN Test, die Zwei-Stufen-Skala,
  mehrere Tage in einem Aufruf, ein Erledigt-Tag auf einer Liste, die
  Erledigtes verbirgt, eine Endzeit, eine Samstags-Woche.
  `taskDay.contract.test.ts` spielt zurück; rot bewiesen (geplanter Termin
  auch am Fälligkeitstag: 2 von 36 fallen).
  ↳ **Drei Dinge bleiben mit Absicht draußen,** die Fixture sagt es unter
  `notInThisTable`: `todayIsoKey` liest die Uhr; der LOKALE Erledigt-Tag wird
  vom Aufrufer aufgelöst (Gerätezone) und reist als `completed_day` — der Kern
  liest keine Zone (DESIGN §4.5 b); `mergeDayItems` baut Sortierschlüssel über
  das lokale `Date` gegen Epoch-Zeiten des Aufrufers — Darstellung.
  ↳ **Schritt 2 (dieser PR): `cal_core::task_day`.** Antwortet pro Tag mit
  Zeilen {task (Position), time, endTime, deadlineChip} — die vier Fragen
  einer Kachel in EINER Überfahrt pro Tag statt einem Aufruf pro Kachel; die
  Wochen-Grenzen und die Fälligkeits-Aufteilung gleich mit. Die zwei
  Rückrufe wertet die Hülle einmal pro Liste aus und schickt sie als Daten
  (`completedVisible`, `currentUserByList`); kein Aufrufer hat sich geändert.
  `task_order` ist jetzt feldbasiert und `pub(crate)`, von Gruppierung UND
  Kalendertag gefragt; der TypeScript-`taskOrder` ist weg, die zwei Tests, die
  ihn direkt riefen, beobachten die Ordnung durch `filterTasksOnDay`. Die
  drei Kachel-Helfer (`taskTimeOnDay`, `taskEndTimeOnDay`, `isDeadlineChip`)
  bleiben vorerst als TypeScript-Zwillinge der Antwort-Felder, von derselben
  Fixture auf beiden Seiten gepinnt — die Views rufen sie pro Kachel mit
  (task, day); auf die Antwort-Zeilen umstellen ist ein eigener Schritt.
  Wire-Typen erzeugt (7), Türen synchron auf beiden Oberflächen.
  ↳ **Von drei Gegenlesern gefunden, auf beiden Seiten behoben:** `parent_id: ""`
  hieß im TypeScript „kein Elternteil" (Wahrheitswert-Test), im Rust „Unter-
  aufgabe" (`is_some`) — eine undatierte Aufgabe mit leerer Eltern-Id
  verschwand vom Tag. Kein Erzeuger im Repo schreibt sie; ein Differenz-Lauf
  über Zufallszeilen traf sie neunmal in dreitausend. Der Kern liest die leere
  Id jetzt als kein Elternteil, die Hülle schickt `null`, die Fixture pinnt den
  Fall. Dazu: die Hülle bleibt TOTAL (leere Zeichenkette → `null` in fünf
  Datums-/Zeitfeldern, `weekStartsOn` geklemmt), statt in einem `useMemo` die
  ganze Ansicht zu werfen; und drei Aufrufer, die pro Tag eine Überfahrt
  machten (Monatsraster: 42-mal, jede mit ALLEN Aufgaben serialisiert; mobile
  Tagesliste über die native Brücke; Widget-Schnappschuss), fragen jetzt einmal
  mit allen Tagesschlüsseln (`groupTasksByDay`).
  ↳ **Schritt 3 (Folge-PR): die Views lesen die Antwort-Zeilen.**
  `filterTasksOnDay`/`groupTasksByDay` geben `DayTaskEntry {task, time,
  endTime, deadlineChip}` zurück — die Kern-Zeile über das eigene
  Aufgaben-Objekt gelegt — und `mergeDayItems` trägt den Eintrag an jedem
  Zeit-Item mit. Die drei TypeScript-Zwillinge (`taskTimeOnDay`,
  `taskEndTimeOnDay`, `isDeadlineChip`) sind gelöscht; Tages-, Wochen- und
  Monatsansicht, die mobile Tagesliste und der Widget-Schnappschuss lesen
  Zeit, Blockende und Frist-Marker aus dem Eintrag statt sie pro Kachel neu
  abzuleiten. Der Vertragstest spielt die Kachel-Fakten jetzt DURCH DIE TÜR
  zurück (vorher aus den Zwillingen); die Fixture ist unverändert. Die
  Unit-Tests der Zwillinge sind zu Tests über den Eintrag geworden — drei
  davon fragten nach einem Tag, an dem die Aufgabe gar nicht liegt, und
  prüfen jetzt genau das: kein Eintrag. Kein Verhalten hat sich geändert.
- [~] **Der Aufgaben-Zustand zieht in den Kern** (`taskStatus`), in denselben
  zwei Schritten — mit dem Schnitt vom 2026-09-09: Zustand rein, Zeichen vorn.
  ↳ **Schritt 1 (dieser PR): die Tabelle.** Von den 18 Exporten des Moduls
  sind drei schon Türen (`priorityRank`, `isImportantPriority`,
  `normalPriority`), zwei sind Glyphen (bleiben), fünf sind Wortwahl über `t`
  (bleiben), eines ein CSS-Token, eines ein Nachschlagen. Was umzieht: die drei
  SCHLÜSSEL-Abbildungen (`statusI18nKey`, `effortI18nKey`, `priorityI18nKey`)
  und `subtaskProgress`. `crates/cal-core/tests/fixtures/taskStatus.json` hält
  die Schlüssel als EINE Tabelle (jeder Zustand, jeder Aufwand, jede Priorität
  in beiden Skalen) und sechs Fortschritts-Fälle, gemessen am heutigen
  TypeScript; `taskStatus.contract.test.ts` spielt zurück.
  ↳ **Warum eine Tabelle und keine Tür pro Aufruf:** die Schlüssel werden pro
  Kachel gefragt, auf dem Telefon über die native Brücke — der Kern
  VERÖFFENTLICHT die Tabelle einmal beim Installieren, die Oberfläche schlägt
  nach (Muster `ConferenceProvider::i18n_key`, DESIGN §4.5 a). Der Fortschritt
  wird für alle Eltern einer Liste in EINER Überfahrt beantwortet; die Hülle
  merkt sich die Antwort pro Aufgaben-Array (WeakMap), weil die Views pro Zeile
  fragen.
  ↳ **Schritt 2 (Folge-PR auf #47): gebaut.** `cal_core::task_status` — ohne
  Feature-Gate, nichts darin kollationiert — veröffentlicht die Tabelle
  (`task_i18n_keys_json`) und zählt den Fortschritt (`subtask_progress_json`,
  Eltern-Id → `{done, total}` für jede Elternaufgabe mit zählenden Kindern).
  Zwei Türen auf beiden Oberflächen (`taskI18nKeys` ohne Eingabe,
  `subtaskProgress`), acht Wire-Typen erzeugt. Die Hülle liest die Tabelle
  beim ERSTEN Nachschlagen und behält sie; `subtaskProgress` schickt pro
  Aufgaben-Array einmal drei Felder je Zeile (`id`, `status`, `parent_id`)
  und merkt sich die Antwort in einer WeakMap — die Views fragen pro Zeile
  mit derselben Liste. Die drei Schlüssel-Zwillinge und die Zählschleife sind
  weg; der Rust-Vertragstest liest dieselbe Fixture, rot bewiesen (Regel
  „abgebrochen zählt nicht" entfernt → ein Fall fällt). Die leere Eltern-Id
  bleibt hier absichtlich, was sie im TypeScript war — eine Id wie jede
  andere, nach der niemand fragt; der Kern zählt nach Gleichheit, ohne
  Wahrheitswert-Test, also gibt es hier keine #46-Lücke.
  ↳ **Von zwei der drei Gegenleser gefunden, gegen das echte WASM belegt:** die
  Hülle las die Antwort als nacktes Objekt (`byParent[parentId]`), und eine
  kinderlose Elternaufgabe namens `constructor`, `toString` oder `__proto__`
  fand `Object.prototype` statt `null` — Ids sind Freitext (eine CalDAV-UID ist
  die Id), und der Screenreader hätte einen Fortschritt mit unaufgelöstem
  `{{done}}` gehört. Die Antwort liegt jetzt in einer `Map`; die Fixture pinnt
  drei der Namen, `__proto__` pinnt ein Unit-Test (als JSON-Literal-Schlüssel
  würde es den Prototyp setzen). Der dritte Gegenleser (Aufrufer, CI) fand
  nichts: jede Zeile fragt mit demselben Hook-Array, eine Überfahrt pro Liste.
- [~] **Die Status-Kopplung zieht in den Kern** (`taskCascade`), in denselben
  zwei Schritten. Vier Antworten: was ein Elternteil aus seinen Kindern ist
  (`deriveStatusFromChildren`), welche Schreibvorgänge ein Statuswechsel
  plant — Wurzel, Nachkommen, Vorfahren, IN DIESER REIHENFOLGE
  (`planStatusCascade`), dieselben nach Anlegen/Löschen einer Unteraufgabe
  (`planAncestorRecompute`), und das Begleitdatum „gestartet → heute"
  (`autoDateOnStart`). Der Kern liest keine Uhr: `todayKey` reist als
  Parameter; ob er überhaupt mitreist (Einstellung, Anbieter kann
  `in_progress` halten, Listen-Regel) entscheidet die Oberfläche, genauso das
  Anwenden und Ansagen der Schreibvorgänge.
  ↳ **Schritt 1 (dieser PR): die Tabelle.**
  `crates/cal-core/tests/fixtures/taskCascade.json`, gemessen am heutigen
  TypeScript: 8 Auto-Datum-Zeilen, 17 Ableitungen (JEDE Anwesenheitsmenge der
  vier Zustände), 34 Kaskaden, 11 Nachberechnungen — die 42 Desktop-Tests
  plus die Zeilen, die der Umzug braucht und die es nie gab: die exakte
  Schreib-Reihenfolge (Tiefensuche, LETZTES Kind zuerst), der Halt der
  Kaskade an der ersten unveränderten Ebene gegen das Durchsteigen der
  Nachberechnung (eine absichtliche Asymmetrie, jetzt gepinnt), eine Wurzel,
  die nicht in der Liste ist (bekommt ihren Schreibvorgang trotzdem), ein
  verwaister Elternteil, die leere Eltern-Id (Wahrheitswert-Test: kein
  Elternteil, in BEIDE Richtungen — der #46-Zwilling, diesmal vorab gepinnt),
  der leere Heute-Schlüssel, das leere geplante Datum (zählt als datiert).
  `taskCascade.contract.test.ts` spielt zurück (71 Tests); rot bewiesen (Regel
  „abgebrochen bleibt abgebrochen" entfernt → die Kaskaden-Zeilen fallen).
  ↳ **Schritt 2 (Folge-PR auf #49): gebaut.** `cal_core::task_cascade` — ohne
  Feature-Gate — plant die Schreibvorgänge (`plan_status_cascade_json`,
  `plan_ancestor_recompute_json`; Antwort = `StatusWrite {task_id, status,
  scheduled_date?}` in Anwendungsreihenfolge) und beantwortet das Begleitdatum
  (`auto_date_on_start_json`), weil der Aufgaben-Dialog das Datum der Wurzel
  selbst setzt. Drei Türen auf beiden Oberflächen, sechs Wire-Typen erzeugt.
  Die Ableitung (`deriveStatusFromChildren`) hat KEINE eigene Tür — kein
  Aufrufer braucht sie allein; die Fixture beobachtet sie durch die
  Nachberechnung (Elternteil zweimal gefragt, als `open` und als `cancelled`),
  ihre acht Desktop-Unit-Tests sind gestrichen (17 Anwesenheitsmengen liegen in
  der Tabelle). Die Hülle schickt vier Felder je Zeile (`id`, `status`,
  `parent_id`, `scheduled_date`) und gibt `StatusWrite`/`CascadeOptions` in der
  alten Gestalt zurück — kein Aufrufer geändert. Die drei Wahrheitswert-Regeln
  (leere Eltern-Id, leerer Heute-Schlüssel, leeres Datum) liest der Kern wie
  das TypeScript, die Hülle schickt für die ersten zwei ohnehin `null`. Rust
  5/5 beim ersten Lauf (70 Zeilen + Draht), rot bewiesen (Asymmetrie der
  Nachberechnung entfernt → eine Zeile fällt).
- [~] **Die Wiederholungs-Projektion zieht in den Kern**
  (`expandTaskOccurrences`), das letzte Aufgaben-Modul des Bogens, in denselben
  zwei Schritten. Drei Antworten: welche Vorkommen eine wiederkehrende
  eingeplante Aufgabe in einem Fenster zeigt — die echte Aufgabe an ihrem
  eigenen Tag, schreibgeschützte Projektionen an jedem anderen, alles andere
  unverändert durchgereicht (`expandScheduledRecurringTasks`); ein Schritt
  desselben Laufs (`nextTaskOccurrence`); was „auf diesen Tag verschieben" auf
  einer Quelle sein kann, der das Datum gehört (`occurrenceMoveTarget`). Der
  Kern antwortet mit POSITIONEN und TAGEN (welche Eingabe-Aufgabe, welcher Tag,
  echt oder projiziert); die Kopie und ihre Id (`<id> occ <tag>`) baut die
  Hülle, wie sie sie auch zurückliest.
  ↳ **Schritt 1 (dieser PR): die Tabelle.**
  `crates/cal-core/tests/fixtures/taskOccurrences.json`, gemessen am heutigen
  TypeScript: 41 Projektionen, 14 Schritte, 7 Verschiebungen — die 34
  Desktop-Tests plus die Zeilen, die der Umzug braucht und die es nie gab: eine
  Basis NACH dem Fenster (fehlt, wird nicht durchgereicht), ein leeres Fenster,
  die Standard-Obergrenze 400, die Schritt-Obergrenze 100 000 (Basis 1700 kommt
  nie an), Intervall 0 = 1, ein Zähl-Ende endet nicht, ungültige Feste Daten
  werden VERWORFEN (Tag 32, Monat 13, Tag 0) — der Spawner klemmt stattdessen,
  ein Unterschied zwischen Spawner und Projektor, gepinnt wie der Projektor
  heute antwortet; monatlich ohne Monatstag schreitet vom geklemmten Tag weiter
  (31. → 28. → 28.), eine Basis abseits ihres Wochentags zeigt sich trotzdem,
  zwei Serien bleiben in Eingabe-Reihenfolge, `in_progress` projiziert. Eine
  Zeile trägt `wasTypeScript`: ein leeres `scheduled_date` lässt die Aufgabe
  VERSCHWINDEN (JS-Artefakt: "" ist nicht null, parst zu Invalid Date) — kein
  Erzeuger schreibt es; der Port darf "" als undatiert lesen und ändert die
  Zeile dann absichtlich. `expandTaskOccurrences.contract.test.ts` spielt zurück
  (63 Tests); rot bewiesen (Regel „erledigt projiziert nicht" entfernt →
  zwei Zeilen fallen).
  ↳ **Schritt 2 (Folge-PR auf #51): gebaut.** `cal_core::task_occurrences` —
  ohne Feature-Gate — läuft auf den Schrittfunktionen des Spawners
  (`spawn::next_trigger`, `spawn::recurrence_ended`, jetzt `pub(crate)`) und
  liest die Regel vorher wie der Projektor: ungültige Feste Daten verworfen
  (keine übrig = keine), Monatstag außerhalb 1..31 = keiner. Drei Türen auf
  beiden Oberflächen (`expandTaskOccurrences` → Zeilen `{task, day,
  projection}`, `nextTaskOccurrence`, `occurrenceMoveTarget`), sechs Wire-Typen
  erzeugt. Die Hülle schickt drei Felder je Zeile (`status`, `scheduled_date`,
  `recurrence` im Wire-Format), legt die Antwort über die eigenen Zeilen
  (echte Aufgabe = das Objekt des Aufrufers, Projektion = Kopie mit
  `<id> occ <tag>`) und behält die Id-Kodierung; `nextTaskOccurrence` nimmt
  weiter den Formularwert und wandelt mit `toBackend`. Die
  `wasTypeScript`-Zeile ist absichtlich geändert: ein leeres `scheduled_date`
  liest die Hülle als undatiert, die Aufgabe wird durchgereicht statt zu
  verschwinden (Zeile jetzt `an-empty-scheduled-date-is-undated`, im Rust-Test
  dieselbe Normalisierung). Der Spawner selbst bleibt unverändert — sein Klemmen
  ungültiger Fester Daten ist ein eigener Punkt (unten). Rust 4/4 beim ersten
  Lauf (62 Zeilen + Draht), rot bewiesen (Verwerf-Regel entfernt → die
  Tag-32-Zeile und die Alle-ungültig-Zeile fallen).
- [x] **Spawner und Projektor lesen ungültige Feste Daten verschieden.** ENTSCHIEDEN
  UND ANGEGLICHEN (siehe unten).
  `spawn::next_fixed_date_after` KLEMMT Tag 32 auf den Monatsletzten und Tag 0
  auf den 1.; `fromBackend` (und jetzt `task_occurrences::projector_rule`)
  VERWIRFT solche Einträge. Dasselbe beim Monatstag: `spawn::advance` klemmt
  `day_of_month` ≥ 32 auf den Monatsletzten und liefert bei 0 gar KEIN Datum
  (`clamp_to_month(y, m, 0)` = None — es wird keine nächste Instanz erzeugt,
  obwohl der Kalender den 15. jedes Monats projiziert hat); `projector_rule`
  verwirft den Eintrag und geht in ganzen Monaten vom Ankertag weiter. Kein
  Editor schreibt solche Werte (`toBackend` bereinigt, EWS klemmt beim
  Abbilden), ein Sync-übertragener oder fremd geschriebener Datensatz könnte.
  Entscheiden: eine Lesart für beide — vermutlich das Verwerfen, weil ein
  Trigger, den niemand so gemeint hat, kein Datum erzeugen sollte; dann
  `spawn.rs` anpassen (`day_of_month.filter(1..=31)` auch dort, Feste Daten
  bereinigen statt klemmen) und die Fixture-Zeilen
  `a-fixed-date-with-day-32-is-dropped` und
  `monthly-day-of-month-clamps-to-short-months` bleiben, wie sie sind. (Vom
  dritten Gegenleser des Port-PRs gefunden: die Flagge nannte nur die Festen
  Daten.)
  ↳ **Angeglichen: das Verwerfen, auf beiden Seiten.** `spawn.rs` liest eine
  Regel jetzt wie der Projektor — `is_valid_month_day` (Monat 1..12, Tag 1..31)
  entscheidet, ob ein Fester Trigger überhaupt einen Kalendertag benennt;
  ungültige werden übersprungen, und nur eine Regel mit mindestens einem
  gültigen Trigger ist eine Feste-Daten-Regel (sonst läuft die Frequenz, auch
  im Backlog-Zweig). `advance` ignoriert einen Monatstag außerhalb 1..31 statt
  ihn zu klemmen (0 lieferte vorher gar kein Datum). Ein GÜLTIGER Tag über
  die Monatslänge hinaus klemmt weiter (30. Februar = Monatsende), beide
  Seiten. `task_occurrences::projector_rule` ist damit weg — der Projektor
  braucht keine eigene Lesart mehr, die Fixture-Zeilen laufen unverändert
  durch dieselben Schrittfunktionen. Sieben Spawner-Tests pinnen die Lesart
  (Tag 32/Monat 13/Tag 0 verworfen, ungültig neben gültig ignoriert, 30.
  Februar klemmt, Backlog mit nur ungültigen Triggern nimmt sein Intervall,
  Monatstag 0 und 40 schreiten in ganzen Monaten, Monatstag 31 klemmt).
- [~] **Die kleinen Doppelungen ziehen in den Kern** — der Bogen nach den
  Aufgaben-Regeln (entschieden 2026-09-13): `taskAssignment`, dann
  `eventGroups`, `signatures`, `birthdays`, `planTaskDates`,
  `syncConflictGroups`; ein PR-Paar pro Modul, vermessen, dann Port.
  ↳ **taskAssignment, Schritt 1 (dieser PR): die Tabelle.** Von den fünf
  Exporten ist `isMineOrUnassigned` längst der Kern (`is_mine_or_unassigned`,
  gepinnt durch `shared/contracts/taskOwnership.json`, das host-core liest);
  die TypeScript-Kopie dient noch `shared/dayStart.ts` und fällt mit dem
  Tagesstart. `classifyDoneByMe` hat seit der Gruppierung (#42) keinen
  Aufrufer mehr und fällt mit dem Port. Was umzieht: `selfAssignOnStatusChange`
  (wer eine Aufgabe nach dem Statuswechsel hält — nehmen beim Starten oder
  Erledigen, zurücktreten beim Wiederöffnen; vier Aufrufstellen auf beiden
  Oberflächen), `taskAssignmentMode` (wie viele Personen eine Liste halten
  kann; fehlende Fähigkeit = keine) und `clampAssignees` (auf das kürzen, was
  die Liste hält — die ERSTE bleibt, dieselbe wie bei Todoists Adapter).
  `crates/cal-core/tests/fixtures/taskAssignment.json`, gemessen am heutigen
  TypeScript: 15 Zuweisungs-, 6 Modus-, 5 Kürzungs-Zeilen — die Desktop-Tests
  plus die Zeilen, die der Umzug braucht: Reihenfolge bleibt, wenn ich in der
  Mitte stehe; JEDE Kopie von mir fällt beim Zurücktreten; Abbrechen einer
  Aufgabe, die ich halte; `in_progress` auf der Aufgabe eines Kollegen; leere
  Liste unter `single`; jeder deklarierte Modus. Der Kern wird POSITIONEN
  antworten (welche der gegebenen Zuweisungen bleiben) oder einen Zustand
  („nimm mich"), nie eine Nutzer-Zeile. `taskAssignment.contract.test.ts`
  spielt zurück (27 Tests); rot bewiesen (Zurücktreten leert alles → drei
  Zeilen fallen).
  ↳ **Schritt 2 (Folge-PR auf #55): gebaut.** `cal_core::task_assignment`
  beantwortet die drei Fragen: `self_assign_on_status_json` mit einem
  ZUSTAND (`unchanged` / `assign_me` / `keep {positions}` — welche der
  gegebenen Zuweisungen bleiben, in Reihenfolge), `task_assignment_mode_json`
  (liest nur `task_assignment` aus dem Fähigkeiten-Block, fehlend = keine),
  `clamp_assignees_json` (Positionen; `single` behält die erste). Ein Nutzer
  ist auf der Leitung seine Id — die Regeln vergleichen nur Ids —, und keine
  Nutzer-Zeile wird zurückgegeben. `TaskAssignment` ist statt einer
  Zeichenkette NACH UNTEN gezogen: die Aufzählung liegt jetzt in `cal-core`,
  `plugin-core` reexportiert sie unter dem alten Pfad (kein Aufrufer im
  Manifest geändert, das erzeugte `TaskAssignment.ts` ist dasselbe). Drei
  Türen auf beiden Oberflächen, fünf Wire-Typen erzeugt. Die Hülle schickt
  Ids und legt Positionen über die eigenen Nutzer-Objekte; Signaturen der drei
  Funktionen unverändert, kein Aufrufer geändert. `classifyDoneByMe` (tot seit
  #42) ist gelöscht; `isMineOrUnassigned` bleibt als TypeScript für
  `shared/dayStart.ts`, weiter durch `taskOwnership.json` gepinnt. Rust 4/4
  beim ersten Lauf, rot bewiesen (Zurücktreten behält mich statt der anderen
  → Zeilen-Test und Draht-Test fallen).
- [~] **Die Signaturen ziehen in den Kern** (`signatures`), das zweite Modul
  des Bogens der kleinen Doppelungen. Dazwischen geprüft und gestrichen:
  `eventGroups` (drei Helfer sind JavaScript-Kleber um erzeugte Typen —
  Map-Schlüssel als JSON, Map-Index, Wire-Objekt aus dem Termin; dieselbe
  Begründung wie `eventKey.ts`), `planTaskDates` (liest die Uhr, formatiert)
  und `syncConflictGroups` (Listen-Gruppierung; wenn, dann im Host).
  `birthdays` braucht einen Entwurf: der Host liefert den Namen eines
  Geburtstagskalenders als Schlüssel plus Listenname statt „Birthdays – "
  (DESIGN §4.5 a), dann fallen die Schere und die Präfix-Zwillinge.
  ↳ **Schritt 1 (dieser PR): die Tabelle.** Die Signatur-Regel hat KEINEN
  Rust-Zwilling; beide Editoren laufen sie: die LETZTE Zeile, die genau `-- `
  ist (nachlaufender Leerraum egal), eröffnet den Block; Auslesen, Abschneiden
  (samt Leerzeilen davor), Anwenden (ersetzt statt zu stapeln; leerer Körper
  entfernt; leere Beschreibung öffnet ohne Lücke).
  `crates/cal-core/tests/fixtures/signatures.json`, gemessen am heutigen
  TypeScript: 20 Texte (je Auslesen und Abschneiden) und 19 Anwendungen — die
  11 Desktop-Tests plus die Zeilen, die der Umzug braucht: Marker mit
  Tab/mehr Leerraum, eingerückter Marker (keiner), `---` (keiner), Marker als
  erste/letzte/einzige Zeile, mehrere Leerzeilen davor, ohne Leerzeile davor,
  CRLF (Split nur an LF, das CR bleibt an seiner Zeile), und die Zeichen, bei
  denen JavaScript und Rust auseinandergehen (DESIGN §4.5 c): NEL U+0085 ist
  für JavaScript KEIN Leerraum, BOM U+FEFF und NBSP SIND welcher — der Port
  darf nicht auf `is_whitespace` lehnen. Gemessen und gepinnt: nachlaufende
  Zeilenumbrüche des Textes bleiben erhalten und bekommen die Lücke obendrauf.
  `signatures.contract.test.ts` spielt zurück (60 Tests); rot bewiesen
  (erster statt letzter Marker → die Weiterleitungs-Zeilen fallen).
  ↳ **Schritt 2 (Folge-PR auf #57): gebaut.** `cal_core::signatures` — ohne
  Feature-Gate — mit `signature_in_json`, `strip_signature_json`,
  `apply_signature_json` und der Konstante `SIGNATURE_MARKER`. Der Leerraum ist
  AUSGESCHRIEBEN (`is_js_whitespace`: JavaScripts WhiteSpace + LineTerminator —
  BOM, NBSP und alle Zs zählen, NEL U+0085 nicht), statt auf `is_whitespace` zu
  lehnen; genau die Zeilen, die die Fixture dafür trägt, wären sonst rot. Split
  nur an LF, wie gemessen. Drei Türen auf beiden Oberflächen, zwei
  Wire-Typen erzeugt. Die Hülle behält ihre drei Funktionsnamen und die
  Marker-Konstante (für die Tests, die einen Block ausschreiben; der
  Vertragstest prüft, dass Hülle und Kern dieselbe Marke buchstabieren); kein
  Aufrufer geändert (Termin-Dialog, Signatur-Knopf, mobiler Editor). Rust
  beim ersten Lauf grün, rot bewiesen (erster statt letzter Marker).
  ↳ **Vom Review benannt, kein Befund dieses Ports:** ein einzelnes
  Surrogat-Halbzeichen (kaputtes UTF-16, etwa aus einer fremden Zwischenablage)
  wirft an JEDER JSON-Tür der App — serde lehnt `\ud83d` ohne Partner ab —,
  hier wie bei der Konferenz-Erkennung, die derselbe Editor in derselben
  Beschreibung längst fragt; ein Rust-Rand trägt UTF-8, eine byte-gleiche
  Antwort gäbe es also ohnehin nicht. Kein Erzeuger bekannt (Host-Zeilen sind
  gültiges UTF-8, Eingabefelder liefern wohlgeformte Paare). Wenn Toleranz
  gewünscht ist, gehört sie in die Tür-Konvention insgesamt, nicht in ein
  Modul. 5.000 Zufallstexte mit allen Leerraum- und Steuerzeichen, Emoji und
  Markern: Byte-gleich.
- [x] **Geburtstagskalender: der Kern nennt das Buch, die Oberfläche den Kalender**
  — das letzte Modul des Bogens der kleinen Doppelungen, nach einem Entwurf
  (entschieden 2026-09-13: Feld mit Buchname; Kalenderzeile als Typ erzeugt).
  `host_core::birthdays::synthesise_calendar` schrieb englisch „Birthdays – "
  in `Calendar.name`, und beide `listCalendars`-Hüllen schnitten es mit
  `birthdayCalendarListName` wieder ab. Jetzt ist der Name der Buchname, und
  `host_core::wire::CalendarRow` trägt auf einer Geburtstagszeile — und nur
  dort — `birthdays: {contact_list_id, list_name}`, gestempelt von
  `CalendarRow::new` aus der Id, so dass kein Host es vergessen kann (der
  Desktop-Befehl baute seine Zeilen bisher als Literale und geht jetzt auch
  durch `new`). Eine gemeinsame `localizeBirthdayCalendarName` in
  `shared/birthdays.ts` setzt „Geburtstage – Familie" aus dem Schlüssel; beide
  Hüllen rufen dieselbe. Gefunden und mit entfernt: der Zweig „ein
  umbenannter Geburtstagskalender bleibt, wie er ist" war tot — der Desktop
  hängt die Geburtstagszeilen NACH dem Stempeln der Umbenennungen an, Mobile
  wendet bewusst keine an —, und der Rust-Kommentar, man könne über eine
  Umbenennung neu übersetzen, war falsch. `cal_core::Calendar` und
  `CalendarRow` erzeugen jetzt ihre TypeScript-Typen; die zwei
  handgeschriebenen `Calendar`-Kopien (Desktop `src/api/types.ts`, Mobile
  `api/calendar.ts`) sind weg — dieselbe Anordnung, die bei den
  Aufgabenlisten-Zeilen schon einmal Schaden angerichtet hatte. Die Id-Präfixe
  bleiben (die mobile Erinnerungsübersicht kennt nur die Termin-Id), sind aber
  jetzt durch `shared/contracts/birthdayIds.json` gegen die Ids gepinnt, die
  die Synthese tatsächlich erzeugt; die Termin-Präfix-Zeichenkette ist dafür
  eine Konstante `BIRTHDAY_EVENT_PREFIX` geworden.
- [~] **Der Tagesstart zieht in den Kern** (`dayStart`), ein neuer Bogen nach
  den kleinen Doppelungen (gewählt 2026-09-13). `shared/dayStart.ts` hat außer
  `is_mine_or_unassigned` keinen Rust-Zwilling; Desktop-Prüfer und -Dialog,
  die mobilen Prüfungen, das Modal und der Vorplaner der OS-Benachrichtigungen
  laufen die Regeln jeden Morgen. Drei davon lesen die Uhr selbst
  (`movedToToday`, `filterDeadlinePinTargets`, und `shouldFireToday` über
  `now`); der Kern bekommt Tag und Uhrzeit hinein.
  ↳ **Schritt 1 (dieser PR): die Tabelle.**
  `crates/cal-core/tests/fixtures/dayStart.json`, gemessen am heutigen
  TypeScript in Europe/Berlin: 134 Zeilen über zwölf Regeln — überfällig,
  verschleppt (mit Kopplung), handlungsfähige Nachfahren (Liste und Ja/Nein),
  „auf heute", Frist-Anheften, Tage bis zur Frist, die drei Erinnerungen samt
  Gruppen, das Auslöse-Tor. Die Tests aus `dayStartReview.test.ts`,
  `DeadlinePinChecker.test.ts` und `useCurrentDayKey.test.ts`, plus die
  Zeilen, die der Umzug braucht: Eingabe-Reihenfolge; Zuständigkeit nur per Id
  und für eine Liste ohne Eintrag; ein Projekt-Elternteil mit abgelaufener
  Frist UND abgelaufenem Plan ist nie überfällig, landet deshalb bei
  „verschleppt" und versteckt seine verschleppte Unteraufgabe; die Kopplung
  fragt nur die Liste der Unteraufgabe (der Vorfahr darf in einer
  ungekoppelten liegen); der verschleppte Vorfahr eines Kollegen versteckt
  nichts; die Stapel-Reihenfolge der Nachfahren; beide Zeitumstellungen und
  ein Schalttag, ein Jahr unter 100 (JavaScript repariert es, der Rückweg
  lehnt ab); das Durchfallen in die nächste Gruppe bei ausgeschaltetem
  Schalter; die Ränder des Auslöse-Parsers (Sekunden, Leerzeichen, einstellige
  Minute, nicht-ASCII-Ziffern, Marke eines anderen Tages). Als
  `wasTypeScript` markiert: ungepolsterte und hexadezimale Tage, ein Anker,
  der kein Tag ist (Textvergleich), eine leere Uhrzeit, und eine doppelte Id,
  unter der der Weg zweimal läuft (der Enkel käme zweimal in den Stapel).
  Nicht messbar, weil es hängt: drei Baum-Wege ohne Besucht-Menge laufen bei
  einem Eltern-Zyklus endlos (`hasActionableDescendants` für jeden Kandidaten
  der Erinnerungen, `actionableDescendants`, die Kopplung beim Verschleppen);
  der Port muss enden. Außen vor und warum (`notInThisTable`): die
  Zusammensetzung um die Regeln — verschleppte Zeilen nach dem
  Übertrags-Standard der Liste teilen, die „aufgetaucht"-Zahl, die Ziele des
  stillen Stapels — liegt DREIMAL inline (Desktop-Prüfer, `useDayStartChecks`,
  `dayStartSchedule`) und ist der nächste Schritt; `effectiveForList` und
  `parseCountdownDays` gibt es je zweimal; `host_core::reminders` liest
  dieselbe Einstellung `tasks.dayStartTrigger` anders (Sekunden erlaubt,
  Unsinn heißt Mitternacht statt „sofort") — eine Entscheidung für den Port.
  `dayStart.contract.test.ts` spielt zurück (135 Tests); rot bewiesen (eine
  heute fällige Frist als überfällig → zwei Zeilen fallen). Kein
  Produktionscode geändert.
  ↳ **Schritt 2 (Folge-PR auf #60): gebaut.** `cal_core::day_start` — ohne
  Feature-Gate — mit EINER Tür `day_start_json`: eine `DayStartQuestion` nennt
  ihre Regel (`rule`: overdue, carried_over, actionable_descendants,
  has_actionable_descendants, moved_to_today, deadline_pin_targets,
  days_until_deadline, untimed_today, deadline_arrived, deadline_countdown,
  reminder_groups, should_fire) und trägt, was diese Regel liest; die Antwort
  sind Positionen, Tage oder Ja/Nein. Zwölf Türen wären zwölf Verdrahtungen je
  Oberfläche für dieselbe Überfahrt gewesen. Der Kern liest keine Uhr: die Hülle
  schickt den Tag und für das Auslöse-Tor Stunde und Minute. Ein Nutzer ist
  seine Id, die Identität und die Kopplung reisen je Liste. Tage werden wie im
  TypeScript als TEXT verglichen (nach UTF-16-Einheit, damit auch ein Anker,
  der kein Tag ist, gleich sortiert); nur die Tage bis zur Frist lesen einen
  Tag als Datum, und zwar streng (`YYYY-MM-DD`, ASCII-Ziffern, Jahr ab 100).
  Die Wege nach unten erweitern jede Id höchstens einmal, der Aufstieg hält an
  einer schon gesehenen Id — ein Eltern-Zyklus hängt den Tagesstart nicht mehr
  (Rust-Tests in `day_start::walks`). ABSICHTLICH geänderte Fixture-Zeilen: ein
  ungepolsterter und ein hexadezimaler Tag sind keine Tage mehr, und unter einer
  doppelten Id läuft der Weg einmal (der Enkel kommt einmal in den Stapel, beide
  Zeilen mit der Id bleiben — zwei Konten, zwei Aufgaben). Die Hülle behält alle
  zwölf Funktionsnamen und Signaturen; kein Aufrufer geändert. Die letzte
  TypeScript-Kopie von `isMineOrUnassigned` ist gelöscht; der Kern fragt
  `task_assignment::mine_or_unassigned` über Ids, und
  `shared/contracts/taskOwnership.json` wird auf der TypeScript-Seite jetzt
  durch die Tagesstart-Tür gelesen (DESIGN, Reichweiten-Wächter-Kommentar
  angepasst). Rust 19/19 beim ersten Lauf, rot bewiesen.
  ↳ **Vom Review gefunden und behoben:** die vier Übertrags-Schleifen
  (Desktop-Prüfer und -Dialog, mobile Prüfungen und Modal) fragten
  `actionableDescendants` je verschleppter Wurzel, und jede Frage schickte und
  indexierte die ganze Aufgabenliste — bei 2000 Aufgaben und 100 Wurzeln etwa
  0,4 s statt 6 ms, auf dem UI-Thread. Jetzt gibt es die Stapelform
  `actionable_descendants_of` (Shell: `actionableDescendantsOf`): eine Frage für
  alle gekoppelten Wurzeln, ein Index; die Schleifen bauen ihre Ziele in
  derselben Reihenfolge. In Rust und durch die Tür gegen den Einzelweg über jede
  Nachfahren-Zeile der Fixture gepinnt. Dazu vier veraltete Sätze (die
  taskAssignment-Fixture, der Rust-Leser des Besitz-Vertrags liegt in host-core,
  die `writtenBy`-Zeile nannte die umbenannte Zeile, der Kopf des
  TypeScript-Vertragstests). Acht weitere Befunde widerlegt, darunter die
  Rundung naher Fließkommazahlen (kein Aufrufer erzeugt sie: das Fenster ist
  1..30 geklemmt, die Aufgaben-Überschreibung ist i64) und Zeitzonen, die einen
  Kalendertag übersprangen (der Kern zählt Kalendertage, wie die Funktion es
  immer versprach; betroffen wären nur historische Tage vor dem Heute).
- [~] **Die Tagesstart-Zusammensetzung zieht in den Kern** — der zweite Teil des
  Tagesstart-Bogens, nach den Regeln (#60/#61). Entschieden 2026-09-13: EINE
  Planungs-Frage an den Kern; das Einlesen der Aufgaben-Einstellungen folgt als
  eigener Schritt ebenfalls in den Kern. Die Zusammensetzung steht dreimal
  inline — Desktop-`DayStartReviewChecker`, mobile `useDayStartChecks`, mobiler
  `dayStartSchedule` (für künftige Morgen, nur die Zahl): Überfällige,
  verschleppte Zeilen nach dem Übertrags-Standard der Liste in Fragen / Heute /
  Backlog geteilt, die Ziele der stillen Stapel (Wurzel plus, wo ihre Liste
  koppelt, die handlungsfähigen Nachfahren), die Erinnerungsgruppen und die
  Zahl, die den Rückblick öffnet.
  ↳ **Schritt 1 (dieser PR): die Tabelle.**
  `crates/cal-core/tests/fixtures/dayStartPlan.json`, gemessen am heutigen
  TypeScript: 19 Fälle. Gemessen wurde der Block des Desktop-Prüfers, wörtlich
  in die Testhilfe kopiert (samt Ziel-Sammlung aus `runAutoCarryOverBatch`);
  ein Textvergleich bestätigt Zeile für Zeile, dass Aufteilung und Ziel-Sammlung
  gleich sind und die mobilen Prüfungen denselben Code laufen. Der Vorplaner
  behält Zeilen mit genau `ask`, die Prüfer alles außer `today`/`backlog` — auf
  beiden Oberflächen ist der Wert beim Einlesen geprüft, also dieselbe Menge.
  Gepinnt: Aufteilung nach globalem Standard und Überschreibungen je Feld,
  Überfällige zählen in jeder Liste und sind nie stille Zeile, Erinnerungen
  allein öffnen den Rückblick, die Zahl ist Überfällige + Fragen + Erinnerungen,
  Ziele Wurzel für Wurzel mit Nachfahren nur bei koppelnder Liste, eine
  Unteraufgabe, die zugleich eigene Zeile ist, einmal. Gemessen und gepinnt, wie
  es ist: eine Unteraufgabe in einer ungekoppelten Backlog-Liste unter einer
  Heute-Wurzel steht in BEIDEN Stapeln (Heute läuft zuerst, Backlog danach),
  und die offene Unteraufgabe einer Kollegin unter meiner Wurzel wird
  mitgetragen (der Weg nach unten fragt keine Zuständigkeit).
  `dayStartPlan.contract.test.ts` spielt zurück (20 Tests); rot bewiesen
  (Backlog-Zeilen als Heute-Zeilen → drei Fälle fallen). Kein Produktionscode
  geändert.
  ↳ **Schritt 2 (Folge-PR auf #62): gebaut.** `cal_core::day_start::plan`,
  gefragt über dieselbe Tür (`rule: "plan"`): Überfällige, Fragen / Heute /
  Backlog, die Ziele beider Stapel, die Erinnerungsgruppen und die Zahl, in
  Positionen. Die Einstellungen reisen je Liste aufgelöst mit
  (`DayStartListSettings`, `CarryOverDefault`); eine Liste, die die Frage nicht
  nennt, koppelt nicht und fragt. Die Hülle `planDayStart` legt die Antwort
  über die eigenen Aufgaben; Desktop-Prüfer, mobile Prüfungen und mobiler
  Vorplaner fragen sie, die drei Inline-Kopien und die Ziel-Sammlung in beiden
  `runAutoCarryOverBatch` sind weg. ZWEI Antworten absichtlich geändert
  (Toni, 2026-09-13): **die Elternaufgabe entscheidet** — eine verschleppte
  Zeile, die eine still übertragene Wurzel mitbringt, ist keine eigene Zeile
  mehr (vorher zählte sie doppelt, wurde nach dem Verschieben noch gefragt und
  mit anderem Standard ihrer eigenen Liste von beiden Stapeln geschrieben, der
  spätere gewann); und **der Stapel bringt nur, was mir oder niemandem
  gehört** — dieselbe Zuständigkeit, nach der die Zeilen selbst gewählt werden
  (vorher auch die offene Unteraufgabe einer Kollegin; der Weg geht durch sie
  hindurch weiter). Die Sammel-Knöpfe in Dialog und Modal fragen
  die Stapelform jetzt mit `meFor`; die Einzel-Aktion je Zeile bleibt ohne
  Zuständigkeit. Fixture: drei Zeilen geändert, drei neu (je mit Notiz), 22
  Fälle. Rust 24/24 beim ersten Lauf; rot bewiesen (Mitnehmen ausgeschaltet →
  die Plan-Zeilen fallen).
  ↳ **Vom Review gefunden und behoben:** mit wiederholten Ids (zwei Konten)
  ging eine verschleppte Aufgabe verloren — nicht gefragt, von keinem Stapel
  geschrieben. Zwei Wege dahin: die Ziele waren nach Id gesammelt, so dass eine
  zweite Zeile mit derselben Id die genommene verdrängte; und Wurzeln konnten
  einander nehmen, weil der Weg nach unten jeder Zeile mit der Eltern-Id folgt,
  der Aufstieg aber nur der letzten Zeile mit einer Id. Jetzt werden Ziele je
  Zeile gesammelt (zwei Zeilen mit einer Id sind zwei Aufgaben und werden beide
  geschrieben — die JavaScript-`Map` behielt nur die spätere), und eine
  genommene Zeile, die kein verbleibender Stapel schriebe, bleibt in ihrem
  Teil. Mit eindeutigen Ids ändert sich nichts (200.000 Zufallsmorgen gegen eine
  unabhängige Nachbildung der Entscheidungen: keine unerklärte Abweichung).
  Dazu: die Begründungen im Moduldoc, hier und im PR waren zu stark („nie in
  beide Richtungen“, „in beiden Stapeln“, „wie jede andere Regel“), der
  Dialog-Kommentar nannte Sammel-Knöpfe und Einzel-Aktion gleich, der
  Besitz-Weg der Sammel-Knöpfe hatte keinen Test (jetzt in Rust durch die Tür
  und in TypeScript), die Namens-Wächter nannten die neuen Zeilen nicht, und
  zwei Fixture-Texte beschrieben den Stand vor dem Umzug.
- [~] **Die Aufgaben-Einstellungen ziehen in den Kern** — der dritte Teil des
  Tagesstart-Bogens, nach Regeln (#60/#61) und Zusammensetzung (#62/#63).
  Entschieden 2026-09-13. Beide Oberflächen lesen dieselben sechzehn
  gespeicherten Werte mit je eigener Kopie jeder Regel: die Aufgaben-Schalter,
  das Countdown-Fenster, das Tagesfenster des Kalenders, Ansicht, Abhaken,
  Übertrags-Standard, Auslöser und die Überschreibungen je Liste — Desktop im
  `TaskCascadeProvider` (private Helfer), Mobile in `taskBehaviour.ts`. Dazu die
  wirksamen Werte je Liste und was beim Schreiben geklemmt wird.
  ↳ **Schritt 1 (dieser PR): die Tabelle.**
  `crates/cal-core/tests/fixtures/taskSettings.json`, gemessen am ECHTEN Code
  beider Oberflächen: der Desktop-Provider gerendert, seine Lesungen aus der
  Tabelle beantwortet; das mobile Modul mit gemocktem Einstellungs-Modul. 88
  Zeilen: 54 Lesungen, 6 wirksame Werte, 12 Countdown-Schreibungen, 9
  Tagesfenster-Schreibungen, 7 Überschreibungs-Änderungen. Gepinnt: Schalter
  gehen nur bei genau `false` aus (die Zweistufen-Priorität nur bei genau
  `true` an); das Countdown-Fenster über `parseInt` (`12abc` ist 12, `1e2` ist
  1, `0x10` ist 0 und wird 1) und 1..30 geklemmt, beim Schreiben gerundet,
  nicht-endlich ist der Standard 3; das Tagesfenster auf halbe Stunden
  gerundet (halb nach oben), geklemmt, verdreht oder leer ist der ganze Tag;
  der Auslöser nur aus den fünf angebotenen Werten (`07:00` wird `00:00`); die
  Überschreibungen je Feld geprüft, leere Einträge fallen, die Reihenfolge der
  gespeicherten JSON-Schlüssel bleibt beim Ändern.
  **Wo die Oberflächen heute auseinandergehen** (Zeile trägt beide Antworten):
  eine fehlgeschlagene Lesung macht mobil ALLE Einstellungen zum Standard, am
  Desktop nur die eine; und eine Liste mit der Id `__proto__` wird am Desktop
  gespeichert, mobil nicht. JavaScript-Artefakte als `wasTypeScript`: die Liste
  `__proto__` geht beim Lesen als Feld verloren und wirkt über den Prototyp
  trotzdem. `taskSettings.contract.test.tsx` spielt beide Oberflächen zurück
  (89 Tests); rot bewiesen (mobile Klemme auf 29 → die fünf Zeilen, die auf 30
  klemmen, fallen für Mobile). Kein Produktionscode geändert.
  Nachgetragen: die Testhilfe lud die mobilen Module statisch, und die
  Desktop-Typprüfung folgte ihnen bis ins Expo-Brückenmodul — in der CI ohne
  mobile Pakete rot. Jetzt über einen Pfad in einer Variablen geladen, dem
  weder `tsc` noch der Bundler folgt.
  ↳ **Schritt 2 (Folge-PR auf #64): gebaut.** `cal_core::task_settings` mit
  EINER Tür `task_settings_json` und fünf Regeln: `read` (die gespeicherten
  Zeichenketten → die Einstellungen), `effective` (die wirksamen Werte einer
  Liste), `countdown_days_to_store`, `day_window_to_store` und
  `with_list_override`. Gelesen wie JavaScript: `parseInt` (führender
  JavaScript-Leerraum, Vorzeichen, ASCII-Ziffern), `Math.round` (halb nach
  oben), Aufzählungen nur mit ihren Gliedern, die Überschreibungen in der
  Schlüssel-Reihenfolge eines JavaScript-Objekts (Array-Index-Ids zuerst
  aufsteigend, wiederholter Schlüssel: erster Platz, letzter Wert) — ein eigener
  Deserializer, weil `serde_json` ohne `preserve_order` sortiert. Hülle
  `shared/taskSettings.ts`; der Desktop-`TaskCascadeProvider` und die mobile
  `taskBehaviour.ts` lesen und schreiben nur noch den Speicher, alle
  Parse-Helfer beider Seiten sind weg, die Standardwerte fragen beide beim
  Kern (mobil erst beim ersten Gebrauch, nie beim Import). ABSICHTLICH
  geändert (Toni, 2026-09-13): **ein Lesefehler lässt nur diesen Wert auf den
  Standard fallen** (mobil liest jetzt jeden Schlüssel mit eigenem `catch`);
  **eine Lesart des Auslösers** — `host_core::reminders::day_start_time` fragt
  `cal_core::day_start_trigger` (nur die fünf angebotenen Werte, sonst
  `00:00`; vorher jede Uhrzeit, auch mit Sekunden), mit eigenem Host-Test; und
  eine Liste `__proto__` ist eine gewöhnliche Liste. Fixture: vier Zeilen
  geändert, eine neu (numerische Listen-Ids zuerst), 89 Zeilen, keine trägt
  mehr eine Desktop-Antwort. Rust 12/12 und Host 1/1 beim ersten Lauf.
  Nebenbei nötig: `wasm-opt` erlaubt jetzt auch `nontrapping-float-to-int`
  (die Klemmen wandeln f64 in ganze Tage und Minuten, rustc erzeugt
  `i32.trunc_sat_f64_u`, der gebündelte Optimierer lehnte es ab — wie zuvor
  bulk-memory); und der TypeScript-Vertragstest liest seine Fixture mit
  `JSON.parse` statt per Import, weil ein JSON-Import den Schlüssel
  `__proto__` zum Prototyp macht.
  Nach der Prüfung (drei Linsen: 8 bestätigt, 3 widerlegt) behoben: ein
  Countdown, den `parseInt` als Unendlich liest (jenseits von 1,8e308, etwa
  eine 1 mit 309 Nullen), las 30
  statt des Standards 3 — jetzt wieder 3 wie im TypeScript; `serde_json` las
  manche JavaScript-Zahlen eine ULP daneben (104.99999999999999 als 105, also
  120 statt 90) — cal-core schaltet `float_roundtrip` ein. Je eine Zeile dazu
  (91 Zeilen), die geänderten und neuen Zeilen stehen in beiden
  Anti-Stille-Listen, vier veraltete Beschreibungen korrigiert (Notiz der
  Auslöser-Zeile, `dayStart.json`, Kopf des TypeScript-Vertragstests,
  Provider-Doku). Widerlegt: eine Override-Tabelle mit Zahlen jenseits von
  f64, 128 Ebenen Tiefe oder einzelnen Surrogaten fällt ganz auf leer — kein
  Aperio-Schreiber erzeugt so etwas.
- [~] **Termin-Wiederholung zieht in den Kern** — neuer Bogen (Toni,
  2026-09-14), mit Entwurfsrunde. Vermessen: ZWEI Ausroller —
  `shared/recurrence.ts` (`expandAll`, rrule.js) für alle Ansichten und das
  Widget, `host-core`s `expand_occurrences` (rrule-Kiste) für die
  Erinnerungen; UNTIL wird an neun Stellen gelesen, mindestens fünfmal
  verschieden. Entschieden: (1) der Zeitzonen-Fehler der lokalen Erinnerungen
  wird VORAB behoben — PR #66: `enumerate_local_triggers` las `rrule_tzid`
  nicht und rollte in UTC aus, ab der Zeitumstellung am 25.10. eine Stunde zu
  früh; (2) der Bogen beginnt mit dem Ausrollen, der Motor (rrule-Kiste oder
  eigene Regel) wird erst nach dem Pin mit gemessener WASM-Größe gewählt.
  ↳ **Schritt 1 (Pin): gebaut.** `shared/contracts/eventOccurrences.json`
  (dort, weil host-core die Tabelle liest und nur von dort einbetten darf),
  78 Zeilen: Regeln (auch WKST, BYMONTH mit BYDAY, BYMONTHDAY=-1), Bereich
  (auch zonierte Serien genau an den Rändern), UNTIL (auch das `T235959Z` des
  Editors und ein zoniertes UNTIL über eine Zeitumstellung), Ausnahmen, Zonen,
  ganztägig, Einzeländerungen, Größe. `expect` ist die Antwort der Ansichten
  (`expandAll`); wo die Erinnerungen anders antworten — jeder Termin einzeln,
  wie `event_triggers` ausrollt —, trägt die Zeile `reminders`. Verglichen
  werden Zeitpunkte, sortiert nach Zeitpunkt und Position; die Reihenfolge von
  `expandAll` (nach dem Text des Starts) zählt nicht. **22 Zeilen weichen ab**,
  jede eine Entscheidung für den Port: UNTIL als reines Datum, ohne `Z` oder
  vor dem Start (die rrule-Kiste nimmt die Regel nicht an, die Erinnerungen
  behalten nur den Starttermin; rrule.js liest ohne Zone ein Datum als 00:00
  UTC und eine Uhrzeit ohne `Z` als UTC, bei einer zonierten Serie beides als
  Wanduhrzeit — eine ganztägige Serie westlich von UTC verliert so ihren
  letzten Tag, `wasTypeScript`); ein Start mit Millisekunden (die Erinnerungen
  verlieren sie und beginnen einen Tag später); ein abschließendes Semikolon
  (rrule.js scheitert, die Kiste liest es); eine Ausnahme eine Millisekunde
  daneben (die Erinnerungen löschen das Vorkommen trotzdem); ein Zonenname mit
  Leerzeichen (host-core trimmt, Intl nicht) oder in Kleinbuchstaben (Intl
  nimmt ihn, chrono-tz nicht); eine Zeitumstellung um Mitternacht (Santiago:
  die Ansichten schieben 00:30 auf 01:30, die Erinnerungen lassen den Tag aus
  und enden einen Tag später); Einzeländerungen, auch an einer zonierten Serie
  nach der Umstellung (die Erinnerungen wenden keine an, der alte Platz
  erinnert weiter); eine abgesagte Serie (die Erinnerungen überspringen sie,
  gewollt); und die Kappe der Erinnerungen bei 500 Vorkommen. Auf beiden Seiten
  gleich und gepinnt: die Sommerzeit-Lücke (vorwärts um die Lückenlänge), die
  Überlappung (die frühere Lesung), zonierte Serien an den Bereichsrändern, ein
  zoniertes UNTIL mit `Z` über eine Umstellung, WKST, ganztägige Serien ohne
  Zone (steppen in UTC, nach der Umstellung um 23:00 am Vortag), das
  `T235959Z` des Editors auf einer ganztägigen Serie östlich von UTC (behält
  einen Tag zu viel — `wasTypeScript`) und ein Vorkommen, das vor dem Bereich
  begann (fehlt, gewählt wird nach dem Start). Verträge:
  `src/intl/eventOccurrences.contract.test.ts` (Ansichten) und
  `event_occurrence_contract` in `host-core/src/reminders.rs` (Erinnerungen,
  die abweichenden Zeilen namentlich). Die Prüfung von #67 fand die Lücken und
  das Reihenfolge-Artefakt; die Tabelle wurde danach neu gemessen.
  ↳ **Beifund, vorab behoben (Toni, 2026-09-14):** eine Serie verlor beim
  Bearbeiten als Ganzes ihre Zeitzone. Beide Editoren bauten
  `{rrule, exceptions}` aus dem Formular neu, ohne `tzid`, und jeder Schreiber
  speichert, was er bekommt: lokal blieb `rrule_tzid` leer, CalDAV, Google,
  Graph und EWS schrieben den Start in UTC. Ab der nächsten Zeitumstellung
  rutschte die Serie um eine Stunde, in den Ansichten, in ihren Erinnerungen
  und in jedem anderen Programm. Betroffen war jede Serie mit Zone, auch eine
  vom Anbieter (iPhone, Outlook, Google), die seit dem 23.06. als Ganzes
  bearbeitet wurde; ein Termin, der im Editor zur Serie wurde, bekam nie eine.
  Jetzt fragen beide Editoren
  `editedRecurrence` (`shared/recurrence.ts`): die Regel aus dem Formular, die
  Ausnahmen und die Zone der Serie. Ein Termin, der im Editor zur Serie wird,
  bekommt die Zone des Geräts wie eine neue Serie. Eine Serie, die schon ohne
  Zone wiederholt, bleibt unverändert: eine verlorene Zone und eine gewollte
  UTC-Serie sehen gleich aus, und das Stempeln verschöbe späte Termine auf einen
  anderen Wochentag (Nachprüfung von #68; die zuerst gebaute Reparatur beim
  Speichern ist deshalb zurückgenommen, Toni 2026-09-14). Die Hilfe beschreibt
  das Verrutschen unter „Fehlersuche & Protokolle".
  ↳ **Eine Zonen-Regel im Kern (Zeitzonen-Auswahl, Stufe 2, 2026-09-14):**
  Welche gespeicherten Zonennamen eine Zone sind, entschieden drei Stellen
  verschieden:
  - Die Ansichten nahmen jeden Namen außer `UTC`.
  - Die Erinnerungen trimmten den Namen und kannten ihn nur in exakter
    Schreibweise.
  - Eine neue Serie bekam, was das Gerät meldete.

  Jetzt antwortet `cal_core::series_clock` einmal für alle (Toni, 26a). Ein Name
  zählt, wenn tzdata ihn kennt und er keiner der 18 UTC-Namen ist.
  Groß-/Kleinschreibung (ASCII) spielt keine Rolle, getrimmt wird nichts.
  - **Namen:** `cargo xtask tz-list` erzeugt sie aus den tzdata-Dateien von
    chrono-tz. CI prüft das mit `--check`.
  - **Fixture:** `seriesClock.json` lesen Kern, WebAssembly-Tür und Handy-Tür.
  - **`eventOccurrences.json`:** Zwei Zeilen stimmen jetzt zwischen Ansichten und
    Erinnerungen überein. Neu sind vier Zeilen für UTC-Namen und einen Offset.
    Die Tabelle hat jetzt 82 Zeilen, 20 davon weichen ab. Ein Wächter
    (`AGREEING` in `reminders.rs`) hält die Zonen-Zeilen übereinstimmend.
  - **Offen:** CalDAV und Graph geben den gespeicherten Namen weiter direkt an
    chrono-tz, das nur die exakte Schreibweise kennt. Eine Serie mit
    „europe/berlin“ schreiben sie als UTC-Zeit. Das bestand schon vorher.

  ↻ **Mit dem nächsten Handy-Build prüfen.** Keine CI sieht, ob `mobile/index.ts`
  die Tür installiert.
  - Die App startet ohne „series clock rules used before
    installSeriesClockRules()“.
  - Eine Serie mit `Etc/UTC` zeigt dieselben Zeiten wie vorher.
  - Mit dem Gerät auf UTC wird eine neue Serie ohne Zone gespeichert.
  - In einem Dev-Build `resolvedOptions().timeZone` für UTC, Indien und die
    Ukraine auf iOS und Android notieren (DESIGN-series-time-zone.md,
    „Ungeprüft“).
  ↳ **Die Weltliste im Kern (Zeitzonen-Auswahl, Stufe 3, 2026-09-15):**
  `cal_core::zone_list` benennt die 312 Zonen (Stadt, Gebiet, Region), sucht
  darin und ordnet eine gespeicherte Zone oder die Gerätezone ein. Toni hat
  entschieden (29a bis 32a):
  - sortiert wird nach der Normalzeit ab heute, fest;
  - „+5“ findet nur die volle Stunde;
  - gebaut wird mit tzdata 2025b;
  - der Versatz lautet „UTC−04:00“ mit echtem Minuszeichen.

  Der Generator schreibt jetzt zu jedem Namen seine Art, gelesen aus den
  Abschnitten von tzdatas `backward`. Namen ohne Schrägstrich (`EST5EDT`) kommen
  nie in die Liste.

  Namen und Suche laufen im normalen Kern, auf dem Desktop über WebAssembly. Nur
  die Versätze brauchen chrono-tz (Feature `zones`): auf dem Desktop ein
  Tauri-Befehl, auf dem Handy eine synchrone Tür. CI prüft, dass chrono-tz nicht
  ins WebAssembly-Modul gerät. Die Fixture `timeZoneFilter.json` lesen Kern,
  Handy-Tür und WebAssembly-Tür.
  ↻ **Mit dem nächsten Handy-Build:** die `.so` frisch erzeugen (vier neue
  Funktionen), dazu die Liste und die Suche einmal auf dem Gerät aufrufen.
  🚩 **Aktualität der Zeitzonendaten** (32a): Marokko ab 20.09.2026, Vancouver,
  Edmonton und Inuvik ab 01.11.2026, Chisinau stellt eine Stunde später um.
  Eigene Aufgabe.
  ↳ **Die Exchange-Zonentabelle aus CLDR (Zeitzonen-Auswahl, Stufe 4,
  2026-09-15):** `cargo xtask windows-zones` erzeugt die Windows-Zonennamen für
  Exchange aus der CLDR-Datei `windowsZones.xml` (release-48-2). Sie liegt mit
  Lizenz und Prüfsumme in `crates/adapter-ews/cldr/`. Die handgeschriebene
  Tabelle ist weg.
  - 311 der 312 Zonen haben einen Windows-Namen, Antarctica/Troll nicht.
  - Scoresbysund, Casey und Vostok stehen in CLDR unter einer Windows-Zone mit
    anderer Uhr. Ein Uhr-Wächter im Generator findet sie, und sie gelten wie
    Troll als nicht speicherbar (22a).
  - Geschrieben werden jetzt 308 gelistete Zonen, bisher waren es 138.
  - Die alte Tabelle hatte drei falsche Zeilen: Chihuahua, Almaty und Beirut
    mit dem erfundenen Namen „Lebanon Standard Time“. 31 Windows-Namen las sie
    gar nicht.
  - Der Sync-Token, den der Host speichert, trägt einen Fingerabdruck der
    Zonen-Übersetzung. Nach einem Update gibt der Adapter deshalb alle
    zwischengespeicherten Exchange-Termine einmal neu aus.

  Toni hat entschieden (38a, 39, 40a): Live-Test vor dem Merge an seinem eigenen
  Exchange-Server, Lizenzhinweis bei den Daten.
  Live-Test Runde 1 an Tonis Exchange 2019 (15.09.2026): Der Server kennt alle
  CLDR-Namen außer São Tomé, ein unbekannter Name lässt das Speichern scheitern,
  ein Update ohne Zone behält die Zone, eine Zone nach Beginn und Ende verschiebt
  die Zeitpunkte, eine Serie ohne Zone kommt als Greenwich mit der Endzone
  `tzone://Microsoft/Utc` zurück, und eine ganztägige Serie mit Zone landet auf
  zwei Tagen. Toni hat daraufhin entschieden:
  - 41a: Der Adapter fragt jeden Server einmal nach seinen Zonennamen und
    schreibt nur diese;
  - 42a: Für ganztägige Serien kommt erst ein zweiter Test;
  - 43b: Die Endzone `tzone://Microsoft/Utc` heißt keine Zone.

  41a und 43b sind gebaut: Der Adapter fragt vor dem ersten Speichern einer
  Serie mit Zone `GetServerTimeZones` und schickt einen unbekannten Namen ohne
  Zone. Er liest die Endzone mit. Weil sein Speicher die Endzone nicht kannte,
  liest er jeden Exchange-Kalender nach dem Update einmal von vorn.

  Live-Test Runde 2 (15.09.2026), Wochentage in Outlook und in Aperio
  abgelesen:
  - Ganztägige Serien ohne Zone und mit UTC speichert Exchange auf
    UTC-Mitternacht, und Outlook zeigt sie am richtigen Tag.
  - Eine Zone bei einer ganztägigen Serie verlängert sie: Mit Los Angeles
    werden es zwei Tage.
  - Eine kaputte Serie lässt sich mit UTC nur reparieren, wenn die Zone vor
    Beginn und Ende kommt. Kommt sie danach, werden es drei Tage.
  - Eine echte Greenwich-Serie behält die Endzone Greenwich (43b bestätigt).
  - Aperio selbst zeigt alle diese Serien einen Tag zu spät. Das ist ein
    Lesefehler in Aperio. Auch der „Dienstag“ aus Runde 1 war in Aperio
    abgelesen.

  Die Analyse danach hat gezeigt:
  - Der Lesefehler steckt in Aperios Kern, nicht nur bei Exchange. Aperio
    wiederholt ganztägige Serien in UTC statt an Kalendertagen. Östlich von
    UTC landet ein genannter Wochentag deshalb einen Tag zu spät. Das betrifft
    lokale Kalender, CalDAV, Google und Exchange, auf Desktop und Handy, und
    war schon auf main.
  - Ändert Aperio einen ganztägigen Exchange-Termin, der schon eine Zone trägt,
    schickt es UTC-Mitternacht ohne Zone. Exchange rundet dann in seiner Zone.
    Das ist abgeleitet, nicht gemessen.

  Toni hat entschieden (15.09.2026):
  - 46a: In PR #75 kommt nur der Schutz dazu: Eine ganztägige Serie schreibt
    nie eine Zone.
  - 47a: Ändert Aperio einen ganztägigen Exchange-Termin mit Zone, gehen Beginn
    und Ende auf Mitternacht in der gespeicherten Zone, und die Zone bleibt
    unberührt.
  - 48a: Der Lesefehler wird ein eigener PR mit einer Kern-Regel: Eine
    ganztägige Serie wiederholt sich an den Kalendertagen des Geräts.
  - 49a: Es gibt eine gezielte dritte Runde des Live-Tests.
  - 51a: Toni legt die Serien S1 bis S3 und die Termine T1 und T2 selbst in
    Outlook im Web an.
  - 52a: Toni macht das Vorkommen von S3 am 26.10. selbst in Outlook zur
    Ausnahme.
  - 53a: Toni meldet eine kurze Liste zurück: die letzte Zeile des Skripts,
    welches Outlook und welcher Aperio-Stand, den Tokio-Termin in Outlook, die
    Termine, die das Skript nennt, und vier Blicke in Aperio.

  Der Schutz aus 46a ist gebaut. Die Regel steht im Kern
  (`written_series_zone`), und Exchange, Microsoft 365, Google und CalDAV
  fragen sie. Exchange fragt für eine ganztägige Serie auch keine Serverzonen
  mehr ab. Microsoft 365, Google und CalDAV schrieben ganztägige Tage schon
  vorher ohne Zone.
  🚩 **Ganztägige Serien an Kalendertagen wiederholen** (48a), eigener PR.
  **Live-Test Runde 3** (49a), gelaufen am 18.09.2026:
  - eine ganztägige Serie und ein ganztägiger Termin aus Outlook, geändert
    nach der Regel aus 47a und nach der heutigen;
  - ein Termin in der Zone Tokio;
  - eine tägliche ganztägige Serie;
  - eine geänderte Ausnahme.

  Die Anfragen schreibt der ignorierte Test `live_test_requests_round_3` im
  EWS-Adapter aus Aperios eigenem Lese- und Schreibpfad. Ein Skript außerhalb
  des Repositorys liest Tonis Outlook-Termine zuerst. Weicht ihre Form ab,
  stoppt es, bevor es schreibt. Toni legt die Termine in Outlook selbst an
  (51a und 52a) und meldet die kurze Liste (53a).

  Ergebnisse (Einzelheiten in DESIGN, Stufe 4, „Gemessen“):
  - Regel 47a lässt Outlook-Termine auf ihrem Tag. Das gilt für die Serie,
    für einen Einzeltermin und für eine Ausnahme.
  - Die heutige Regel macht aus einem ganztägigen Outlook-Termin zwei Tage,
    und Exchange ändert dabei die Zone. Das betrifft auch main.
  - Exchange rundet in der gespeicherten Zone. 47a bleibt deshalb so.
  - Eine tägliche ganztägige Serie beginnt wegen des Startdatums einen Tag
    früher, am Sonntag.

  ↻ **Eine Ausnahme in Exchange zu ändern scheiterte**, behoben in PR #77.
  Aperio schickte beim Ändern eines einzeln geänderten Termins
  `DeleteItemField calendar:Recurrence`. Exchange lehnt das an einer Ausnahme
  mit `ErrorInvalidPropertyDelete` ab, und die ganze Änderung scheiterte.
  Jetzt bleibt das Löschen bei einer Ausnahme weg. Die zurückgegebene Ausnahme
  behält ihre Override-Id. Auf dem Handy wirkt das mit dem nächsten Build.
  🚩 **Ausnahmen zeigen den Betreff der Serie.** Aperio baut eine Ausnahme aus
  der Serie und liest ihren eigenen Betreff, Ort und Text nicht nach. Das ist
  eine eigene, spätere Aufgabe außerhalb der Zeitzonen (58a).

  Toni hat die Reihenfolge festgelegt (57a):
  1. der Ausnahme-Fehler als kleiner eigener PR;
  2. der Lesefehler im Kern (48a);
  3. der Exchange-Schreiber mit 47a und dem Datumsfehler zusammen;
  4. danach die weiteren Stufen des Entwurfs.

  Die Prüfung des Ausnahme-Fixes hat zwei ältere Fehler gefunden. Toni hat sie
  direkt danach eingereiht, noch vor 48a (61a):
  ↻ **A: Das Handy ändert eine Exchange-Ausnahme nicht direkt**, behoben in
  PR #78. Für „nur dieses Vorkommen“ öffnete es den Serienkopf. Dann löschte es
  die Ausnahme und legte einen neuen Einzeltermin an, oder das Speichern
  scheiterte. Jetzt öffnet es die Ausnahme selbst und ändert sie direkt, wie der
  Desktop (`isProviderOverride` in `shared/recurrence.ts`). Zu- und Absagen gehen
  wie auf dem Desktop an die Serie. Bei CalDAV und Google braucht das PR #80.
  Wirkt mit dem nächsten Handy-Build.
  🚩 **Klang eines geänderten Vorkommens: Desktop und Handy uneins.** Die
  Erinnerungen einer Ausnahme suchen ihren Klang unter der Id der Ausnahme
  (`host-core/src/reminders.rs`). Das Handy speichert ihn dort, der Desktop
  unter der Serie (`EventDialog.tsx`, `seriesIdOf`). Ein am Desktop gewählter
  Klang erreicht die Ausnahme also nicht. Eine Regel festlegen, am besten im
  Kern (`sound.rs`: erst die Ausnahme, dann die Serie).
  ↻ **B: Verschieben „nur dieses Vorkommen“ hinterlässt ein Duplikat**, behoben
  in PR #79. Auf dem Desktop legte Verschieben oder Ziehen einer geänderten
  Ausnahme einen neuen Einzeltermin an und nahm den Platz aus der Serie heraus;
  die Ausnahme selbst änderte es nie (`moveActions.ts`). Die Suche nach dem
  Vorkommen in Exchange verglich dabei `Start` statt `OriginalStart`, sodass
  eine um mehr als sechs Stunden verschobene Ausnahme doppelt stehen blieb.
  Jetzt verschiebt Ziehen „nur dieses Vorkommen“ die Ausnahme selbst, wie der
  Dialog sie speichert, und die Exchange-Suche vergleicht `OriginalStart`. Das
  gilt auch für das Löschen einer weit verschobenen Ausnahme und für den Umzug
  in einen anderen Kalender, auf Desktop und Handy. Bei CalDAV und Google
  braucht das Ziehen PR #80; bei Exchange löst #80 eine Ausnahme, die nicht
  über ein Nachbar-Vorkommen darf, aus der Serie (64a).
  ↻ **Eine geänderte Ausnahme traf bei CalDAV und Google die ganze Serie**
  (Prüfung von #78; 62b, 63a), behoben in PR #80. Die Override-Id
  `{Serie}::rid::{Platz}` konnte nur Exchange adressieren. Bei CalDAV/iCloud
  überschrieb „nur dieses Vorkommen“ speichern die ganze Serie mit einem
  einzelnen Termin, und ein Kalenderwechsel löschte die Serie. Bei Google
  scheiterte das Speichern, und ein Kalenderwechsel ließ ein Duplikat zurück.
  Der Desktop hatte das seit 4179edfe. Jetzt:
  - CalDAV schreibt nur den Block der Ausnahme und legt den Rest der Ressource
    byte-genau zurück. Ein einzelnes Vorkommen löschen behält die übrigen
    Ausnahmen und ihre Zeitzonen und nimmt die Ausnahme im Platz mit.
  - Google sucht die Instanz über ihren Platz (`originalStart`). Dadurch findet
    das Löschen auch eine weit verschobene Ausnahme, und es trifft in einer
    täglichen Serie nie mehr das Nachbar-Vorkommen.
  - Löschen über eine Override-Id nimmt bei allen dreien nur dieses Vorkommen
    heraus.
  - Exchange löst eine Ausnahme, die es nicht über ein Nachbar-Vorkommen
    schieben will, selbst aus der Serie (64a).
  Wartet auf den Live-Test: Googles Schreibweise von `originalStart` für
  ganztägige Serien ist nicht dokumentiert; iCloud; die Exchange-Grenze.
  🚩 **CalDAV: Ändern des Serienkopfs schreibt nur den Kopf.** `put_master_only`
  verlässt sich darauf, dass der Server die übrigen Ausnahmen der Ressource
  wieder anhängt (Kommentar in `events.rs` seit 27f9fcd1). Das ist nicht
  belegt; ein Server, der sich an RFC 4791 hält, verwirft sie. Live messen
  (iCloud, Nextcloud) und bei Bedarf auf das Zusammenführen umstellen, das
  #80 für Ausnahmen gebaut hat.
  **Live-Test Runde 4** (65, 66b), gelaufen am 19.09.2026 auf dem Desktop mit
  iCloud und Exchange. Das Handy war nicht dabei, Toni hat Aperio dort noch nie
  benutzt (69).
  - ↻ Der Build startete nicht: Die CSP des Produktionsbuilds verbot das
    WebAssembly der Kernregeln. Behoben in PR #81.
  - D3: Das Folge-Vorkommen einer ganztägigen iCloud-Serie stand am 26. statt
    am 27.10. Das ist der Lesefehler aus 48a.
  - ↻ D6, D7 und H2: Ändern eines Outlook-Termins scheiterte mit
    `ErrorInvalidRecipients`. Aperio las den Organisator als Gast und wollte
    ihn benachrichtigen. Behoben in PR #83.
  - ↻ D8: Das gelöste Vorkommen öffnet Outlook als „Besprechung“. Vermutlich,
    weil es den Organisator als Gast mitnahm. PR #83 nimmt ihn heraus. In der
    nächsten Runde nachsehen.
  - D9: Toni hat den Einzeltermin in Outlook gelöscht, und Aperio hat ihn
    richtig entfernt. Das Löschen aus Aperio heraus ist noch ungeprüft.
  - ↻ H1: „Anwenden auf“ stand da, aber nicht in der Tab-Reihenfolge. Behoben
    in PR #82 (68): Der Titel nennt den Umfang, und ein schreibgeschütztes Feld
    liegt in der Tab-Reihenfolge, auch für die ganze Serie.

  ↻ **Der Organisator galt als Gast** (67a, 70a bis 72a), behoben in PR #83.
  Die Regel steht einmal im Kern (`cal_core::attendee`, DESIGN §7.3):
  - Beim Lesen nimmt jeder Adapter die Zeile des Organisators aus den Gästen.
  - Benachrichtigen darf nur, wer den Termin organisiert. Das sagt ein
    ausdrückliches Merkmal des Anbieters, sonst gilt der Termin als fremd
    organisiert.
  - Beide Hosts schreiben die Gästeliste nur, wenn sie sich gegenüber dem
    Cache geändert hat (Exchange, Google, Microsoft 365).
  - Abgeleitete Termine tragen den Organisator ihrer Quelle mit.
  - `CACHE_GENERATION` 2: Der erste Start liest jedes externe Konto einmal neu
    ein, Exchange vollständig.
  Die Prüfung von #83 fand sechs Lücken, alle im selben PR behoben:
  - iCloud erkannte nur die erste Adresse des Kontos als eigene. Jetzt zählt
    jede Adresse aus `calendar-user-address-set`.
  - Scheiterte die Adressabfrage an einem Netz- oder Serverfehler, blieben
    eigene iCloud-Besprechungen dauerhaft „fremd“. Jetzt wird die Abfrage
    wiederholt, und solange sie scheitert, scheitert das Lesen.
  - Sagte der Anbieter „nicht der Organisator“, nannte aber keine Adresse,
    galt der Termin als eigener. Jetzt zählt die Antwort des Anbieters zuerst.
  - Nach einem neuen Exchange-ChangeKey fand der Cache-Vergleich den Termin
    nicht mehr und schrieb die Liste doch. Jetzt findet er ihn.
  - Webex konnte die Gäste einer fremden Besprechung anschreiben. Jetzt lädt
    es dort niemanden ein.
  - Den letzten Gast zu entfernen kam beim Anbieter nicht an. Jetzt wird die
    Liste geleert, und „Teilnehmer benachrichtigen“ bleibt für die Absage
    stehen (74a).
  Gemessen in Live-Runde 5 (D10): Exchange nimmt die Absage an den letzten
  entfernten Gast an und verschickt sie.
  Auf dem Handy wirkt das mit dem nächsten Build. Die `.so` muss frisch
  erzeugt werden; neue FFI-Funktionen gibt es nicht, nur neue Felder im JSON.
  **Live-Test Runde 5**, gelaufen am 19.09.2026 auf dem Desktop (Build
  dd4c4440):
  - D7 bis D10 und H1b wie erwartet. D10: Exchange schickt dem letzten
    entfernten Gast die Absage.
  - D6 und H2: Speichern klappt, aber Aperio zeigt einzeln geänderte
    Exchange-Termine mit dem Titel der Serie, und jedes Speichern schreibt
    Titel, Ort, Text und Erinnerung der Serie zurück. Toni hat das live
    bestätigt. Das ist 58a, als Nächstes nach dem iCloud-Fix (78a).
  - E1: Toni speicherte versehentlich das iCloud-Original einer eigenen
    Besprechung; der Gast verschwand, und iCloud sagte ihm ab. E2 fiel damit
    aus.
  ↻ **Ein iCloud-Termin verliert seine Gäste** (E1), behoben in PR #84.
  Aperio baute den VEVENT neu und schrieb `ORGANIZER` und `ATTENDEE` nur beim
  Benachrichtigen; ohne `ORGANIZER` sagt ein RFC-6638-Server die Besprechung
  allen ab. Zudem erkannte Aperio Toni nicht als Organisator, weil iCloud ihn
  per Principal-Pfad nennt (Messung M1), also fehlte der Schalter. Jetzt:
  - Aperio erkennt das Konto an allen Einträgen seiner
    `calendar-user-address-set` und am `EMAIL` einer Zeile.
  - Jedes Speichern liest zuerst die Kopie des Servers und trägt die Zeilen
    der Besprechung wörtlich weiter; nur eine geänderte Gästeliste ändert
    Zeilen. Eine Serie behält ihre Ausnahmen Byte für Byte, „nur diesen
    Termin löschen“ fügt eine einzige `EXDATE`-Zeile ein, und ein Speichern
    ohne Änderung schickt nichts.
  - Bei iCloud und Microsoft 365 zeigen Editor und Löschdialoge statt einer
    Wahl den Satz, wer die Teilnehmer informiert (76a, 80a, 82b), auf Desktop
    und Handy, auch nach der Entfernen-Taste in Woche, Tag, Monat und Agenda
    (ein gemeinsamer `DeleteEventConfirm`). Die Löschdialoge fragen
    „organisiert das Konto?“ jetzt über `organized_elsewhere` statt über einen
    Adressvergleich. Der Satz beim ersten Gast kommt zusammen mit „X
    hinzugefügt“ in einer Ansage.
  - `CACHE_GENERATION` 3.
  Wartet auf den Live-Test mit „Aperio R6 eigene“: Titel ändern, einen Gast
  hinzufügen und entfernen, den letzten entfernen, eine Serie ändern und ein
  Vorkommen löschen.
  ↻ **Fremde Einladungen sind schreibgeschützt** (77a, 83b, 84a), gestapelt
  auf #84 und zusammen mit #83 zu mergen. Kalender auf einem Server mit
  Terminplanung tragen `invitations_reply_only`; beide Editoren zeigen so
  eine Besprechung schreibgeschützt (Antwort, eigene Erinnerungen, Klang,
  Farbe, Beitreten, Verfügbarkeit, Löschen bleiben), CalDAV schreibt in so
  einem Termin nur die VALARMs und lässt jede andere Zeile Byte für Byte
  stehen, der Löschdialog sagt „Der Organisator bekommt eine Absage“, die
  Wiederholung steht als Satz da (Kern-Regel mit Prüf-Datei, EN/DE), und
  jede Ablehnung des Servers wird in beiden Oberflächen zu einem Satz.
  `CACHE_GENERATION` 4. Offen bleibt „nur diesen Termin“ einer iCloud-Serie
  als echte Ausnahme (79b, PR 3).
  🚩 **Nach dem Live-Test zu klären (PR 2):**
  - Nimmt iCloud es an, wenn ein Gast `X-APPLE-DEFAULT-ALARM` entfernt, und
    setzt es den Standard-Hinweis danach wieder? Darauf ruht das Ersetzen der
    Wecker.
  - Schickt iCloud dem Organisator wirklich eine Absage, wenn ein Gast seine
    Kopie oder ein einzelnes Vorkommen löscht? RFC 6638 führt `EXDATE` nicht
    unter den erlaubten Änderungen eines Gastes; Apple macht es trotzdem
    (86a: der Dialog sagt es schon jetzt).
  - `EXDATE` einer Zeit-Serie wird weiter als UTC geschrieben, nicht mit
    `TZID`; nur die Ganztagsform ist jetzt richtig.
  - `respond_to_event` kennt nur die eine Adresse aus der Discovery, keine
    Aliase, und gibt das neue ETag nicht zurück. Deshalb braucht ein Speichern
    direkt nach dem Antworten einen frischen Stand: sonst meldet der Adapter
    einen Konflikt, weil die Antwort das ETag schon weitergedreht hat.
  - Ein CalDAV-Server ohne Terminplanung bleibt wie in #84: Der Editor sperrt
    nicht, und `plan_block` lehnt eine geänderte Gästeliste ab.
  - Auf einem farbfähigen Server versteckt der Editor die Farbe einer fremden
    Einladung, statt sie geräte-lokal zu halten.
  - `WKST` zählt nur in der einen Form, in der es die Wochen verschiebt.
  - Eine Serie mit Zone wird in Wanduhr-Zeit ausgeklappt, ihr `UNTIL` bleibt
    aber ein echter Zeitpunkt (`shared/recurrence.ts`, `zonedOccurrences`).
    Ein Abendtermin am Tag der Grenze fällt dadurch um den Zonen-Versatz
    heraus — im Kalender und im Satz „letzter Termin am …“ gleichermaßen.
    Gefunden bei der Prüfung von #85; der Satz sagt, was die Ansicht zeigt.
  - Die Wiederholungs-Zusammenfassung erscheint auch im normalen Editor, wenn
    die gespeicherte Regel nicht die ist, die der Picker zurückbauen würde
    (87b, gebaut: `pickerMisreadsRule`). Die Bedienelemente selbst halten
    solche Regeln weiter nicht; wer eines anfasst, speichert die Regel des
    Pickers.
  - iOS-Volltastatur erreicht die schreibgeschützten Zeilen des Handys nicht;
    VoiceOver und TalkBack erreichen sie.
  🚩 **Eigener PR nach dem Stapel (89a):** Die erzeugten Kotlin-Bindings
  (`mobile/modules/cal-ffi/android/src/main/java/uniffi/cal_ffi/cal_ffi.kt`)
  nicht mehr einchecken. Die Android-CI erzeugt sie ohnehin vor jedem Bau neu,
  und die Swift-Bindings liegen auch nicht im Repo. Umzustellen sind: Datei
  loeschen und ignorieren, der lokale Android-Lauf erzeugt sie neben dem
  frischen `.so`, und `mobile/scripts/check-ffi-bridges.mjs` erzeugt sie selbst,
  statt die eingecheckte Fassung zu vergleichen. Anlass: Eine veraltete Datei
  faellt sonst erst beim Checksum-Fehler auf dem Geraet auf.
  🚩 **Offen nach #84:**
  - Verschieben einer iCloud-Besprechung in einen anderen Kalender bleibt
    Anlegen und Löschen: Die Gäste bekommen eine Absage, die Kopie hat keine
    Gäste (81b). Richtiges Verschieben (WebDAV MOVE) erst nach einer Messung.
  - `SEQUENCE` geht wörtlich zurück; ob iCloud ihn selbst hochzählt, ist nicht
    gemessen. Ebenso `SCHEDULE-AGENT=CLIENT` (ein stilles Speichern bleibt
    unmöglich, solange Aperio kein iTIP verschickt).
  - Ein CalDAV-Server ohne Terminplanung speichert neue Gäste nicht als Daten.
  - Die `EXDATE` einer ganztägigen Serie wird als UTC-Zeitpunkt geschrieben,
    nicht als Datum.
  - Der VTODO-Neubau in `tasks.rs` verwirft `ORGANIZER` und `ATTENDEE` ebenso.
  - Microsoft 365: dass Graph bei jeder Änderung und beim Löschen mailt, steht
    in Microsofts Doku; gemessen ist es nicht.
  - Eigenschaften, die Aperio nicht kennt (etwa `X-APPLE-TRAVEL-ADVISORY-…`),
    gehen beim Neubau eines Termins verloren; das gehört zu 58a.
  🚩 **Den Organisator zeigen** (73a), eigene Aufgabe: eine Zeile
  „Organisiert von …“ im Editor, die Suche über `$.organizer` und der
  Organisator in der Verfügbarkeitsprüfung. Seit PR #83 fragt die Prüfung ihn
  nicht mehr ab, weil er kein Gast mehr ist.
  🚩 **Update-Regel für ganztägige Exchange-Termine** (47a) und
  **Datumsfehler** (Startdatum, Wochentag, Monatstag und Monat aus dem
  UTC-Datum): eigene PRs nach Runde 3.
  🚩 **Open-Source-Hinweise** in der Desktop- und der Handy-App (40a): CLDR und
  die eingebauten ICU4X-Bibliotheken nennen.
  🚩 **Wenn der EWS-Adapter das Repository verlässt,** müssen `cldr/`, die
  erzeugte Tabelle und `cargo xtask windows-zones` mitgehen. Sonst fällt die
  Prüfung still weg.
  ↻ **Mit dem nächsten Handy-Build:** die `.so` frisch erzeugen. Rust im
  EWS-Adapter hat sich geändert, neue FFI-Funktionen gibt es nicht.
- [ ] Schritt 3: Darstellung (`dayGridLayout`, `titleSuggestions`,
  `eventDateTime`, `taskRecurrence`, `quickDates`, `eventKey`) bleibt pro
  Oberfläche und darf auseinanderlaufen.


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
- [~] E-Mail-Reminder: UI-Option + (Adapter-)Versand — lokaler Scheduler überspringt sie derzeit bewusst.  
  ↳ Adapter-Hälfte für Google + CalDAV FERTIG (round-trippt als providereigener E-Mail-Alarm — Aperio verschickt selbst nie Mail, genau wie §14 es verlangt). Die UI-Option fehlt auf beiden Oberflächen.
- [-] Per-Vorkommen-Sound-Override, ohne das Vorkommen aus der Serie herauslösen zu müssen.  
  ↳ HINFÄLLIG: §14.4 wurde am 2026-06-04 (27459041) umgeschrieben — der Item-Override ist per Design serien-gebunden. Eine Vorkommen-Ebene wäre ein NEUER Wunsch, keine Lücke.

---

### B8 · 🚩 Das macOS-Artefakt ist ein Entwicklungs-Build `[ ]`
Gefunden bei der Prüfung von #81. `build-artifacts.yml` baut macOS mit
`cargo build --workspace --release` für beide Architekturen und fügt sie mit
`lipo` zusammen, am Tauri-CLI vorbei. Nur das CLI schaltet das Tauri-Feature
`custom-protocol` ein. Ohne es kompiliert Tauri im Entwicklungsmodus: Die
Oberfläche aus `dist/` wird nicht eingebettet, und das Fenster zeigt auf die
`devUrl`. So ein Build zeigt nichts an und prüft auch nie die CSP.
Vorschlag aus der Prüfung:
- In `src-tauri/Cargo.toml` ein Feature `custom-protocol = ["tauri/custom-protocol"]`
  anlegen.
- Beide `cargo build` in `build-artifacts.yml` mit `--features
  aperio/custom-protocol` aufrufen.
- Den Kommentar dort korrigieren.
- Eine CI-Probe ergänzen, dass die Binärdatei die Oberfläche enthält.
Windows und Linux laufen über `npm run tauri -- build` und sind nicht
betroffen.

## 🟡 C. Bewusste Deferrals (dokumentiert, niedrigere Priorität)

### C1 · Task-Recurrence in EWS & Todoist (§9.1)
- [x] EWS: Recurrence lesen/schreiben (aktuell beim Schreiben verworfen, „Phase 6f.2-Follow-up")  
  ↳ Erledigt über die geteilte `<t:Recurrence>`-Maschinerie der Kalenderseite; EWS' fehlendes Jahres-INTERVAL ist als eingeschränkte Recurrence-Capability deklariert, der Editor graut es aus.
- [~] Todoist: `due_string` ↔ `TaskRecurrence` (aktuell out of scope)  
  ↳ Backlog-/On-Demand-Regeln round-trippen inzwischen verlustfrei über den §9.12-Extras-Block; es fehlt die einfache terminierte Regel — und ihr Fehlen ist als Capability deklariert, statt still zu scheitern.

### C2 · Task-Detailpunkte (§9 — geringere Konfidenz, in Agent-Notizen erwähnt)
- [x] Recurrence-Template nach Abschluss generieren (für alle Adapter out of scope)  
  ↳ Ins Gegenteil gedreht: der Spawner läuft für JEDEN Adapter. Zwei Spiegelzweige — Provider kann die Regel nicht → die nächste Instanz wird erzeugt; Provider wiederholt selbst → terminaler Completion-Record, damit die erledigte Runde sichtbar bleibt.
- [~] TaskView-Filter-UI  
  ↳ Gruppierung (Status | Liste) steht auf beiden Oberflächen, Listen-Filter über Seitenleiste + Backlog-Dialog. Es fehlt der Filter nach Status / Priorität / Zeitraum.
- [~] Move/Copy-Prompts (Subtasks mitnehmen / Recurrence-Instanz vs. Regel / Reminder-Kompatibilität)  
  ↳ Geteiltes Urteil: Subtasks per ENTSCHEIDUNG gelöst (Kinder reisen immer, der Dialog sagt es) — der Prompt ist hinfällig. Recurrence-Scope für Termine gebaut. Offen bleibt die Reminder-Kompatibilität.
- [x] `role="group"` + `aria-label` an Subtask-Eltern (a11y)  
  ↳ Als echter W3C-Baum gelöst statt als angeklebte Gruppe: role=tree / treeitem / verschachtelte role=group, eingeklappt komplett ausgeblendet statt nur per CSS.

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
