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
- [ ] Zwei Verträge, die vor jedem Umzug festzuklopfen sind: (a) der Kern gibt
  **i18n-Schlüssel + Variablen** zurück, nie fertigen Text (Vorbild
  `cal-core/src/conferencing.rs:70`); (b) der Kern liest **nie** die Uhr oder
  die Gerätezone — Tageschlüssel und Offset sind immer Parameter.
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
