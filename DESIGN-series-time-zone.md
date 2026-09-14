# Zeitzone einer Serie — Entwurf

Status: **entschieden, noch nicht gebaut.** Toni hat die Form am 14. September
2026 festgelegt (Entscheidungen 13b, 14a, 15a, 16b, 17b, 18a und 20a bis 24a).
Die Planung lief in zwei Runden: drei Varianten mit je einer Gegenprüfung, dann
zwei Planer (Bedienung, Unterbau) mit je einem Kritiker und einer
Zusammenführung. Der Code-Stand dahinter ist main nach #69 und #70. Die
Stufenliste am Ende sagt, was in welcher Reihenfolge gebaut wird. Nichts davon
lief bisher auf einem Gerät.

## Der Anlass

Eine Terminserie ohne Zeitzone wiederholt sich in Aperio nach UTC, in den
Ansichten wie in den Erinnerungen. Sie hält dieselbe UTC-Uhrzeit das ganze Jahr
und steht deshalb in Berlin nach jeder Zeitumstellung eine Stunde daneben.

Ob so eine Serie UTC wirklich meint oder ihre Zone unterwegs verloren hat, lässt
sich aus den Daten nicht erkennen. Google liefert `Etc/UTC` beim Lesen ohne Zone
aus, Microsoft 365 filtert `UTC`, und eine CalDAV-Zeit mit `Z` hat nie eine Zone.
Aperio selbst hat Google bisher für jede zonenlose Serie `Etc/UTC` geschickt.

Toni wollte deshalb eine Auswahl im Editor, statt Aperio raten zu lassen.

## Was eine Zone entscheidet

Ein Einzeltermin ist in Aperio ein fester Zeitpunkt. Seine Zone ändert weder,
wann er stattfindet, noch wann seine Erinnerung kommt.

Bei einer Serie entscheidet die Zone zweierlei:

- ob sie über die Zeitumstellung ihre Uhrzeit hält;
- auf welchen Wochentag sie nahe Mitternacht fällt. Sonntag 23:30 UTC ist
  Montag 01:30 in Berlin.

Nahe Mitternacht gibt es den einzigen echten Hinweis in den Daten: Die
Wochentage einer Regel passen nur zu einer der beiden Uhren. Abseits von
Mitternacht gibt es keinen, dort ist die Wahl Tonis Sache.

## Die Entscheidungen

- **13b — nur Serien, Zeiten auf ihrer Uhr.** Die Auswahl gibt es nur bei Serien
  mit Uhrzeit, beim Anlegen und beim Bearbeiten der ganzen Serie. Beginn und Ende
  werden auf der Uhr der gewählten Zone getippt und gezeigt. Eine Serie ohne
  Zone öffnet mit UTC-Zeiten. Einzeltermine bleiben, wie sie sind.
- **14a — „keine Zone“ ist UTC.** Beides ist eine einzige Wahl, angesagt als
  „UTC (ohne Sommerzeit)“. An keinen Anbieter gehen dafür neue Werte.
- **15a — nachfragen, wenn Wochentage wandern würden.** Aperio nennt, welcher
  Wochentag sich jeweils ergibt und zu welcher Uhr die Wiederholungstage
  passen. Eine Regel, die sich nicht umschreiben lässt, wird mit Grund
  abgelehnt.
- **16b — beim Anbieter geänderte Einzeltermine.** Der Uhrwechsel ist erlaubt;
  die Hilfe warnt, wie schon beim Verschieben ganzer Serien (11c).
- **17b — die volle Liste.** Rund 313 Weltzonen von Anfang an, durchsuchbar,
  erzeugt in Rust aus chrono-tz.
- **18a — Einzeltermine behalten die Anbieter-Zone.** Ein eigener, späterer Fix
  ohne Auswahl. Nicht Teil dieses Entwurfs.
- **20a — was eine Ablehnung entfernt.** Nur die Antwort „wie sie jetzt fallen“
  fällt weg, mit Grund. Der ganze Wechsel wird nur abgelehnt, wenn gar keine
  Antwort geht.
- **21a — „diesen und alle folgenden“** zeigt die Uhr der Serie, mit einem
  Hinweis auf ihre Zone, die dort nicht änderbar ist.
- **22a — Zonen, die Exchange nicht speichern kann,** stehen markiert in der
  Liste und sind in Exchange-Kalendern nicht wählbar.
- **23b — Kopien in anderen Kalendern.** Aperio bietet an, die Zone auf die
  Kopien einer Termingruppe mitzunehmen.
- **24a — Serien, deren Tage am Start hängen,** werden nicht gefragt. Die
  Ansage nach der Wahl nennt den neuen Starttag.

## Was es löst

**Aperio sagt, auf welcher Uhr eine Serie läuft.** Eine zonenlose Serie steht
ehrlich als „UTC (ohne Sommerzeit)“ da, statt stillschweigend so zu laufen.

**Eine verlorene Zone lässt sich in Aperio reparieren.** Wer die Berliner Zone
wählt, bekommt die Wochentag-Frage und kann die Serie dorthin zurückholen, wo sie
hingehört.

**Man tippt, was man meint.** „09:00 New York“ wird als 09:00 New York
eingegeben, nicht umgerechnet auf die Uhr des Geräts.

## Was es NICHT löst

Ob UTC gewollt oder verloren war, kann Aperio auch weiterhin nicht lesen. Es sagt
nur, was die Serie tut, und lässt Toni entscheiden.

Microsoft-365-Serien kommen ohne Wiederholungsregel bei Aperio an, schon
aufgeklappt in einzelne Termine. Ihre Zone lässt sich beim Anlegen wählen, danach
aber weder zeigen noch ändern.

Handy-Kalender speichern über Aperio keine Wiederholungsregel. Dort gibt es die
Auswahl nicht.

Ansichten, Agenda, Widget, Erinnerungen und Ansagen zeigen weiter die Uhrzeit
des Geräts und nennen keine Zone.

## Verhalten

### Wo die Auswahl erscheint

| Der Editor zeigt | Wiederholung | Ganztägig | Zonen-Auswahl | Beginn und Ende stehen auf |
|---|---|---|---|---|
| neuer Termin | keine | egal | nicht da | Geräte-Uhr |
| neuer Termin | ja | aus | wählbar, Vorgabe Gerätezone (oder UTC, wenn das Gerät UTC meldet) | Uhr der Serie |
| egal | ja | an | nicht da; die Zone wird nicht gespeichert | Datum des Geräts |
| gespeicherter Einzeltermin, hier zur Serie gemacht | ja | aus | wählbar, Vorgabe Gerätezone | Uhr der Serie |
| ganze Serie (der Master, seit #70) | ja | aus | wählbar | gespeicherte Zone; UTC, wenn keine |
| ganze Serie, auf „keine Wiederholung“ gestellt | – | egal | nicht da | Geräte-Uhr |
| diesen und alle folgenden | Regeländerungen gelten nicht | aus | nur ein Hinweis: „Zeitzone der Serie: New York. Ändern lässt sie sich, wenn du die ganze Serie bearbeitest.“ | Uhr der Serie (21a); der Desktop lädt dafür den Master beim Öffnen |
| nur dieser Termin | – | egal | nicht da | Geräte-Uhr |

Die Auswahl hat ihre **eigene** Sichtbarkeit nach dieser Tabelle. Sie folgt nicht
dem Wiederholungs-Block, denn der Desktop zeigt diesen in jedem Umfang. In
Kalendern, die keine Zone speichern (Handy-Kalender), ist sie ebenfalls nicht da.

### Beschriftung, Hinweis und was man hört

**Weicht die Uhr der Felder von der des Geräts ab**, heißen die vier Felder
„Startdatum (New York)“, „Startzeit (New York)“, „Enddatum (New York)“ und
„Endzeit (New York)“. Das Enddatum der Wiederholung nennt die Uhr genauso.
Verglichen wird mit kanonischen Zonen-Namen, denn V8 meldet alte Namen wie
`Asia/Calcutta` statt `Asia/Kolkata`.

**Nach der Endzeit steht dann ein Hinweis:** „Auf diesem Gerät (Berlin): Montag,
14. September, 15:00 bis 16:00“. Auf dem Desktop ist das ein fokussierbarer
Hinweis mit einem eigenen Tab-Stopp. Ein statischer Absatz wäre im
Anwendungsbereich des Dialogs für NVDA nicht erreichbar. Auf dem Handy ist es ein
Text mit einem eigenen Wisch-Stopp. Stimmen beide Uhren überein, fehlt er.

**Beim Öffnen wird nichts zusätzlich angesagt.** Eine zonenlose Serie hört man
als „Startzeit (UTC), 22:30“, gefolgt vom Hinweis.

**Die Auswahl selbst** steht nach dem Wiederholungs-Block und vor den
Erinnerungen, auf beiden Plattformen. Auf dem Desktop ist sie ein Knopf, der
einen Dialog öffnet: „Zeitzone der Serie Berlin, Europa, UTC+02:00, Schalter,
öffnet Dialog“. Auf dem Handy ist sie ein Knopf wie die übrigen Auswahlfelder:
„Zeitzone der Serie: Berlin, Europa, UTC+02:00“, Hinweis „Doppeltippen zum
Auswählen.“

### Die Zonenliste

**313 Einträge:** 312 kanonische Zonen ohne `Etc/*` und ein UTC-Eintrag. Ein
Eintrag lautet „Stadt, Region, UTC±hh:mm“, zum Beispiel „Berlin, Europa,
UTC+02:00“. Dreistufige Namen lauten „Indianapolis (Indiana), Amerika,
UTC-04:00“. Der Versatz wird in Rust für den **aktuellen Beginn im Formular**
berechnet und neu erfragt, wenn sich der Beginn ändert.

**Der UTC-Eintrag** heißt „UTC (ohne Sommerzeit)“. Er passt auch zu GMT,
`Etc/UTC` und Zulu.

**Oben stehen, solange die Suche leer ist:** „Aktuell: …“ (wenn die Zone weder
die des Geräts noch UTC ist), „Dieses Gerät: …“ (außer das Gerät läuft auf UTC)
und der UTC-Eintrag. Danach folgen alle Einträge nach Standard-Versatz, dann nach
Stadt.

**Die Suche** findet Teile von Stadt, Zonen-Id, alten Namen, Region in der
Sprache der Oberfläche und Versatz („+2“, „+02:00“, „5:30“). Anfrage und Liste
werden auf beiden Seiten mit derselben festen Tabelle gefaltet: klein, ä/ö/ü zu
a/o/u, „ue/oe/ae“ zu u/o/a, ß zu ss, kombinierende Zeichen weg. So finden
„Zürich“ und „Zuerich“ beide Zurich. Wird ein Eintrag über einen alten Namen
gefunden, sagt er das: „Kyiv, Europa, UTC+03:00 (auch Kiev)“. Die Zählzeile sagt
„Eine Zeitzone“ oder „12 Zeitzonen“, bei keinem Treffer „Keine Zeitzone passt zu
„…““.

**Markierte Einträge** stehen in der Liste, lassen sich aber nicht wählen, und
der Grund gehört zum Text:
- „(dieses Gerät kann die Zeitzone nicht verwenden)“, wenn Intl auf dem Gerät
  sie ablehnt;
- „(Exchange kann diese Zeitzone nicht speichern)“ in Exchange-Kalendern (22a).

**Gespeicherte Zonen, die nicht in der Liste stehen:**
- Löst sich der Name noch auf (ein alter Name wie `Europe/Kiev`), steht er als
  „Aktuell: Europe/Kiev (nicht in Aperios Liste)“ da, und die Felder stehen auf
  dieser Uhr.
- Löst sich nichts auf (ein Windows-Name, eine eigene TZID), steht er als
  „Aktuell: Pacific Standard Time (Aperio kann diese Zeitzone nicht lesen; die
  Serie wiederholt sich nach UTC)“ da, und die Felder stehen auf UTC, so wie
  Ansichten und Erinnerungen.
- Solange die Auswahl unberührt bleibt, wird nichts umgeschrieben.

**Desktop-Dialog:** ein verschachtelter Dialog „Zeitzone der Serie“. Das Suchfeld
hat den Fokus; die Zählzeile beschreibt es und wird nach einer Tipppause höflich
angesagt. Darunter eine Liste, in der Pfeil hoch und runter die Markierung
bewegen, Bild hoch und runter um zehn. Eingabe übernimmt den markierten Eintrag
und tut ohne Markierung nichts. Escape schließt nur die Auswahl, der Fokus geht
zurück auf den Knopf. Eine Markierung allein ändert das Formular nie.

**Handy-Dialog:** ein eigener Dialog mit Kopfzeile, Suchfeld mit dem ersten
Fokus, Zählzeile (Android per Live-Region, iOS verzögert angesagt), eine Liste mit
Auswahlzeilen und „Abbrechen“. Wer eine Zeile wählt, übernimmt und schließt.

### Die Zone wechseln

**Bei einem neuen Termin** bleiben die getippten Uhrzeiten stehen und zählen nun
auf der neuen Uhr. Wochentag und Monatstag der Wiederholung kommen aus dem Datum,
also passen Regel und Beginn immer, und die Wochentag-Frage kommt nie. Angesagt
wird: „Zeitzone der Serie: New York. Die Uhrzeiten bleiben wie getippt und
zählen jetzt auf dieser Uhr: Montag, 14. September, 09:00 bis 10:00.“ Gibt es
die getippte Uhrzeit auf der neuen Uhr nicht (Zeitumstellung im Frühjahr),
erscheint sofort der Fehler dazu.

**Bei allem Gespeicherten** (ganze Serie, oder ein gespeicherter Einzeltermin,
der hier zur Serie wird) bleibt der Zeitpunkt. Die Felder zeigen ihn auf der
neuen Uhr, ausgehend von den aktuellen Werten im Formular, also bleiben Änderungen
von vorher erhalten. Angesagt wird: „Zeitzone der Serie: New York. Die Serie
beginnt zum selben Zeitpunkt: Sonntag, 13. September, 18:30 bis 19:30 auf dieser
Uhr.“ Ein Hin- und Rückwechsel mit denselben Antworten ergibt genau die
Ausgangswerte.

### Die Wochentag-Frage (15a)

**Sie kommt direkt nach der Wahl** bei etwas Gespeichertem, wenn der Beginn auf
den beiden Uhren an verschiedenen Tagen liegt **und** die Regel Tage nennt
(Wochentage, Monatstag, Monat). Serien, deren Tage am Beginn hängen (täglich,
wöchentlich ohne Wochentage, monatlich oder jährlich bis zum 28.), werden nicht
gefragt (24a); nur wenn so eine Serie ihre Tage nicht halten kann, etwa monatlich
am 31., kommt die Frage doch.

**Die zwei Antworten**, berechnet vom Kern auf der Grundlage von series_shift:

- **„wie sie jetzt fallen“:** Die Regel wird so umgeschrieben, dass die Termine
  auf den Tagen bleiben, auf die sie jetzt fallen. Der Beginn bleibt, jede
  Ausnahme, die einen Termin streicht, wandert mit, ein UTC-UNTIL ebenso. Das
  geschieht in **einem** Schritt von der alten zur neuen Uhr, nie über die Tage
  des Geräts.
- **„wie in der Regel“:** Der Regeltext bleibt. Der Beginn wandert nur dann auf
  den passenden Tag, wenn die Wochentage der Regel **nur** zum Starttag auf der
  alten Uhr passen. Ausnahmen, die einen Termin streichen, wandern mit, ein
  UTC-UNTIL ebenso.
- **In beiden Fällen:** Eine Ausnahme, die auf der alten Uhr keinen Termin
  streicht, bleibt wörtlich stehen. So kommt bei der Reparatur auch eine Löschung
  von vor dem Zonenverlust zurück. Landet ein verschobener Beginn oder eine
  Ausnahme in der Frühjahrs-Lücke der neuen Uhr, wird diese Antwort mit Grund
  abgelehnt.

**Ein Beispiel.** Eine Serie hat ihre Berliner Zone verloren: Regel „montags“,
Beginn Sonntag 23:30 UTC. Heute wiederholt sie sich also Montag 23:30 UTC, in
Berlin dienstags. Wer Berlin wählt und „wie in der Regel“ antwortet, bekommt den
Beginn Montag 00:30 Berlin mit der Regel „montags“: repariert. Eine Serie, die
wirklich UTC meint (Regel „sonntags“, Beginn Sonntag 23:30 UTC), bekommt mit „wie
in der Regel“ den Beginn Sonntag 00:30 Berlin, mit „wie sie jetzt fallen“ Montag
00:30 Berlin mit der Regel „montags“.

**Wortlaut.**
- Titel: „An welchen Tagen soll sich die Serie wiederholen?“
- Nachricht: „In {neue Uhr} beginnt die Serie am {Beginn neu}, in {alte Uhr} am
  {Beginn alt}. Ihre Wiederholungsregel nennt {Tage}. {Passung}“ – wobei
  {Passung} „Das passt zum Start in {Uhr}.“, „… in beiden.“, „… in keiner der
  beiden.“ oder „Aperio kann nicht erkennen, zu welcher Uhr sie passt.“ lautet.
- Knöpfe: „Wiederholen am: Montag (wie in der Regel)“, „Wiederholen am: Dienstag
  (wie sie jetzt fallen)“, „UTC behalten“ (bzw. die alte Zone).
- Englisch entsprechend: „On which days should the series repeat?“, „Repeat on:
  Monday (as the rule says)“, „Repeat on: Tuesday (where they fall now)“, „Keep
  UTC“.

**Ablehnungen (20a).** Lässt sich „wie sie jetzt fallen“ nicht umsetzen, fällt
dieser Knopf weg, und die Nachricht sagt: „Die Tage, wie sie jetzt fallen, lassen
sich nicht behalten: {Grund}.“ Die Gründe sind dieselben wie beim Verschieben
ganzer Serien. Nur wenn keine Antwort möglich ist (etwa eine Regel mit festen
Stunden bei neuer Uhrzeit), wird der Wechsel selbst abgelehnt, und die Zone
bleibt.

**Danach** geht der Fokus immer zurück auf den Zonen-Knopf. Bei „alte Zone
behalten“ oder Abbrechen ändert sich nichts („Zeitzone der Serie nicht geändert:
UTC (ohne Sommerzeit)“). Bei einer Antwort wird die Zone gesetzt, die Felder
zeigen die neuen Werte, die Wochentag-Kästchen ändern sich, wenn die Regel
umgeschrieben wurde, und angesagt wird zusätzlich „Wiederholt sich am: Dienstag“.

Auf dem Desktop öffnet die Frage erst, wenn die Auswahl geschlossen ist und der
Fokus sicher auf dem Knopf liegt; die Nachricht beschreibt jeden Knopf, der erste
Fokus liegt auf „alte Zone behalten“. Auf dem Handy öffnet sie unter iOS beim
Schließen der Auswahl, unter Android im nächsten Bild, weil das Schließ-Ereignis
dort fehlt.

### Kopien in anderen Kalendern (23b)

Liegt die Serie in einer Termingruppe, bietet Aperio nach dem Speichern an, die
Zone auf die Kopien mitzunehmen. Heute trägt die Gruppe nur Titel, Beginn, Ende,
Ganztägig, Ort und Beschreibung; eine reine Zonenänderung hält den Zeitpunkt und
löst deshalb heute kein Angebot aus.

Nimmt Toni das Angebot an, bekommt jede beschreibbare Kopie dieselbe Zone, mit
derselben Antwort wie in der Wochentag-Frage. Eine Kopie, deren Regel diese
Antwort ablehnt oder deren Kalender die Zone nicht speichern kann, wird nicht
geändert und mit Grund genannt. Die genaue Form der Frage (eine gemeinsame Frage
nach der Wochentag-Frage, oder die bestehende Mitnahme-Frage um die Zone
erweitert) wird in dieser Stufe festgelegt und vorher gemessen.

### Wiederholung, Ganztägig, Umfänge

**Wiederholung aus:** Der Zeitpunkt bleibt, die Felder gehen zurück auf die
Geräte-Uhr, Zusatz und Hinweis verschwinden. Einmal angesagt: „Keine
Wiederholung. Die Zeiten stehen jetzt in der Uhrzeit dieses Geräts: …“

**Wiederholung wieder an** in derselben Sitzung: Die frühere Zone kommt zurück
(bei einer gespeicherten Serie die gespeicherte), der Zeitpunkt bleibt,
angesagt wie eine Wahl.

**Ganztägig an:** Die Daten bleiben, Auswahl und Hinweis verschwinden, die Zone
wird nicht gespeichert, wenn Ganztägig in dieser Sitzung eingeschaltet wurde. Eine
ganztägige Serie, unberührt gespeichert, behält ihren Wert wörtlich.

**Ganztägig aus:** Die Uhrzeiten kommen auf der gemerkten Uhr zurück.

**Nur dieser Termin:** Geräte-Uhr, keine Zone. **Diesen und alle folgenden:** Uhr
der Serie (21a), der Rest behält die Zone des Masters.

**Titelvorschläge** füllen die Regel, lassen die Zone aber stehen.

### Prüfen und Speichern

- **Lücke:** Einen getippten Beginn oder ein Ende, das es auf der Uhr nicht gibt,
  lehnt Aperio beim Speichern ab, sichtbar sobald die Uhrzeit feststeht: „{Zeit}
  gibt es am {Datum} in {Uhr} nicht: Die Uhren werden vorgestellt. Wähle eine
  andere Uhrzeit.“ Einzeltermine behalten das heutige Verhalten.
- **Doppelte Stunde im Herbst:** Die frühere der beiden Möglichkeiten gilt.
- **Unberührte Werte** speichern die gespeicherten Zeitpunkte unverändert.
- **Eine Speicherfunktion** ersetzt `editedRecurrence` an jeder Stelle. Heute
  würde eine Wahl nie gespeichert, weil `editedRecurrence` die alte Zone behält.
  Unberührt heißt: der gespeicherte Wert wörtlich; gewählt: die kanonische Zone;
  UTC: keine Zone.
- **Anlegen aus dem Editor** stempelt nie die Gerätezone über eine gewählte UTC.
  Duplizieren, Kopieren und Verschieben behalten das heutige Stempeln, mit
  kanonischer Gerätezone.
- **UNTIL:** Ein unberührtes Ende bleibt roh. Ein geändertes Enddatum wird als
  23:59:59 dieses Datums auf der Uhr der Serie gespeichert, als UTC-Zeitpunkt. Ein
  UTC-UNTIL wandert nur über den Zonenwechsel. WKST bleibt erhalten.

### UTC und Anbieter

- **Was als UTC gilt:** keine oder eine leere Zone, `Etc/UTC` und seine Aliase,
  `Etc/GMT` und seine Aliase mit Versatz null, ohne Groß- und Kleinschreibung.
  Eine einzige Kern-Regel, die Ansichten, Liste und Geräteprüfung benutzen.
  `Etc/GMT±N` bleibt eine benannte Zone „nicht in der Liste“.
- **Was die Anbieter für „keine Zone“ bekommen**, bleibt wie heute: Google
  `Etc/UTC`, Microsoft 365 `UTC`, CalDAV `Z`, Exchange keine Zone.
- **Exchange:** Erst messen, was ein Update ohne StartTimeZone mit einer Serie
  macht, die eine Zone hat. Lässt sich das Feld entfernen, entfernt ein Wechsel
  auf UTC es. Sonst wird der Wechsel einer Exchange-Serie auf UTC mit Grund
  abgelehnt. Zonen, die Aperios Windows-Tabelle nicht abbilden kann, sind nach
  22a markiert und nicht wählbar; eine neue Serie mit so einer Gerätezone beginnt
  auf UTC, und Aperio sagt warum; Kopieren oder Verschieben nach Exchange wird mit
  der Zone im Grund abgelehnt.
- **Exchange und Microsoft 365** leiten Start- und Enddatum und die Standard-Tage
  heute aus dem UTC-Datum ab. Beide müssen sie auf der Uhr der Serie lesen, bevor
  die Editoren Beginn und UNTIL auf diese Uhr stellen.
- **Handy-Kalender** speichern keine Regel; die Auswahl ist dort nicht da.

### Nach dem Speichern

Ansichten, Agenda, Widget, Erinnerungen, Benachrichtigungen und Ansagen bleiben
auf der Uhr des Geräts. Ein Sync-Konflikt, bei dem sich nur die Zone
unterscheidet, zeigt weiter die rohen Daten, solange die optionale letzte Stufe
nicht gebaut ist.

### Hilfe

Ein neuer Abschnitt „Zeitzone einer Serie“ in `03-events.md` (Deutsch und
Englisch):

1. Was die Zone entscheidet.
2. Wo die Auswahl erscheint.
3. Uhrzeiten werden auf dieser Uhr getippt (Feldbeschriftung, Hinweis zum
   Gerät).
4. UTC (ohne Sommerzeit) heißt: keine Zone.
5. Wechseln: Bei einem neuen Termin bleiben die getippten Zeiten, bei einem
   gespeicherten der Zeitpunkt; im anderen Sommerzeit-Halbjahr verschiebt sich
   die Serie um eine Stunde.
6. Die Wochentag-Frage, mit dem Berlin-Beispiel und den Ablehnungen.
7. Suchen in der Liste; der Versatz ist der am Beginn der Serie.
8. Warnung nach 16b: Beim Anbieter geänderte Einzeltermine bleiben an ihrer
   alten Zeit hängen; Verweis auf den Abschnitt „Einzeln geänderte oder gelöschte
   Termine“.
9. Kopien in anderen Kalendern (23b).
10. Grenzen: Microsoft-365-Serien, Handy-Kalender, Exchange, Anzeigen auf der
    Geräte-Uhr, Sync-Konflikte.

In „Fehlersuche & Protokolle“: Eine Serie, die ihre Zone verloren hat, zeigt
UTC-Zeiten. Ganze Serie öffnen, Zone wählen, Wochentag-Frage beantworten.

## Stufenliste

Jede Stufe ist ein eigener PR. Wo die Oberfläche berührt wird, kommen Desktop und
Handy im selben PR.

1. **Handy-Tür zu series_shift** — `feat(mobile): a door into cal_core::series_shift`.
   cal-ffi-Export, Kotlin- und Swift-Funktion, `CalFfiModule.ts`, Installation
   in `mobile/index.ts`, Bindungen neu erzeugt. Schließt die Lücke aus #69.
2. **Zonennamen und eine UTC-Regel** — `feat(core): generated zone names and one UTC clock`.
   `cargo xtask tz-list [--check]` erzeugt aus chrono-tz die 312 kanonischen
   Namen, die Alias-Tabelle und die UTC-Menge. Kern-Regeln `canonical_zone` und
   `series_clock_zone`, Türen in WebAssembly und cal-ffi. `zoneOrNull` und
   `localTimeZone` laufen darüber. Gemeinsame Fixture `seriesClock.json`.
   Vorher messen: Größe des WebAssembly-Moduls, Kosten des Aufrufs in großen
   Ansichten, welche Zonen-Namen WebView2, Hermes auf iOS und Android für UTC,
   Indien und die Ukraine melden.
3. **Die Weltliste mit Versatz und Suche** — `feat(core): the world zone list with offsets, and its search`.
   Kern-Feature `zones` (in WebAssembly aus): Liste mit Versatz und Aliasen,
   Suche mit Faltungstabelle. Desktop über einen Tauri-Befehl, Handy über
   cal-ffi. Fixture `timeZoneFilter.json`. Vorher messen: Größenzuwachs nativ,
   Abweichungen zwischen chrono-tz und Intl der Geräte, welche Namen Hermes auf
   alten Geräten ablehnt, wie NVDA und VoiceOver „UTC+02:00“ und „UTC+05:30“
   lesen.
4. **Exchange-Zonentabelle aus CLDR** — `feat(ews): Windows zone table generated from CLDR windowsZones`.
   Vorher messen: wie viele der 312 Zonen danach abbildbar sind, und was Exchange
   mit einem Update ohne StartTimeZone macht.
5. **Exchange und Microsoft 365 lesen Serien-Daten auf der Uhr der Serie** — `fix(ews, graph): read a series' dates on its own clock when writing`.
6. **Kalender melden, welche Zonen sie speichern** — `feat(plugin-core): calendars declare which series time zones they can store`.
   `time_zones`: alle, keine (Handy-Kalender) oder nur bestimmte (Exchange). Das
   Handy muss dafür auch die eingebauten Manifeste lesen.
7. **Datum, Uhrzeit und Regel auf einer Serien-Uhr** — `feat(shared): dates, times and repeat rules on a series clock`.
   Umrechnung mit Lücke und Doppelstunde, Formularfelder mit Uhr, Regel-Ableitung
   aus dem Tages-Schlüssel, WKST, UNTIL. Noch unsichtbar. Fixture
   `wallClock.json`. Vorher messen: ob der Datums-Picker auf dem Handy eine feste
   Zone annimmt, und ob Node auf den CI-Linux-Rechnern IANA-TZ-Werte beachtet.
8. **Die Kern-Regel für den Uhrwechsel (15a)** — `feat(core): changing the clock a series repeats on`.
   `series_clock_change` mit Türen, `seriesClock.ts` in shared, Fixture
   `seriesClockChange.json`.
9. **Das Formular-Modell** — `feat(shared): the series clock form model`.
   Sichtbarkeit nach der Tabelle, Vorbelegung, Wechsel, Speicherfunktion.
10. **Die Auswahldialoge** — `feat(editors): the series time zone picker dialogs`.
    Desktop und Handy. Vorher messen: NVDA im verschachtelten Dialog, VoiceOver
    und TalkBack mit einer langen Liste.
11. **Die Editoren** — `feat(editors): times typed on the series clock, and the weekday question`.
    Beschriftungen, Hinweis, Auswahl, Wochentag-Frage, „diesen und alle folgenden“
    lädt auf dem Desktop den Master (21a), Hilfe und Fehlersuche. Vorher messen:
    ob WebView2 bei jedem Pfeil im Wiederholungs-Feld ein Änderungsereignis
    schickt, Fokusfolge von Auswahl zu Frage, Ansage nach dem Schließen auf iOS.
12. **Kopien nehmen die Zone mit (23b)** — `feat(groups): carry a series' time zone to its copies`.
    Die Gruppen-Mitnahme im Kern trägt die Zone; das Angebot nach dem Speichern
    nennt sie; Kopien, die ablehnen, werden mit Grund genannt.
13. **Exchange sagt, was es nicht speichern kann** — `fix(ews): keep what Exchange can store, and say when it cannot`.
    Nach der Messung aus Stufe 4.
14. **Optional: lesbare Sync-Konflikte bei reiner Zonenänderung** — `feat(sync): say what changed when only a series' zone differs`.

## Risiken

- **Zwei Zonen-Datenbanken.** Der Versatz kommt aus chrono-tz, Anzeige und
  Aufklappen aus Intl des Geräts. Für Zonen, die sich seit der chrono-tz-Version
  geändert haben, können Versatz im Eintrag und Uhrzeit in den Feldern
  auseinanderliegen.
- **Versatz-Ansagen ungetestet.** Liest ein Screenreader „UTC+02:00“ schlecht,
  sind 313 ähnliche Einträge schwer zu unterscheiden. Die Form steckt in einem
  Übersetzungsschlüssel und lässt sich ohne Rust ändern.
- **Exchange-Wechsel auf UTC.** Behält Exchange die alte Zone, wenn keine
  geschickt wird, und lässt sie sich nicht entfernen, muss dieser Wechsel
  abgelehnt werden.
- **Städtenamen aus Exchange.** Eine Wiener Serie kommt nach dem Aktualisieren
  als Berlin zurück (dieselbe Uhr).
- **Beim Anbieter geänderte Einzeltermine** behalten nach einem Uhrwechsel ihren
  alten Zeitpunkt und können doppelt erscheinen oder Lücken lassen. Nur die Hilfe
  warnt (16b).
- **Lücken werden abgelehnt.** Serien auf der Gerätezone, die eine
  Frühjahrs-Uhrzeit bisher still gerundet haben, lassen sich so nicht mehr
  speichern.
- **Altes UNTIL im Osten.** Zonen-Serien östlich von UTC mit einem UNTIL als
  `<Datum>T235959Z` zeigen ihr Ende einen Tag später. Roh bleibt es, solange es
  unberührt ist; wer das angezeigte Datum neu tippt, verlängert die Serie um
  einen Tag.
- **Lange Listen auf dem Handy.** Virtualisierte Listen können Zeilen vor dem
  Wischen verbergen; alle 313 Zeilen zu zeichnen kann auf alten Android-Geräten
  langsam sein.
- **Große Editor-Stufe ohne Handy-Tests.** Stufe 11 berührt zwei große Editoren.
  Die Parität ruht darauf, dass die Logik in shared liegt und dort getestet ist.
- **Microsoft 365.** Eine dort mit Zone angelegte Serie lässt sich danach in
  Aperio weder zeigen noch korrigieren.
- **Kosten im Aufklappen.** Die UTC-Tür läuft in jedem Aufklappen; ihre Kosten in
  großen Ansichten sind ungemessen.
- **Erzeugte Tabellen.** Die Generatoren hängen daran, dass chrono-tz seine
  Quelldaten mitliefert und die CLDR-Datei eingecheckt ist; ein Update lässt
  `--check` laut scheitern.

## Ungeprüft

- Wie NVDA (deutsche und englische Stimme) und VoiceOver die Versatz-Texte,
  „Startzeit (New York)“ und die Einträge mit Kommas lesen.
- NVDA im verschachtelten Dialog mit Suchfeld und Liste; ob der Fokus-Rücksprung
  das Schließen der Auswahl und das Öffnen der Frage übersteht.
- iOS: ob eine Ansage nach dem Schließen eines Dialogs gehört wird. Android: ob
  das Öffnen im nächsten Bild mit TalkBack funktioniert. Ob Listenzeilen per
  Wischen erreichbar bleiben.
- Welche Zonen-Namen Intl auf Android 7 bis 9 und iOS 16.4 ablehnt.
- Welche Gerätezone WebView2, iOS und Android für UTC, Indien und die Ukraine
  melden.
- Ob der Datums-Picker auf dem Handy eine feste Zone annimmt.
- Was Exchange bei einem Update ohne StartTimeZone macht und ob sich das Feld
  entfernen lässt; wie viele Zonen nach der CLDR-Tabelle abbildbar sind.
- Ob Google und Microsoft 365 alle 312 kanonischen Namen annehmen und
  zurückgeben, besonders umbenannte (Europe/Kyiv, America/Ciudad_Juarez); ob iCloud
  TZIDs mit erzeugten VTIMEZONEs behält.
- Abweichungen zwischen chrono-tz und den Intl-Daten der Geräte.
- Größen und Aufrufkosten.
- Ob Node auf den CI-Linux-Rechnern IANA-TZ-Werte beachtet (auf Windows wurde
  `America/Los_Angeles` ignoriert).
