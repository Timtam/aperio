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
    "name_key": "import.ics.name"
  }
]
```

Der Name kommt wie die übrigen Texte des Manifests aus `strings` (de, en).

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
`events.import` (I6), Graph gar nicht. Die Liste und der Editor fragen den
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
  `import-too-large`). Die vorläufige Regel `importedForEditor` gilt in
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
    trägt ihren Kalender (`useHeldOnImport`). Die Liste hakt Vorhandene ab,
    nennt sie mit Titel in einem Satz und an der Zeile („steht schon in
    „Arbeit““), angehakt „wird als Kopie importiert“, ohne UID; ein anderer
    Kalender hakt wieder an, was er nicht hat. Der Editor sagt „Dieser Termin
    steht schon in …“ und legt dann eine Kopie an. Solange gefragt wird,
    schreibt keiner von beiden und sagt warum; wer nicht fragen konnte, sagt
    es und importiert mit UID (CalDAV lehnt ein Doppel dann ab).
- **I5 Handy.** „Termine importieren …“ in der Kalenderliste mit
  Dokumentwähler; „Teilen“ und „Öffnen in“: iOS als
  Dokumenttyp mit Rang `Alternate` (Aperio wird angeboten, nicht Standard),
  Android mit Intent-Filtern `VIEW` und `SEND` für `text/calendar`.
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
  Google über `events.import` (ungemessen), lokal behält alles. Ein Anbieter ohne stillen Weg legt
  den Termin ohne Teilnehmer an, und der Editor sagt es. Vorher Exchange live:
  Der zweite Lauf (ICS-2, 075306) fand keinen Termin, nur die Nachricht; die
  Runde ICS-3 misst drei Wege zum Termin und danach zusagen, absagen und
  löschen über Aperios eigenen Weg, und was hinausgeht.
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
- **I7 Aufgaben** aus VTODO, später.
