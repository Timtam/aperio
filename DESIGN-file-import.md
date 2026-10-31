# Datei-Import (zuerst ICS)

Ein Bogen zwischen PR 7 (#128, pausiert) und PR 9. Er bringt Termine aus einer
`.ics`-Datei nach Aperio, auf beiden Seiten.

## Entscheidungen (Toni, 2026-10-25)

1. **Keine Dateiverknüpfung** an Mac und Windows, vorerst. Importiert wird über
   einen Eintrag in Aperio; am Handy auch über „Teilen“ und „Öffnen in“.
2. **Ein Termin in der Datei** öffnet den Termin-Editor im Anlegen-Modus,
   vorausgefüllt aus der Datei. Das Kalender-Feld ist die Zielauswahl; der
   Schalter „Benachrichtigen“ gilt wie bei jedem neuen Termin.
3. **Mehrere Termine** zeigen erst eine Liste mit Vorschau. Ein Eintrag öffnet
   den Editor vorausgefüllt; „Alle in Kalender … importieren“ legt die übrigen
   an. Schon vorhandene (gleiche UID im Ziel) sind markiert und abgewählt; wer
   sie anwählt, bekommt sie als Kopie.
4. **Teilnehmer bleiben.** Ein Termin mit Teilnehmern, den du selbst
   organisierst, ist wie ein neuer Termin: Die bestehenden Regeln gelten
   (iCloud schreibt Teilnehmer nur beim Benachrichtigen). Eine Einladung, die
   jemand anderes organisiert, behält Organisator und Teilnehmer, still, je
   Anbieter auf seinem Weg (nach dem Live-Test I0, siehe I6).
5. **Umfang zuerst:** Termine samt Serien und ihren gelöschten Terminen. Ein
   geänderter Einzeltermin einer Serie wird ein eigener Termin, die Serie
   bekommt an seinem Platz eine Lücke. Aufgaben (VTODO) kommen später.
6. **Einträge:** am Desktop „Importieren …“ in der Werkzeugleiste (Gruppe
   Neu) mit Strg+O, dazu eine Datei, die auf das Fenster gezogen wird; am
   Handy „Termine importieren …“ in der Kalenderliste neben „Neuer Kalender“.
7. **Adapter melden Dateiformate an** (Toni): Der Host verdrahtet keinen
   Parser. Ein Adapter sagt im Manifest, welche Dateien er lesen kann, und
   liest sie selbst; `.ics` meldet der iCal-Adapter an, der die Abbildung aus
   `ical-mapping` mit dem CalDAV-Adapter teilt.

8. **Reihenfolge** (Toni, 2026-10-26): erst der Desktop (I3) mit einer
   vorläufigen Regel, dann I4, I6, dann das Handy. Bis I6 kommt eine
   Einladung, die einen Organisator nennt, ohne Teilnehmer und Organisator
   in den Editor, und der Editor sagt das; bis I4 werden Doppelte nicht
   erkannt.

## Was gemessen ist und was nicht

- iCloud verschickt selbst Einladungen, sobald ein Termin mit Teilnehmern und
  dem Konto als Organisator ankommt; Aperio schreibt dort Teilnehmer daher nur
  beim Benachrichtigen (gemessen, Entscheidungen 76a, 80a).
- Eine von Apple importierte Einladung trägt `SCHEDULE-AGENT=CLIENT` am
  Organisator und `NONE` an der eigenen Zeile; iCloud schickte dann weder die
  Antwort auf eine Zusage noch die Absage beim Löschen (Live-Runde 6). Ob das
  Anlegen selbst still bleibt, ist ungemessen; RFC 6638 §7.1 sagt ja.
- iCloud (Live-Lauf 100518, 2026-10-26, Kalender „Privat“; Organisator =
  Tonis Exchange-Adresse, damit jede Antwort dort ankäme): Eine fremde
  Einladung per `PUT` legt iCloud an (201) und speichert sie wörtlich, mit
  `SCHEDULE-AGENT=CLIENT` am Organisator und `NONE` an der eigenen Zeile wie
  ohne. Nichts wurde verschickt, auch nicht bei Aperios Zusage mit Antwort
  und beim Löschen mit Absage, und auch nicht in den Gegenproben ohne
  Marker (kein `SCHEDULE-STATUS`, keine Mail bei Exchange, gmx oder am
  iPhone). iCloud verschickt für eine Kopie, die ein Client anlegt, offenbar
  gar nichts. Aperio liest beide als fremd organisiert; mit Markern als
  still (`scheduling_silenced`). Aperio schreibt die Marker trotzdem: Sie
  sind der Weg des Standards (RFC 6638 §7.1), Apple setzt sie selbst beim
  Import, und die Dialoge versprechen dann keine Mail.
- Google hat `events.import`, das Organisator und Teilnehmer behält und nichts
  verschickt — laut Dokumentation, ungemessen (kein Google-Konto).
- Exchange (I0, Live-Lauf 231133, 2026-10-25, Exchange der Hochschule;
  berichtigt nach Lauf 075306, 2026-10-26):
  - `CreateItem` mit `MimeContent` und `METHOD:REQUEST` im Kalenderordner
    legt **keinen Termin** an, sondern eine Einladungs-*Nachricht*
    (`t:MeetingRequest`), roh als iCal-Text wie als MIME-Nachricht, auch für
    einen eigenen Termin (`ORGANIZER` = Konto). Sie trägt Organisator,
    Teilnehmer und UID (darum las I0 sie als Termin; `MyResponseType` fehlte,
    weil eine Nachricht keinen hat), nichts wird verschickt. Aperio sieht sie
    nicht: Der Abgleich liest nur `t:CalendarItem`, Outlooks Kalenderansicht
    ebenso. Lauf 075306 (ICS-2) brach darum bei jeder Prüfung ab.
  - Ohne `METHOD` wird derselbe Text ein Termin, aber ein eigenes Meeting des
    Kontos (`MyResponseType` Organizer): Der fremde Organisator geht verloren.
  - Das gewöhnliche `CreateItem` lehnt ein `Organizer`-Feld ab
    (`ErrorInvalidPropertySet`), behält aber eine mitgegebene `UID`.
  - Runde ICS-3 (Lauf 080508, 2026-10-26): Liegt die Einladungs-Nachricht im
    Posteingang, macht Exchanges Kalender-Assistent in 90 s keinen Termin
    daraus (er arbeitet nur an zugestellten Nachrichten). Eine vorläufige
    Zusage auf die Nachricht (`TentativelyAcceptItem`, `SaveOnly`) macht
    sofort einen Termin, aus dem Posteingang wie aus dem Kalenderordner:
    fremder Organisator, alle Teilnehmer, die UID der Datei, `MyResponseType`
    Tentative, nichts verschickt, kein Entwurf; die Nachricht zieht Exchange
    selbst weg (in einen anderen Ordner, vermutlich „Gelöschte Elemente“).
    Aperio liest den Termin als fremd organisiert (`organized_elsewhere`).
  - Runde ICS-4 (Lauf 081527, 2026-10-26), der Weg über den Kalenderordner:
    - Nach der vorläufigen Zusage liegt im Kalenderordner nur der Termin
      (`IPM.Appointment`), die Nachricht in „Gelöschte Elemente“
      (`IPM.Schedule.Meeting.Request`, mit dem Titel des Termins). Der
      Posteingang bleibt leer, nichts verschickt, kein Entwurf.
    - Die eigene Antwort lässt sich nicht auf „nicht beantwortet“
      zurücksetzen: `PidLidResponseStatus` (0x8218) = 5 lehnt Exchange ab
      (`ErrorInvalidExtendedPropertyValue`).
    - Aperios eigene Wege an diesem Termin: Zusagen mit Antwort schickt
      „Akzeptiert: …“ an den Organisator (die Unzustellbar-Meldung kam);
      Zusagen ohne Antwort setzt Accept und schickt nichts, ohne Entwurf
      (der Verdacht, stilles Antworten lasse Entwürfe liegen, ist
      widerlegt); Absagen ohne Antwort nimmt den Termin aus dem Kalender und
      schickt nichts; Löschen schickt nichts, auch mit
      `send_cancellations` — wie die Oberfläche es für die Kopie eines
      Teilnehmers schon sagt (ein schlichtes Löschen).
  - Runde ICS-5 (Lauf 130307, 2026-10-26), Aperios eigener Weg aus I6e
    (`create_event` mit `keeps_invitation`): ein Einzeltermin, einer, dessen
    Datei das Konto nicht nennt, eine Serie mit Lücke in Europe/Berlin, ein
    ganztägiger und das eigene Meeting des Kontos.
    - Jeder fremde wurde ein `IPM.Appointment` mit dem fremden Organisator,
      `MyResponseType` Tentative, der UID der Datei und der Erinnerung;
      Aperio liest ihn als fremd organisiert. Das Konto steht unter den
      Teilnehmern, auch wo die Datei es nicht nannte. Posteingang, Entwürfe
      und „Gelöschte Elemente“ blieben leer, nichts wurde verschickt.
    - Die Serie zeigt Outlooks Kalenderansicht am 2., 16. und 23.11. um
      10:00 Berliner Zeit, der 9.11. fehlt. Die Zone speichert Exchange unter
      dem TZID der Datei: den Beginn als `Name="Europe/Berlin"` neben leerer
      `Id`, das Ende umgekehrt (`Id="Europe/Berlin"`, leerer Name). Aperio las
      die leere Kennung als keine Zone; die Serie hätte sich bei Aperio in
      UTC wiederholt und wäre über die Zeitumstellung eine Stunde gewandert.
      Behoben in I6e (siehe dort).
    - Den ganztägigen Termin speichert Exchange in UTC
      (`tzone://Microsoft/Utc`, 26.11. 00:00Z bis 27.11. 00:00Z,
      `IsAllDayEvent`); Aperio liest den 26.11. Outlook zeigt einen
      ganztägigen Termin in seiner gespeicherten Zone (Tokio, dritte Runde
      der Serienzonen), also am 26.11.; in diesem Lauf nicht nachgesehen.
    - Das eigene Meeting wurde das des Kontos (`MyResponseType` Organizer),
      nichts verschickt.

## Bauweise

**Manifest.** Ein Adapter nennt seine Formate:

```json
"file_imports": [
  {
    "id": "ics",
    "extensions": ["ics", "ical", "ifb"],
    "mime_types": ["text/calendar"],
    "yields": ["events"],
    "name": "iCalendar file",
    "name_key": "import.ics.name"
  }
]
```

Der Name steht wörtlich in `name` (Pflicht, beim Laden geprüft); `name_key`
übersetzt ihn wie die übrigen Texte des Manifests aus `strings` (de, en).

**Einstieg.** Ein optionaler C-ABI-Einstieg `aperio_plugin_import_file`, wie
`discover` ohne Konto: Er bekommt das Format und den Inhalt und gibt die Teile
als JSON zurück. Am Handy, wo alles statisch gelinkt ist, trägt ihn
`StaticHooks`. Ein Plugin, das ihn nicht exportiert, meldet keine Formate an.

**Ergebnis.** `cal_core::FileImport` (TS-Typen generiert): die gelesenen
Termine — Titel, Beschreibung, Ort, Beginn und Ende, ganztägig, Wiederholung
mit Ausnahmen und Zone, Erinnerungen, Teilnehmer, Organisator, UID — und was
übersprungen wurde, benannt (Aufgaben, Notizen, Frei/Belegt, ein Teil, der
sich nicht lesen ließ), nicht nur gezählt.

**Host.** Die Registry sammelt die Formate der geladenen Plugins. Desktop und
Handy fragen sie nach dem Dateifilter und reichen eine Datei an das Plugin,
dessen Format sie trägt. Kein Adapter-Name im Host.

**Doppelte.** `Event::ical_uid` trägt die UID, wo der Anbieter sie kennt:
CalDAV `UID`, Exchange `calendar:UID` (nur über GetItem, das der Live-Lauf
bestätigt; der Abgleich über SyncFolderItems fragt nicht danach), Google
`iCalUID`, Graph `uid` für alle Zeilen einer Serie und `iCalUId` nur für einen
Einzeltermin (`iCalUId` ist je Vorkommen eine andere), lokal eine Spalte
(Migration 47). Ein geändertes Vorkommen trägt die UID seiner Serie. Gelesen
wird sie auf jedem Weg, auch beim Abgleich zwischen Geräten: Ein lokales
Update, das sie nicht mitbringt, lässt die gespeicherte stehen, und eine UID,
die eine Seite hat und die andere nicht, ist nie ein Konflikt. Beim Anlegen
heißt das Feld `NewEvent::uid_to_keep`, mit Absicht anders: Ein Anlegen, das
aus einem gelesenen Termin zusammengesetzt wird (Kopie, Rest einer geteilten
Serie, herausgelöstes Vorkommen), nimmt so nie dessen UID mit. Nur der Import
setzt es. Ein Verschieben in einen anderen Kalender legt im Ziel an, bevor es
die Quelle löscht; mit derselben UID stünde sie einen Moment zweimal beim
Anbieter, und ob iCloud oder Exchange das zulassen, ist ungemessen: Es bekommt
eine neue. Geschrieben wird die UID, wo der Anbieter sie annimmt: CalDAV,
Exchange (`t:UID`, gemessen), lokal. Google nimmt sie nur über
`events.import`, das Aperio für fremde Einladungen nutzt (I6f), Graph gar
nicht. Die Liste und der Editor fragen den
Zielkalender über den Zeitraum der Termine.

## Schritte

- **I0 Live-Test Exchange.** Eine fremde Einladung über `CreateItem` mit
  `MimeContent` anlegen, `SendMeetingInvitations=SendToNone`: Bleibt der
  Organisator, bleiben die Teilnehmer, geht eine Mail hinaus? Teilnehmer-
  Adressen unter `.invalid`, damit jede verschickte Mail als Unzustellbar-
  Meldung zurückkommt. Toni startet das Testprogramm.
- **I1 Anmeldung** (gebaut). `PluginManifest::file_imports`
  (`FileImportFormat`, beim Laden geprüft), der Einstieg
  `aperio_plugin_import_file` (`ImportFileFn`, `StaticHooks::import_file_fn`,
  `PluginManager::import_file`), im SDK `declare_import_file!` und
  `import_file: yes`, statisch `register!(crate, import_file)`;
  `host_core::file_import` (`import_formats`, `format_for`, `import_file`,
  höchstens 25 MB) und die Befehle beider Hosts (`file_import_formats`,
  `read_import_file`; am Handy `fileImportFormatsJson`,
  `readImportFileJson`). Die Typen `FileImport`, `ImportedEvent`,
  `SkippedPart`, `ImportFormat` kommen generiert nach TS.
- **I2 ICS im iCal-Adapter** (gebaut), in drei Teilen:
  - **I2a** Die CLDR-Tabelle Windows → tzdata zieht aus dem Exchange-Adapter
    in den Kern; Exchange liest sie von dort.
  - **I2b** Der iCal-Leser (auch CalDAV) löst eine Zone wie Outlooks „W.
    Europe Standard Time“ über die Tabelle auf und, wo sie fehlt, über die
    VTIMEZONE der Datei. Bisher las er solche Zeiten als UTC.
  - **I2c** Der Import selbst (`adapter_ical::import::import_ics`, über
    `mapping::read_calendar_file`): Einzeltermine und Serien, geänderte
    Termine als eigene Termine ohne UID mit Lücke in der Serie, ein
    abgesagtes Vorkommen nur als Lücke; ein abgesagter Termin
    (`STATUS:CANCELLED`, `METHOD:CANCEL`) und eine Antwort (`METHOD:REPLY`,
    `COUNTER`) fügen nichts hinzu und werden benannt, ebenso Aufgaben,
    Notizen, Frei/Belegt und Unlesbares. Proben in
    `crates/adapter-ical/tests/fixtures/` (Outlook-Einladung, Teams-Serie,
    Google-Einladung, Apple-Export, Absage, Antwort).
- **I3 Desktop** (gebaut). „Importieren …“ in der Werkzeugleiste und Strg+O
  öffnen den Dateidialog mit dem Filter der Registry (`useFileImport`); eine
  aufs Fenster gezogene Datei kommt als HTML-Drop ohne Pfad und wird aus
  ihren Bytes gelesen (`useFileDrop`, `read_import_content`). Ein Termin
  öffnet den Editor (`importFrom`): Tag, Zeit, Ort, Beschreibung,
  Erinnerungen, die Serie mit den Lücken und der Zone der Datei
  (`preserveRecurrenceZone`), der Hinweis als erster Halt, der Titel „Termin
  aus Datei“. Mehrere öffnen `ImportDialog`: angehakt alle, „Bearbeiten …“
  öffnet einen im Editor, „N Termine importieren“ schreibt die angehakten
  nacheinander in den gewählten Kalender, ohne zu benachrichtigen; was nicht
  ankam, wird mit Titel und Grund genannt, was ankam, als „importiert“
  markiert. Kein Termin, oder ein Fehler, ist ein Satz mit Schließen
  (`importFailureSentence`, ein zu großer Datei am Marker
  `import-too-large`). Die vorläufige Regel `importedForEditor` galt bis I6;
  seit I6b gelten `importInto`, `importPeople` und `importEditorPeople` in
  Editor und Liste.
- **I4 UID**, in drei Teilen:
  - **I4a** Lesen (gebaut): `Event::ical_uid` bei CalDAV, Exchange, Google,
    Graph und lokal, durch Abgleich, Schnappschuss und Zwischenspeicher
    (Generation 8, Exchange-Parser 6).
  - **I4b** Schreiben (gebaut): `NewEvent::uid_to_keep` bei CalDAV (die UID
    benennt die Ressource; eine ungewöhnliche bekommt eine eigene),
    Exchange (`t:UID`) und lokal; Editor und Liste geben die UID der Datei
    mit. Steht sie schon im Kalender, lehnt CalDAV ab, und Aperio sagt es:
    `WriteRefusal::UidTaken` („Dieser Termin steht schon in diesem
    Kalender“).
  - **I4c** Doppelte in Liste und Editor (gebaut, Desktop): Beide fragen den
    gewählten Kalender über den Zeitraum der Termine (`importSpan`, eine
    Minute weiter) und vergleichen die UIDs (`alreadyHeld`, beides in
    `shared/fileImport.ts`), wieder bei jedem Kalenderwechsel; eine Antwort
    trägt ihren Kalender (`useHeldOnImport`). Die Liste lässt Vorhandene
    abgewählt, nennt sie mit Titel in einem Satz und an der Zeile („steht schon in
    „Arbeit““), angehakt „wird als Kopie importiert“, ohne UID; ein anderer
    Kalender hakt wieder an, was er nicht hat. Der Editor sagt „Dieser Termin
    steht schon in …“ und legt dann eine Kopie an. Solange gefragt wird,
    schreibt keiner von beiden und sagt warum; wer nicht fragen konnte, sagt
    es und importiert mit UID (CalDAV lehnt ein Doppel dann ab).
- **I5 Handy.** „Termine importieren …“ in der Kalenderliste mit
  Dokumentwähler; „Teilen“ und „Öffnen in“: iOS als
  Dokumenttyp mit Rang `Alternate` (Aperio wird angeboten, nicht Standard),
  Android mit Intent-Filtern `VIEW` und `SEND` für `text/calendar`. In vier
  Teilen, wie am Desktop:
  - **I5a** Brücke (gebaut): `fileImportFormatsJson` und
    `readImportFileJson` in beiden nativen Modulen (Android in einer eigenen
    Gruppe `fileImportFunctions`), `mobile/src/api/fileImport.ts`. Ein
    Lesefehler kommt mit den Codes des Desktops (`unsupported`,
    `invalid_input`, `internal`; `importCoded`), damit
    `importFailureSentence` ihn in der Sprache der Person sagt. Die
    Kotlin-Bindings waren älter als I1 und sind neu erzeugt.
  - **I5b** (gebaut) Einstieg, Dokumentwähler und Liste: „Termine
    importieren …“ unter „Neuer Kalender“ in der Kalenderliste
    (`useFileImport`); der Dokumentwähler filtert nach den MIME-Typen der
    Formate (ungemessen, ob jeder Dateimanager `.ics` als `text/calendar`
    meldet). Was gelesen wurde, entscheidet `readAndPresent`
    (`mobile/src/state/fileImportFlow.ts`): ein Termin den Editor, im
    zuletzt benutzten Kalender, sonst im ersten angebotenen; mehrere den
    Bildschirm `FileImport`; ein Fehler, eine leere Datei oder kein
    beschreibbarer Kalender einen Satz. Der Bildschirm ist die Liste des
    Desktops: Kalenderwahl, Doppelte (I4) und Einladungen (I6), eine Zeile
    als Ankreuzfeld mit „Bearbeiten …“ als Aktion (für Sehende ein Knopf
    daneben, wie „Verwalten“ in der Kalenderliste). Was der Editor aus der
    Liste anlegt, meldet er ihr über `importProgress`, weil eine Route keine
    Rückruffunktion trägt. Die Regeln beider Listen stehen jetzt einmal in
    `shared/fileImport.ts` (`importedCreate`, `rechosenForHeld`,
    `heldStatusSentence`, `invitationListSentence`, `importWhen`,
    `importRowLabel`).
  - **I5c** (gebaut) der Editor: `importFrom`, `importNote` und die Liste
    (`importToken`, `importIndex`) als Route-Parameter; Titel „Termin aus
    Datei“, die Hinweise vor dem ersten Feld, eine fremde Einladung
    gesperrt wie eine empfangene (`fieldsLocked`; Kalender, Erinnerungen
    und Farbe bleiben wählbar), die Personen beim Anlegen aus
    `importEditorPeople` (gemeinsam mit dem Desktop), die UID der Datei,
    die Zone der Datei. Anders als am Desktop lädt das Handy die Kalender
    erst nach dem Öffnen: Bis dahin gilt eine fremde Einladung als „wird
    geprüft“, sonst läse ein Kalender, der Einladungen behält, kurz als
    einer, der es nicht tut. Kein Test rendert die Bildschirme des Handys;
    geprüft sind die Regeln (vitest) und `tsc`.
  - **I5d** (gebaut, am Gerät ungemessen) „Teilen“ und „Öffnen in“. iOS:
    ein Dokumenttyp für `com.apple.ical.ics` mit Rang `Alternate` (Aperio
    wird angeboten, nicht Standard) und `LSSupportsOpeningDocumentsInPlace`
    aus, also eine Kopie in der Inbox der App; die Datei kommt über
    `Linking` als `file://`-Adresse, beim Start (`getInitialURL`) wie im
    laufenden Betrieb (`url`), und `incomingFileOfUrl` nimmt nur Dateien,
    nicht den OAuth-Rückweg. Android: Intent-Filter `VIEW` und `SEND` für
    `text/calendar` an der Hauptaktivität; `takeIncomingFileJson` (nur
    Android, `ONLY_ON`) liest den Intent — den Start-Intent einmal, einen
    späteren aus `OnNewIntent` —, nimmt den Strom aus den Daten oder dem
    `ClipData` eines Teilens und kopiert ihn in den Cache, höchstens ein
    Byte über die Grenze des Imports, damit das Lesen eine zu große Datei
    ablehnt, ohne sie ganz zu kopieren. Der Start-Intent bleibt der der
    Aktivität, wenn Android sie nach dem Ende des Prozesses neu anlegt (aus
    den letzten Apps, oder wenn die Aufgabe zurückgeholt wird): Ein
    genommener bleibt deshalb über Prozesse hinweg gemerkt, an seiner
    Aufgabe und seiner Adresse, und einer aus den letzten Apps
    (`FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY`) zählt nie. Ohne Dateinamen heißt
    die Kopie „import“, mit der Endung ihres MIME-Typs, weil der Import das
    Plugin an der Endung findet; der letzte Teil einer `content://`-Adresse
    ist eine Kennung, kein Name. Eine Datei, die sich nicht öffnen oder
    kopieren lässt, ist ein Fehler mit ihrem Namen (`internal`), also ein
    Satz. Gefragt wird beim Start und bei jeder Rückkehr in den Vordergrund.
    Beides öffnet über `readAndPresent` Editor oder Liste im Kalender-Tab
    (`IncomingFileImport`, neben den anderen App-weiten Hosts), jeder Import
    eine eigene Route (`getId` am Token), damit eine Datei nie einen
    offenen Editor oder eine offene Liste übernimmt. Ein Satz ist eine
    native Meldung (`Alert`), die sich auch über einem offenen Blatt zeigt,
    wo ein Modal an der App-Wurzel es unter iOS nicht kann. iOS nimmt die
    Start-Adresse einmal je Lauf; ein Wechsel der Sprache öffnet die Datei
    nicht noch einmal.
    Ungemessen: ob Expos AppDelegate ein geöffnetes Dokument an `Linking`
    weitergibt, welche Apps Aperio anbieten, und die Kopien in der Inbox
    bleiben liegen.
- **I6 Fremde Einladungen** (Toni, nach I0): Organisator und Teilnehmer
  bleiben, niemand bekommt etwas. Exchange (gemessen, ICS-3/ICS-4): die
  Einladungs-Nachricht über `MimeContent` mit `METHOD:REQUEST` in den
  Kalenderordner, dann `TentativelyAcceptItem` mit `SaveOnly`; die Nachricht,
  die Exchange dabei nach „Gelöschte Elemente“ legt, entfernt Aperio mit
  `SoftDelete`, wiederherstellbar (Toni, 2026-10-26). Die eigene Antwort
  steht danach auf „Mit Vorbehalt“, und der Hinweis im Editor sagt das: der
  Organisator erfährt nichts (Toni, 2026-10-26). iCloud mit
  `SCHEDULE-AGENT=CLIENT` am
  Organisator und `NONE` an der eigenen Zeile (gemessen, Lauf 100518),
  Google über `events.import` im Hauptkalender (ungemessen, I6f), lokal
  behält alles. Ein Anbieter ohne stillen Weg legt
  den Termin ohne Teilnehmer an, und der Editor sagt es. Vorher Exchange live:
  Der zweite Lauf (ICS-2, 075306) fand keinen Termin, nur die Nachricht; die
  Runde ICS-3 (Lauf 080508) maß die Wege zum Termin, ICS-4 (Lauf 081527)
  danach zusagen, absagen und löschen über Aperios eigenen Weg, und was
  hinausgeht.
  Entscheidungen (Toni, 2026-10-26): Lesen und Schreiben von iCal ziehen in
  ein gemeinsames Crate, das CalDAV, der iCal-Adapter und Exchange nutzen
  (erst der Umzug ohne neues Verhalten), damit kein Adapter am anderen
  hängt; iCloud wird vor dem Bau live gemessen (bleibt das Anlegen mit
  `SCHEDULE-AGENT` still?); Google wird nach Doku über `events.import`
  gebaut, ungemessen. Steht das Konto nicht unter den Teilnehmern (eine
  Einladung an eine andere Adresse), trägt Aperio es ein, nur in der eigenen
  Kopie (Toni, 2026-10-26). Im Editor ist eine fremde Einladung wie eine
  empfangene: Titel, Zeit, Ort, Beschreibung und Teilnehmer nur lesbar,
  Kalender, Erinnerungen und Farbe wählbar (Toni, 2026-10-26). Unter dem
  Crate bleibt `icalendar`; `calcard` wird
  später geprüft (TODO.md). Warum Exchange überhaupt iCal braucht: Eine
  Einladung, die jemand anderes organisiert, nimmt Exchange nur so an, wie sie
  per Mail kommt (`METHOD:REQUEST`); das gewöhnliche Anlegen lehnt einen
  fremden Organisator ab (I0) und macht mit Teilnehmern ein eigenes Meeting
  (ICS-3). Alles andere legt der Import als gewöhnlichen Exchange-Termin an.
  - **I6a** (gebaut): `crates/ical-mapping` hält die Abbildung (Lesen,
    Schreiben, VTIMEZONE, Zonendefinitionen, rohe Zeilen, die eigene
    Identität), umgezogen ohne neues Verhalten; CalDAV behält die Pfade
    (`adapter_caldav::mapping`), der iCal-Adapter hängt nicht mehr an CalDAV.
  - **I6b** (gebaut, Desktop) Kern und Oberfläche: `Calendar::invitation_import`
    (`not-kept`, `kept`, `kept-tentative`) sagt der Adapter;
    `NewEvent::keeps_invitation` markiert die fremde Einladung, und beide
    Hosts lassen vor dem Anlegen `attendee::guard_invitation_import` laufen:
    behält der Kalender nicht, gehen Organisator und Teilnehmer weg, und der
    Termin wird der eigene. Die Oberfläche unterscheidet je Zielkalender:
    ohne Organisator ein neuer Termin; mit dem Konto als Organisator
    (`calendarCurrentUserEmail`) das eigene Meeting, wie ein neuer Termin;
    sonst die fremde Einladung, gesperrt wie eine empfangene, mit dem Satz
    des Kalenders. Erkennt die Oberfläche das Konto nicht (eine andere
    Schreibweise), prüft der Adapter beim Anlegen mit allen Adressen des
    Kontos. Bis I6c-I6f sagen alle Adapter `not-kept`: Verhalten wie die
    vorläufige Regel, außer dass ein eigenes Meeting jetzt seine Teilnehmer
    behält.
  - **I6c** (gebaut) lokal: `kept`; der Organisator einer behaltenen
    Einladung steht in einer eigenen Spalte (Migration 48), wird gelesen
    (`organized_elsewhere`) und beim Abgleich zwischen Geräten mitgetragen; ein
    Update ohne ihn lässt ihn stehen. Alles andere, was lokal angelegt wird,
    nennt keinen Organisator. Antworten kann ein lokaler Kalender niemandem:
    Zusagen und Absagen erscheinen dort nicht (keine eigene Adresse, keine
    Antworten der Teilnehmer).
  - **I6d** (gebaut) CalDAV: `kept` für jeden Kalender; der Organisator der
    Datei mit `SCHEDULE-AGENT=CLIENT`, die eigene Zeile mit `NONE`, ergänzt,
    wo die Datei das Konto nicht nennt; gelesen als fremd und still. Ein
    Organisator, den die Identität des Kontos nennt, macht das eigene Meeting
    (ohne Benachrichtigen also ohne Teilnehmer, wie jedes eigene Meeting auf
    einem einladenden Server).
  - **I6e** (gebaut) Exchange: `kept-tentative` für jeden Kalender. Eine
    fremde Einladung geht als Anfrage ihres Organisators hinein
    (`ical_mapping::mapping::invitation_request`: `METHOD:REQUEST`, der
    Organisator, die Teilnehmer der Datei ohne ihn und das Konto, wo die
    Datei es nicht nennt) mit `SendToNone`; dann `TentativelyAcceptItem` mit
    `SaveOnly`, die verschobene Anfrage mit `SoftDelete` aus „Gelöschte
    Elemente“ (scheitert das, bleibt der Termin, und Aperio vermerkt es im
    Protokoll), und der Termin wird zurückgelesen
    (`adapter-ews/src/kept_invitation.rs`). Das Konto ist der Anmeldename,
    wo er eine Mail-Adresse ist (`api::own_address`); ein Organisator mit
    dieser Adresse macht das eigene Meeting auf dem gewöhnlichen Weg. Bei
    einer Anmeldung als `DOMÄNE\Name` kennt Aperio keine Adresse, und jede
    markierte Einladung gilt als fremd. Live bestanden in Runde ICS-5. Dazu
    das Lesen der Serienzone: Eine Zone mit leerer `Id` nennt ihr `Name`
    (`zone_definition::element_zone_id`), und eine Kennung, die tzdatas
    eigener Name ist, gilt jetzt auch für die Zone einer Serie, nicht nur für
    ganztägige Grenzen (236). Exchange-Parser 7 und Leseregel 5 lesen den
    Zwischenspeicher neu.
  - **I6f** (gebaut, nach Doku, ungemessen) Google: `events.import`, das
    einzige Anlegen, das einen Organisator annimmt; es verlangt die
    `iCalUID` und kennt kein `sendUpdates` (eine so importierte Kopie eines
    Teilnehmers bekommt keine Einladungs-Mail, sagt Google). Google nimmt sie
    nur an, wo der Besitzer des Kalenders Organisator oder Teilnehmer ist;
    laut Berichten klappt das nur im Hauptkalender, dessen Kennung die
    Adresse des Kontos ist. Darum sagt nur der Hauptkalender `kept`
    (`primary` aus der Kalenderliste), jeder andere `not-kept`, und dort wird
    die Einladung der eigene Termin ohne Teilnehmer (Toni, 2026-10-26). Das
    Konto kommt aus `GET /calendars/primary`, nur bei einem markierten
    Anlegen. Google sagt nicht, was ein Import mit einer UID tut, die schon
    im Kalender steht: Aperio fragt vorher (`events.list?iCalUID=`) und lehnt
    eine vorhandene als `uid-taken` ab (Toni, 2026-10-26). Eine Kopie ohne
    die UID der Datei bekommt `{uuid}@aperio`. Ein Bericht (GAM) sagt, Google
    übernehme den Organisator beim Import nicht; liest Aperio den Termin mit
    dem Konto als Organisator zurück, bleibt er so, wie Google ihn gemacht
    hat, als eigenes Meeting mit Teilnehmern, und Aperio vermerkt es im
    Protokoll (Toni, 2026-10-26: behalten). Verschickt wurde dabei nichts.
    Mit I6f ziehen die beiden Regeln, die Exchange und Google teilen, in den
    Kern: wessen Meeting ein markiertes Anlegen ist
    (`attendee::invitation_organizer`) und wen es nennt
    (`attendee::invitation_attendees`). CalDAV prüft gegen alle Adressen
    seiner Identität und behält seinen eigenen Weg. Quellen:
    Google-Referenz `events.import`, „Invite users to an event“ (dieselbe
    `iCalUID`, Organisator in der Kopie des Teilnehmers), eine Frage auf
    Stack Overflow (Besitzer muss Organisator oder Teilnehmer sein; mit
    Teilnehmern nur im Hauptkalender) und das GAM-Wiki (Organisator nicht
    übernommen).
- **I7 Aufgaben** aus VTODO, später.

## Prüfrunde 1 (PR #129, 2026-10-26)

Fünf Prüfer (Kern, Adapter, Desktop, Handy, Doku), je mit einem Gegenprüfer:
45 bestätigte Funde, einige doppelt. Behoben:

- **Liste am Desktop.** „Bearbeiten …“ legt den Editor über die Liste, und
  der Dialogstapel hängt sie dabei aus: Was sie getan hatte, ging verloren,
  und ein im Editor angelegter Termin kam beim Import erneut. Der Fortschritt
  hängt jetzt am Leseergebnis der Datei (`listProgress`), das der Stapel
  hält. Dazu: eine gleiche Meldung wird erneut fokussiert und gesagt; der
  Satz zu Einladungen wird bei einem Kalenderwechsel gesagt; ohne
  beschreibbaren Kalender schreibt die Liste nichts; ein angekommener Termin
  ist ein eigener Halt; beim Schreiben sind Kalender und Zeilen gesperrt;
  der Satz zum Teilerfolg nennt jeden Termin mit eigenem Grund
  (`partlyImportedSentence`, Plural).
- **Editor.** Wird die Sperre nach der Prüfung des Organisators aufgehoben,
  fiel der Fokus auf die Seite; er geht jetzt zum Titel. Eine fremde
  Einladung bekommt nie die Signatur des Kontos, auch nicht nach einem
  Kalenderwechsel; das Handy hängt sie an einen eigenen importierten Termin
  wie der Desktop.
- **Ablegen am Desktop** liest höchstens ein Byte über die Grenze
  (`IMPORT_MAX_BYTES`, gegen `MAX_IMPORT_BYTES` getestet).
- **Handy.** Jeder Import ist eine eigene Route (`importRouteId`, ein Token
  für die Liste); iOS nimmt die Start-Adresse einmal; Sätze als native
  Meldung; Android merkt sich einen genommenen Start-Intent über Prozesse,
  benennt eine Kopie ohne Namen mit der Endung ihres MIME-Typs und sagt
  einen Kopierfehler; der Schließen-Knopf der Liste ist beim Schreiben
  nicht verfügbar; „Bearbeiten …“ ohne Kalender sagt es.
- **CalDAV.** Ein 412 nach einer Wiederholung gilt nur als gelandet, wo
  Aperio die Ressource selbst benannt hat (eine selbst vergebene UID, oder die
  frische `{uuid}.ics` einer behaltenen UID, die kein schlichter Name ist;
  Prüfrunde 17); wo eine behaltene UID sie benennt, ist er `uid-taken`.
- **iCal.** Die Absage eines einzelnen Termins ohne seine Serie in der Datei
  wird genannt.
- **Doppelte ehrlich** (Toni, 2026-10-26): `Calendar::keeps_uid` sagt, ob
  ein Anlegen die UID der Datei behält (CalDAV, Exchange, lokal). Wo nicht
  (Google beim gewöhnlichen Anlegen, Microsoft 365, Gerätekalender), fragt
  Aperio nicht, was der Kalender schon hält, und sagt in Liste und Editor,
  dass es das dort nicht erkennen kann. Cache-Generation 9, weil gespeicherte
  Kalenderlisten weder diese Angabe noch `invitation_import` tragen.
- **Exchange, zweiter Kalenderordner** (Toni, 2026-10-26: erst live
  messen). Runde ICS-6 (Lauf 170846): Aperio schrieb die Anfrage in einen
  eigenen Kalenderordner der Runde (`SavedItemFolderId` = dieser Ordner),
  aber Exchange legte den Termin der vorläufigen Zusage in den
  Standardkalender; der neue Ordner blieb leer. Aperio meldete trotzdem
  den neuen Ordner. Nichts verschickt, nichts in Entwürfen, nichts in
  Gelöschten Elementen. Toni: **danach verschieben** — der Termin geht nach
  der Zusage in den gewählten Ordner (`MoveItem`); eine weitere Runde misst,
  ob Organisator, Teilnehmer, die vorläufige Antwort und die UID das
  Verschieben überstehen und ob dabei etwas hinausgeht.
- Doku: was CalDAV beim eigenen Meeting über die Liste tut, Zonen nur aus
  der Datei, wer `uid-taken` sagt, wer die Kern-Regeln nutzt, der Fehlertext
  eines Plugins.

## Nach Prüfrunde 1 (Toni, 2026-10-26)

- **Signatur.** Ein importierter Termin bekommt nie von selbst die Signatur
  des Kalenders, auch der eigene nicht: Er behält die Beschreibung aus der
  Datei, eine Regel für jeden Import. Von Hand fügt der Knopf „Signatur“
  sie bei einem eigenen Termin weiter ein.
- **Exchange verschiebt in den gewählten Ordner.** Nach der Zusage fragt der
  Adapter, wo der Termin liegt (`item:ParentFolderId`); liegt er nicht im
  gewählten Ordner, verschiebt `MoveItem` ihn dorthin (der Ordner ohne
  ChangeKey, der bei jeder Änderung des Inhalts veraltet). Scheitert das
  Verschieben, löscht der Adapter den Termin wieder (Gelöschte Elemente,
  `SendToNone`; als Teilnehmer schickt Löschen nie etwas, ICS-4), und das
  Anlegen scheitert: Der Import landet, wo er hin sollte, oder gar nicht.
  Runde ICS-7 (Lauf 172507) bestanden: In einen eigenen Ordner importiert,
  fragte der Adapter, verschob einmal (`MoveItem` Success), und der Termin
  stand dort mit Organisator, beiden Teilnehmern, „vorläufig“, der UID und
  der Erinnerung; im Standardkalender nichts. In den Standardkalender
  importiert, fragte er und verschob nichts (die Ordner-Ids gleichen sich).
  Nichts verschickt, nichts in Entwürfen oder Gelöschten Elementen.
  Ungemessen bleibt, wohin Exchange eine spätere
  Aktualisierung des Organisators legt (die Testadressen enden auf
  `.invalid`).
- **Gesperrte Beschreibung lesbar** (Toni, 2026-10-26, NVDA am Desktop): Ein
  `<textarea readOnly>` ist für Chromium kein bearbeitbarer Text; NVDA
  folgte darin keinem Cursor und sagte nur die erste Zeile. Die Beschreibung
  einer gesperrten Einladung ist jetzt ein fokussierbares Dokument
  (`ReadOnlyText`, `role="document"`, eine Zeile je Block): NVDA wechselt
  dort in den Lesemodus, `Tab` führt hinaus. Ebenso die mehrzeiligen Felder
  eines schreibgeschützten Adressbuchs (Notizen, Mitglieder, Straße).
- **Fokus nach dem Import** (Toni, 2026-10-26: Strg+O aus der
  Kalenderansicht, danach hing NVDA zwischen Fokus- und Lesemodus). Im Code
  keine Ursache: Der Editor schließt wie beim gewöhnlichen Anlegen, und wry
  gibt WebView2 den Fokus beim Aktivieren des Fensters zurück (`MoveFocus`
  bei `WM_SETFOCUS`). Sein Protokoll enthielt keine Zeile aus der Webansicht.
  Eine vorübergehende Fokus-Spur (`traceFocus`, drei Minuten ab dem Import,
  ohne Inhalte) misst den nächsten Durchlauf; danach beheben und entfernen.
