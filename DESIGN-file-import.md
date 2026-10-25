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
   liest sie selbst; `.ics` meldet der iCal-Adapter an, der den Parser des
   CalDAV-Adapters schon teilt.

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
- Google hat `events.import`, das Organisator und Teilnehmer behält und nichts
  verschickt — laut Dokumentation, ungemessen (kein Google-Konto).
- Exchange (I0, Live-Lauf 231133, 2026-10-25, Exchange der Hochschule):
  - `CreateItem` mit `MimeContent` und `METHOD:REQUEST` legt eine fremde
    Einladung an, roh als iCal-Text wie als MIME-Nachricht: Organisator,
    Teilnehmer und UID bleiben, Exchange führt den Organisator auch unter den
    Teilnehmern. `MeetingRequestWasSent` false, nichts in „Gesendete
    Elemente“, keine Unzustellbar-Meldung an die `.invalid`-Adressen.
    `MyResponseType` fehlt beim Lesen.
  - Ohne `METHOD` wird derselbe Text ein eigenes Meeting des Kontos
    (`MyResponseType` Organizer): Der fremde Organisator geht verloren.
  - Ein eigener Termin (`ORGANIZER` = Konto) bleibt ein eigenes Meeting mit
    dem Gast; nichts verschickt.
  - Das gewöhnliche `CreateItem` lehnt ein `Organizer`-Feld ab
    (`ErrorInvalidPropertySet`), behält aber eine mitgegebene `UID`.
  - Ungemessen: ob Zusagen, Absagen und Löschen an so einem Termin sich
    verhalten wie an einer empfangenen Einladung.

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

**Doppelte.** `cal_core::Event` kennt keine UID. I4 gibt `Event` und
`NewEvent` ein `ical_uid`: gelesen, wo der Anbieter sie kennt (CalDAV, Google
`iCalUID`, Graph `iCalUId`, Exchange `UID`, lokal), geschrieben, wo er sie
annimmt. Die Liste prüft das Ziel über den Zeitraum der Termine.

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
- **I4 UID.** `ical_uid` lesen und schreiben, Doppelte in Liste und Editor.
- **I5 Handy.** „Termine importieren …“ in der Kalenderliste mit
  Dokumentwähler; „Teilen“ und „Öffnen in“: iOS als
  Dokumenttyp mit Rang `Alternate` (Aperio wird angeboten, nicht Standard),
  Android mit Intent-Filtern `VIEW` und `SEND` für `text/calendar`.
- **I6 Fremde Einladungen** (Toni, nach I0): Organisator und Teilnehmer
  bleiben, niemand bekommt etwas. Exchange schreibt sie über `MimeContent`
  mit `METHOD:REQUEST` (gemessen), iCloud mit `SCHEDULE-AGENT=CLIENT` am
  Organisator und `NONE` an der eigenen Zeile, Google über `events.import`
  (beides ungemessen), lokal behält alles. Ein Anbieter ohne stillen Weg legt
  den Termin ohne Teilnehmer an, und der Editor sagt es. Vorher ein zweiter
  Exchange-Live-Lauf: importieren, dann zusagen, absagen und löschen über
  Aperios eigenen Weg, und messen, was hinausgeht.
- **I7 Aufgaben** aus VTODO, später.
