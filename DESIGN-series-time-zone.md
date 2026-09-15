# Zeitzone einer Serie — Entwurf

Status: **entschieden; Stufen 1 bis 3 gebaut (#72, #73, #74), Stufe 4 in Arbeit.**
Toni hat die Form am 14. und 15. September 2026 festgelegt (Entscheidungen 13b,
14a, 15a, 16b, 17b, 18a, 20a, 21a, 22a, 23b, 24a, 25b, 26a, 29a bis 32a, 38a
bis 43b, 46a bis 49a und 51a bis 53a, dazu die Vorlese-Form F). Die Planung lief in zwei
Runden: drei Varianten mit je einer Gegenprüfung, dann zwei Planer (Bedienung,
Unterbau) mit je einem Kritiker und einer Zusammenführung. Danach wurde dieses
Dokument selbst gegen die Entscheidungen, den Code und die Planung geprüft. Die
Stufenliste am Ende sagt, was in welcher Reihenfolge gebaut wird. Nichts davon
lief bisher auf einem Gerät.

## Der Anlass

Eine Terminserie ohne Zeitzone wiederholt sich in Aperio nach UTC, in den
Ansichten wie in den Erinnerungen. Sie hält dieselbe UTC-Uhrzeit das ganze Jahr
und steht deshalb in Berlin im anderen Sommerzeit-Halbjahr eine Stunde daneben.

Ob so eine Serie UTC wirklich meint oder ihre Zone unterwegs verloren hat, lässt
sich aus den Daten nicht erkennen. Aperio liest `Etc/UTC` von Google und `UTC`
von Microsoft 365 als „keine Zone“, und eine CalDAV-Zeit mit `Z` hat nie eine
Zone. Aperio selbst hat Google bisher für jede zonenlose Serie `Etc/UTC`
geschickt.

Toni wollte deshalb eine Auswahl im Editor, statt Aperio raten zu lassen.

## Was eine Zone entscheidet

Ein Einzeltermin ist in Aperio ein fester Zeitpunkt. Seine Zone ändert weder,
wann er stattfindet, noch wann seine Erinnerung kommt.

Bei einer Serie entscheidet die Zone zweierlei:

- ob sie über die Zeitumstellung ihre Uhrzeit hält;
- auf welchen Wochentag sie nahe Mitternacht fällt. Sonntag 23:30 UTC ist im
  Winter Montag 00:30 in Berlin.

Nahe Mitternacht gibt es den einzigen echten Hinweis in den Daten: Die Tage einer
Regel passen nur zu einer der beiden Uhren. Abseits von Mitternacht gibt es
keinen, dort ist die Wahl Tonis Sache.

## Die Entscheidungen

- **13b — nur Serien, Zeiten auf ihrer Uhr.** Die Auswahl gibt es nur bei Serien
  mit Uhrzeit, beim Anlegen und beim Bearbeiten der ganzen Serie. Beginn und Ende
  werden auf der Uhr der gewählten Zone getippt und gezeigt. Eine Serie ohne
  Zone öffnet mit UTC-Zeiten. Einzeltermine bleiben, wie sie sind.
- **14a — „keine Zone“ ist UTC.** Beides ist eine einzige Wahl, angesagt als
  „UTC (ohne Sommerzeit)“. An keinen Anbieter gehen dafür neue Werte.
- **15a — nachfragen, wenn Tage wandern würden.** Aperio nennt, welcher Tag sich
  jeweils ergibt und zu welcher Uhr die Wiederholungstage passen. Eine Regel, die
  sich nicht umschreiben lässt, wird mit Grund abgelehnt.
- **16b — beim Anbieter geänderte Einzeltermine.** Der Zonenwechsel ist erlaubt;
  die Hilfe warnt, wie schon beim Verschieben ganzer Serien (11c).
- **17b — die volle Liste.** Rund 313 Weltzonen von Anfang an, durchsuchbar,
  erzeugt in Rust aus chrono-tz.
- **18a — Einzeltermine behalten die Anbieter-Zone.** Ein eigener, späterer Fix
  ohne Auswahl. Nicht Teil dieses Entwurfs.
- **20a — was eine Ablehnung entfernt.** Nur die Antwort, die sich nicht umsetzen
  lässt, fällt weg, mit Grund. Der ganze Wechsel wird nur abgelehnt, wenn gar
  keine Antwort geht.
- **21a — „diesen und alle folgenden“** zeigt die Uhr der Serie, mit einem
  Hinweis auf ihre Zone, die dort nicht änderbar ist.
- **22a — Zonen, die Exchange nicht speichern kann,** stehen markiert in der
  Liste und sind in Exchange-Kalendern nicht wählbar.
- **23b — Kopien in anderen Kalendern.** Aperio bietet an, die Zone auf die
  Kopien einer Termingruppe mitzunehmen.
- **24a — Serien, deren Tage am Beginn hängen,** werden nicht gefragt, außer sie
  können ihre Tage nicht halten. Die Ansage nach der Wahl nennt den neuen Tag des
  Beginns.
- **25b — zusammengelegte Orte.** Die Liste bleibt bei 312 Zonen. Einen Ort, den
  tzdata mit einer anderen Stadt zusammengelegt hat (Oslo mit Berlin), findet
  die Suche als „Berlin (auch Oslo)“. Gespeichert wird die Zone, auf die er
  verweist.
- **26a — eine Regel für gespeicherte Zonen und die Gerätezone.**
  1. Eine Zone gilt nur, wenn tzdata den Namen kennt und er kein UTC-Name ist.
     Sonst wiederholt sich die Serie nach UTC, auch bei einem Versatz wie
     `+05:30` und bei einer Gerätezone, die tzdata nicht kennt.
  2. Alle 18 UTC-Namen gelten als keine Zone.
  3. Die Erinnerungen fragen dieselbe Regel wie die Ansichten.
  4. Bis Stufe 10 bekommt eine neue Serie die Gerätezone in der Schreibweise des
     Geräts, etwa `Asia/Calcutta`.
- **29a — Normalzeit ab heute.** Die Liste sortiert nach dem kleinsten Versatz
  einer Zone im Jahr ab heute. Dublin und Casablanca stehen damit bei UTC+00:00
  neben London, und jede Serie sieht dieselbe Reihenfolge, auch eine alte.
- **30a — feste Reihenfolge.** Normalzeit, dann Stadt, wie oben. Im Sommer hört
  man dadurch manchmal einen kleineren Versatz nach einem größeren („Adak,
  Amerika, UTC−09:00“ vor „Honolulu, Pazifik, UTC−10:00“).
- **31a — „+5“ ist die volle Stunde.** Ein Versatz ohne Minuten findet nur
  UTC+05:00; für Indien tippt man „+5:30“.
- **32a — tzdata 2025b.** Stufe 3 wird mit den Zeitzonendaten aus chrono-tz
  gebaut. Die Aktualität der Daten ist eine eigene Aufgabe (siehe „Risiken“).
- **Vorlese-Form F.** Der Versatz lautet „UTC−04:00“ mit echtem Minuszeichen
  (U+2212); Toni hat die Formen mit NVDA und VoiceOver verglichen.
- **38a — Live-Test vor dem Merge von Stufe 4.** Stufe 4 schreibt für 170
  gelistete Zonen, die bisher ohne Zone an Exchange gingen, zum ersten Mal eine
  Zone, darunter 31 Windows-Namen, die Aperio nie geschickt hat. Toni schickt
  die Testanfragen, die aus Aperios Code erzeugt sind, vor dem Merge an seinen
  Exchange-Server.
- **39 — der Testserver** ist ein eigener Exchange-Server (on-premises). Die
  Abschaltung von EWS in Exchange Online ab Oktober 2026 betrifft ihn nicht.
- **40a — Lizenzhinweis bei den Daten.** Die CLDR-Datei liegt mit ihrer
  Unicode-Lizenz V3 und einer Herkunftsdatei in `crates/adapter-ews/cldr/`, und
  der Kopf der erzeugten Tabelle nennt sie. Eine Seite mit Open-Source-Hinweisen
  in beiden Apps ist eine eigene Aufgabe.
- **41a — der Server wird gefragt.** Ein Exchange-Server lehnt einen
  Windows-Namen, den er nicht kennt, mit `ErrorTimeZone` ab, und das ganze
  Speichern scheitert. Der Adapter fragt deshalb pro Konto einmal, welche Namen
  der Server kennt (`GetServerTimeZones`), und schreibt nur diese. Eine Zone,
  deren Name der Server nicht kennt, geht ohne Zone raus; Stufe 7 sagt es der
  Person.
- **42a — ganztägige Serien erst nach einem zweiten Test.** Eine ganztägige
  Serie mit Zone verschiebt Exchange auf die Tagesgrenzen der Zone. Welche Regel
  eine ganztägige Serie auf ihrem Tag lässt, klärt ein zweiter kurzer
  Live-Test; bestehende kaputte Serien repariert Stufe 5.
- **43b — die Endzone wird gelesen.** Eine Serie ohne Zone speichert Exchange
  mit der Startzone `Greenwich Standard Time` und der Endzone
  `tzone://Microsoft/Utc`. Diese Endzone heißt: keine Zone.
- **46a — eine ganztägige Serie schreibt nie eine Zone.** Die zweite Runde hat
  gezeigt: Eine Zone macht eine ganztägige Exchange-Serie länger. Stufe 4 bekommt
  deshalb nur diesen Schutz. Die Regel steht einmal im Kern, und die Adapter
  fragen sie. Lesefehler, Update-Regel und Datumsfehler werden eigene Stufen
  nach der dritten Runde.
- **47a — Änderungen behalten die gespeicherte Zone.** Ändert Aperio einen
  ganztägigen Exchange-Termin, der schon eine Zone trägt, etwa aus Outlook,
  gehen Beginn und Ende auf Mitternacht in dieser Zone. Die Zone selbst bleibt
  unberührt. Das passt zu 18a und dazu, dass Unberührtes wörtlich bleibt. Die
  dritte Runde bestätigt es vor dem Bau.
- **48a — ganztägige Serien wiederholen sich an Kalendertagen.** Aperio
  wiederholt ganztägige Serien heute in UTC. Östlich von UTC landet ein
  genannter Wochentag deshalb einen Tag zu spät, bei jedem Anbieter außer
  Microsoft 365 und dem Gerätekalender. Eine Kern-Regel behebt das: Eine
  ganztägige Serie wiederholt sich an den Kalendertagen des Geräts, egal welche
  Zone sie trägt. Ansichten, Erinnerungen, Widget und Badge fragen dieselbe
  Regel. Das wird ein eigener PR.
- **49a — eine gezielte dritte Runde.** Siehe Stufe 4, „Gemessen“.
- **51a bis 53a — wie die dritte Runde läuft.** Toni legt die ganztägigen
  Serien und Termine selbst in Outlook im Web an (51a) und macht die Ausnahme
  selbst (52a). Er meldet eine kurze Liste zurück (53a). Die übrigen
  gespeicherten Werte liest das Skript selbst nach.

Drei Festlegungen folgen aus diesen Entscheidungen und kamen erst bei der Prüfung
des Dokuments hinzu; sie stehen in den Abschnitten unten:

- Kopieren und Duplizieren behalten die Zonen-Entscheidung der Quelle (aus 5a und
  14a: nicht raten).
- Eine Regel, die nur den Tag nennt, auf den der Beginn ohnehin fällt, gilt als
  „Tage hängen am Beginn“ (sonst hinge die Frage davon ab, welche Felder zuletzt
  angeklickt wurden).
- Die Exchange-Stufe kommt vor die Editoren, damit ein Wechsel nie still
  verpufft.

## Was es löst

**Aperio sagt, auf welcher Uhr eine Serie läuft.** Eine zonenlose Serie steht
ehrlich als „UTC (ohne Sommerzeit)“ da, statt stillschweigend so zu laufen.

**Eine verlorene Zone lässt sich in Aperio reparieren.** Wer die Berliner Zone
wählt, stellt die Serie wieder auf ihre Uhr. Liegt sie nahe Mitternacht, kommt
die Wochentag-Frage, und die Serie lässt sich dorthin zurückholen, wo sie
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

Die Tabelle hat vier Spalten: was der Editor zeigt, ob wiederholt wird, ob
ganztägig, was mit der Zonen-Auswahl ist, und auf welcher Uhr Beginn und Ende
stehen.

| Der Editor zeigt | Wiederholung | Ganztägig | Zonen-Auswahl | Beginn und Ende stehen auf |
|---|---|---|---|---|
| neuer Termin | keine | egal | nicht da | Geräte-Uhr |
| neuer Termin | ja | aus | wählbar; Vorgabe Gerätezone, UTC, wenn das Gerät UTC oder eine Zone meldet, die tzdata nicht kennt (26a), oder der Kalender die Gerätezone nicht speichern kann (22a) | Uhr der Serie |
| egal | ja | an | nicht da; die Zone wird nicht gespeichert, wenn Ganztägig hier eingeschaltet wurde | Datum des Geräts |
| gespeicherter Einzeltermin, hier zur Serie gemacht | ja | aus | wählbar; Vorgabe wie beim neuen Termin | Uhr der Serie |
| ganze Serie | ja | aus | wählbar | gespeicherte Zone; UTC, wenn keine |
| ganze Serie, auf „keine Wiederholung“ gestellt | – | egal | nicht da | Geräte-Uhr |
| diesen und alle folgenden | Regeländerungen gelten nicht | aus | nur ein Hinweis: „Zeitzone der Serie: New York. Ändern lässt sie sich, wenn du die ganze Serie bearbeitest.“ | Uhr der Serie (21a) |
| nur dieser Termin | – | egal | nicht da | Geräte-Uhr |
| Umfang-Auswahl im Formular (Ausweichweg ohne vorherige Frage) | egal | egal | nicht da | Geräte-Uhr |

Die Auswahl hat ihre **eigene** Sichtbarkeit nach dieser Tabelle. Sie folgt nicht
dem Wiederholungs-Block, denn der Desktop zeigt diesen in jedem Umfang. In
Kalendern, die keine Zone speichern (Handy-Kalender), ist sie ebenfalls nicht da.

**Diesen und alle folgenden (21a):** Der Desktop lädt dafür die ganze Serie schon
beim Öffnen, wie das Handy. Gelingt das nicht, sagt der Editor es mit der
Meldung, die heute beim Speichern kommt, und fällt nicht still auf die
Geräte-Uhr zurück.

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
als „Startzeit (UTC), 22:30“; nach der Endzeit folgt der Hinweis.

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
UTC−04:00“. Das Minus ist das echte Minuszeichen U+2212 (Form F). Die Stadt ist
der letzte Teil der Id mit `_` als Leerzeichen; Namen ohne Schrägstrich wie
`EST5EDT` stehen nie in der Liste, auch wenn tzdata sie wieder zu Zonen macht.

**Städte stehen unter ihrem Namen aus der Zonen-Datenbank** (Vienna, Moscow,
Kyiv); nur die Region steht in der Sprache der Oberfläche. „Wien“ findet deshalb
nichts.

**Der Versatz** wird in Rust für den aktuellen Beginn im Formular berechnet und
neu erfragt, wenn sich der Beginn ändert. Jede Antwort gehört zu dem Beginn, für
den sie erfragt wurde; bis sie kommt, bleibt die letzte Beschriftung stehen, ohne
Antwort steht der Eintrag ohne Versatz.

**Der UTC-Eintrag** heißt „UTC (ohne Sommerzeit)“. Er passt auch zu GMT,
`Etc/UTC` und Zulu.

**Oben stehen, solange die Suche leer ist:** „Aktuell: {Eintrag}“ (wenn die Zone
weder die des Geräts noch UTC ist), „Dieses Gerät: {Eintrag}“ (außer das Gerät
läuft auf UTC oder auf einer Zone, die tzdata nicht kennt) und der UTC-Eintrag.
Danach folgen alle Einträge nach Normalzeit, dann nach Stadt. Die Normalzeit ist
der kleinste Versatz einer Zone im Jahr ab heute (29a); die Reihenfolge bleibt
fest, auch wenn der angezeigte Versatz im Sommer größer ist (30a). Der
UTC-Eintrag steht dort vor den Zonen mit Normalzeit UTC+00:00.

**Die Suche** findet Teile von Stadt, Gebiet, Zonen-Id, alten Namen, Region in
der Sprache der Oberfläche und Versatz („+2“, „+02:00“, „UTC+2“, „5:30“). Jedes
Wort muss passen; eine Suche ohne Wort zeigt die ganze Liste.

Ein Versatz braucht ein Vorzeichen, einen Doppelpunkt oder „UTC“ davor, und ohne
Minuten gilt die volle Stunde: „+5“ findet UTC+05:00, nicht Kolkata (31a). Ohne
Vorzeichen passt ein Versatz zu beiden Seiten („5:30“). Er passt zu dem Versatz,
den der Eintrag am Beginn der Serie zeigt, also in ganzen Minuten: Die Pariser
Ortszeit von 1900 (+00:09:21) steht als „UTC+00:09“ in der Liste und wird mit
„+00:09“ gefunden. Die Sekunden fallen zur Null hin weg; weniger als eine Minute
westlich von UTC heißt „UTC+00:00“.

Anfrage und Liste werden auf beiden Seiten gleich gefaltet: zerlegt und ohne
kombinierende Zeichen (ä wird a), klein, ß zu ss, „ue/oe/ae“ zu u/o/a. So finden
„Zürich“ und „Zuerich“ beide Zurich. Ein Wortteil passt auch, wenn er ohne die
letzte Regel im Namen steht: „enix“ findet Phoenix, obwohl Phoenix gefaltet
„phonix“ heißt.

Alte Namen findet die Suche unter dem Namen, den die Liste ihnen gibt („Kiev“,
„Oslo“, „US/Pacific“); ein Wort mit Schrägstrich findet sie auch als ganze Id
(„Europe/Kiev“). Wird ein Eintrag über einen alten Namen gefunden, sagt er das:
„Kyiv, Europa, UTC+03:00 (auch Kiev)“. Zusammengelegte Orte und andere
Schreibweisen heißen dabei nach ihrem letzten Teil („auch Oslo“), außer der
letzte Teil ist die Stadt selbst („auch Asia/Istanbul“). Alle anderen alten Namen
heißen mit ihrer ganzen Id („auch US/Pacific“, „auch CST6CDT“), weil ihr letzter
Teil allein nichts sagt.

Die Zählzeile sagt „Eine Zeitzone“ oder „12 Zeitzonen“. Bei keinem Treffer sagt
sie: Keine Zeitzone passt zu „{Suche}“.

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

**Eine gespeicherte Zone, die Intl auf diesem Gerät ablehnt** (alte
Android-Zeitzonendaten): Die Felder stehen auf UTC, und nach der Endzeit steht
„Dieses Gerät kann die Zeitzone Kyiv nicht verwenden; die Zeiten stehen in UTC.“
Geänderte Zeiten oder eine geänderte Regel lassen sich dann mit diesem Grund
nicht speichern; eine andere Zone zu wählen geht.

**Desktop-Dialog:** ein verschachtelter Dialog „Zeitzone der Serie“. Das Suchfeld
hat den Fokus; die Zählzeile wird als Beschreibung des Suchfelds vorgelesen und
nach einer Tipppause höflich angesagt. Darunter eine Liste, in der Pfeil hoch und
runter die Markierung bewegen, Bild hoch und runter um zehn. Die Eingabetaste
übernimmt den markierten Eintrag und tut ohne Markierung nichts. Escape schließt
nur die Auswahl, der Fokus geht zurück auf den Knopf. Eine Markierung allein
ändert das Formular nie.

**Handy-Dialog:** ein eigener Dialog mit Kopfzeile, Suchfeld mit dem ersten
Fokus, Zählzeile (Android per Live-Region, iOS verzögert angesagt), eine Liste mit
Auswahlzeilen und „Abbrechen“. Wer eine Zeile wählt, übernimmt und schließt.

### Die Zone wechseln

**Bei einem neuen Termin** bleiben die getippten Uhrzeiten stehen und gelten nun
auf der neuen Uhr. Wochentag und Monatstag der Wiederholung kommen aus dem Datum,
also passen Regel und Beginn immer, und die Wochentag-Frage kommt nie. Angesagt
wird: „Zeitzone der Serie: New York. Die Uhrzeiten bleiben wie getippt und
gelten jetzt auf dieser Uhr: Montag, 14. September, 09:00 bis 10:00.“ Gibt es die
getippte Uhrzeit auf der neuen Uhr nicht (Zeitumstellung im Frühjahr), erscheint
sofort der Fehler dazu.

**Bei allem Gespeicherten** (ganze Serie, oder ein gespeicherter Einzeltermin,
der hier zur Serie wird) bleibt der Zeitpunkt. Die Felder zeigen ihn auf der
neuen Uhr, ausgehend von den aktuellen Werten im Formular, also bleiben Änderungen
von vorher erhalten. Angesagt wird: „Zeitzone der Serie: New York. Die Serie
beginnt zum selben Zeitpunkt: Sonntag, 13. September, 18:30 bis 19:30 auf dieser
Uhr.“ Ein Hin- und Rückwechsel mit denselben Antworten ergibt genau die
Ausgangswerte.

### Die Wochentag-Frage (15a)

**Sie kommt direkt nach der Wahl** bei etwas Gespeichertem, wenn der Beginn auf
den beiden Uhren an verschiedenen Tagen liegt und außerdem eins von beiden gilt:
Die Regel nennt Tage (Wochentage, Monatstag, Monat), oder die Serie kann ihre Tage
auf der neuen Uhr nicht halten (etwa monatlich am 31.). Gefragt wird gegen Regel,
Zone und Ausnahmen, wie sie gerade im Formular stehen.

**Nicht gefragt werden Serien, deren Tage am Beginn hängen (24a):** täglich,
wöchentlich ohne Wochentage, monatlich oder jährlich ohne genannten Tag bis zum
28. Nennt die Regel nur den Tag, auf den der Beginn ohnehin fällt (so schreibt
Aperios Editor den Monatstag, sobald man die Wiederholung weiter bearbeitet),
zählt sie ebenfalls als „Tage hängen am Beginn“. Sonst hinge die Frage davon ab,
welche Felder zuletzt angeklickt wurden.

**Die zwei Antworten**, berechnet vom Kern auf der Grundlage von series_shift:

- **„wie sie jetzt fallen“:** Die Termine bleiben zu denselben Zeitpunkten; die
  Regel nennt danach die Tage, auf die diese Zeitpunkte auf der neuen Uhr fallen.
  Der Beginn bleibt, jede Ausnahme, die einen Termin streicht, wandert mit, ein
  UTC-UNTIL ebenso. Das geschieht in **einem** Schritt von der alten zur neuen
  Uhr, nie über die Tage des Geräts.
- **„wie in der Regel“:** Der Regeltext bleibt. Der Beginn wandert nur dann auf
  den passenden Tag, wenn die Tage der Regel (Wochentage, Monatstag, Monat)
  **nur** zum Tag des Beginns auf der alten Uhr passen. Ausnahmen, die einen
  Termin streichen, wandern mit, ein UTC-UNTIL ebenso.
- **In beiden Fällen:** Eine Ausnahme, die auf der alten Uhr keinen Termin
  streicht, bleibt wörtlich stehen. So wirkt bei der Reparatur auch eine Löschung
  von vor dem Zonenverlust wieder: Der damals gelöschte Termin ist wieder
  gelöscht. Landet ein verschobener Beginn oder eine Ausnahme in der
  Frühjahrs-Lücke der neuen Uhr, wird diese Antwort mit Grund abgelehnt.

**Ein Beispiel (im Winter, Berlin = UTC+1).** Eine Serie hat ihre Berliner Zone
verloren: Regel „montags“, Beginn Sonntag 23:30 UTC. Heute wiederholt sie sich
also Montag 23:30 UTC, in Berlin dienstags. Wer Berlin wählt und „wie in der
Regel“ antwortet, bekommt den Beginn Montag 00:30 Berlin mit der Regel „montags“:
repariert. Eine Serie, die wirklich UTC meint (Regel „sonntags“, Beginn Sonntag
23:30 UTC), bekommt mit „wie in der Regel“ den Beginn Sonntag 00:30 Berlin, mit
„wie sie jetzt fallen“ Montag 00:30 Berlin mit der Regel „montags“.

**Wortlaut.**
- Titel: „An welchen Tagen soll sich die Serie wiederholen?“
- Nachricht: „In {neue Uhr} beginnt die Serie am {Beginn neu}, in {alte Uhr} am
  {Beginn alt}. Ihre Wiederholungsregel nennt {Tage}. {Passung}“. {Passung} ist
  eines von: „Das passt zum Start in {Uhr}.“, „Das passt zum Start in beiden.“,
  „Das passt zum Start in keiner der beiden.“, „Aperio kann nicht erkennen, zu
  welcher Uhr sie passt.“
- Knöpfe: „Wiederholen am: Montag (wie in der Regel)“, „Wiederholen am: Dienstag
  (wie sie jetzt fallen)“ und „{alte Zone} behalten“, etwa „UTC behalten“. Bei
  mehreren Tagen „Wiederholen an: Montag und Mittwoch (…)“, bei einem Monatstag
  „Wiederholen am 15. des Monats (…)“.
- Englisch entsprechend: „On which days should the series repeat?“, „Repeat on:
  Monday (as the rule says)“, „Repeat on: Tuesday (where they fall now)“, „Keep
  UTC“.

**Ablehnungen (20a).** Lässt sich eine der beiden Antworten nicht umsetzen, fällt
ihr Knopf weg, und die Nachricht sagt: „Die Tage, wie sie jetzt fallen, lassen
sich nicht behalten: {Grund}.“ beziehungsweise „Die Tage, wie in der Regel,
lassen sich nicht behalten: {Grund}.“ Die Gründe sind dieselben wie beim
Verschieben ganzer Serien; dazu kommt ein eigener Grund, wenn ein Beginn oder
eine Ausnahme in die Frühjahrs-Lücke der neuen Uhr fiele. Nur wenn keine Antwort
möglich ist (etwa eine Regel mit festen Stunden bei neuer Uhrzeit), wird der
Wechsel selbst abgelehnt, und die Zone bleibt.

**Danach** geht der Fokus immer zurück auf den Zonen-Knopf. Bei „{alte Zone}
behalten“ oder Escape ändert sich nichts („Zeitzone der Serie nicht geändert:
UTC (ohne Sommerzeit)“). Bei einer Antwort wird die Zone gesetzt, die Felder
zeigen die neuen Werte, die Wochentag-Kästchen ändern sich, wenn die Regel
umgeschrieben wurde, und angesagt wird zusätzlich „Wiederholt sich am: Dienstag“.
Hat die Antwort den Beginn verschoben, heißt es statt „Die Serie beginnt zum
selben Zeitpunkt: …“: „Die Serie beginnt jetzt am {Beginn neu} auf dieser Uhr.“

Auf dem Desktop öffnet die Frage erst, wenn die Auswahl geschlossen ist und der
Fokus sicher auf dem Knopf liegt; die Nachricht wird bei jedem Knopf als
Beschreibung vorgelesen, der erste Fokus liegt auf „{alte Zone} behalten“. Auf
dem Handy öffnet sie unter iOS beim Schließen der Auswahl, unter Android einen
Moment später (im nächsten Frame), weil das Schließ-Ereignis dort fehlt; der erste
Fokus liegt auf dem Titel der Frage.

### Kopien in anderen Kalendern (23b)

Liegt die Serie in einer Termingruppe, bietet Aperio nach dem Speichern an, die
Zone auf die Kopien mitzunehmen. Diese Frage kommt nie vor dem Speichern.

Heute trägt die Gruppe nur Titel, Beginn, Ende, Ganztägig, Ort und Beschreibung.
Eine Zonenänderung, bei der der Beginn seinen Zeitpunkt behält, löst heute kein
Angebot aus. Wandert der Beginn mit „wie in der Regel“, bietet die heutige
Mitnahme dagegen an, Beginn und Ende auf die Kopien zu schreiben, ohne deren Zone
und Regel. Stufe 13 legt fest, wie die Zonen-Mitnahme das ersetzt.

Nimmt Toni das Angebot an, bekommt jede beschreibbare Kopie dieselbe Zone, mit
derselben Antwort wie in der Wochentag-Frage. Eine Kopie, deren Regel diese
Antwort ablehnt oder deren Kalender die Zone nicht speichern kann, wird nicht
geändert und mit Grund genannt. Offen und in Stufe 13 zu entscheiden:
- ob die Mitnahme eine eigene Frage zur Zone ist oder die bestehende
  Mitnahme-Frage um die Zone erweitert;
- was eine Kopie bekommt, die eine Antwort braucht, wenn für die Serie selbst
  keine Frage kam (Vorschlag: für diese Kopie einzeln fragen).

### Wiederholung, Ganztägig, Umfänge

**Wiederholung aus:** Der Zeitpunkt bleibt, die Felder gehen zurück auf die
Geräte-Uhr, Zusatz und Hinweis verschwinden. Einmal angesagt: „Keine
Wiederholung. Die Zeiten stehen jetzt auf der Uhr dieses Geräts: …“ Gibt es die
aktuellen Uhrzeiten auf der Uhr der Serie nicht, bleiben sie wie getippt.

**Wiederholung wieder an** in derselben Sitzung: Die frühere Zone kommt zurück
(bei einer gespeicherten Serie die gespeicherte), der Zeitpunkt bleibt,
angesagt wie eine Wahl.

**Ganztägig an:** Die Daten bleiben, Auswahl und Hinweis verschwinden, die Zone
wird nicht gespeichert, wenn Ganztägig in dieser Sitzung eingeschaltet wurde. Eine
ganztägige Serie, unberührt gespeichert, behält ihren Wert wörtlich.

**Ganztägig aus:** Die Uhrzeiten kommen auf der gemerkten Uhr zurück. Eine
gespeicherte ganztägige Serie ohne solche Uhr bekommt die Gerätezone, und die
Uhrzeiten gelten wie getippt.

**Nur dieser Termin:** Geräte-Uhr, keine Zone. **Diesen und alle folgenden:** Uhr
der Serie (21a), der Rest behält die Zone der ganzen Serie.

**Titelvorschläge** füllen die Regel, lassen die Zone aber stehen.

### Prüfen und Speichern

- **Lücke:** Gibt es einen getippten Beginn oder ein getipptes Ende auf der Uhr
  der Felder nicht, lehnt Aperio das beim Speichern ab, sichtbar, sobald die
  Uhrzeit feststeht: „{Zeit} gibt es am {Datum} in {Uhr} nicht: Die Uhren werden
  vorgestellt. Wähle eine andere Uhrzeit.“ Einzeltermine behalten das heutige
  Verhalten.
- **Doppelte Stunde im Herbst:** Die frühere der beiden Möglichkeiten gilt.
- **Unberührte Werte** speichern die gespeicherten Zeitpunkte unverändert.
- **Eine Speicherfunktion** ersetzt `editedRecurrence` an jeder Stelle. Heute
  würde eine Wahl nie gespeichert, weil `editedRecurrence` die alte Zone behält.
  Unberührt heißt: der gespeicherte Wert wörtlich; gewählt: die kanonische Zone;
  UTC: keine Zone.
- **Anlegen aus dem Editor** stempelt nie die Gerätezone über eine gewählte UTC.
- **Schreibweise der Gerätezone:** Bis Stufe 10 bekommt eine neue Serie die
  Gerätezone so, wie das Gerät sie schreibt, etwa `Asia/Calcutta` (26a). Stufe 10
  entscheidet, ob die Vorbelegung kanonisch gespeichert wird.
- **Kopieren und Duplizieren** behalten die Zonen-Entscheidung der Quelle: Eine
  Serie auf „UTC (ohne Sommerzeit)“ bleibt als Kopie auf UTC, statt wie heute die
  Gerätezone zu bekommen. Das Verschieben einer ganzen Serie ändert die Zone
  ohnehin nicht.
- **UNTIL:** Ein unberührtes Ende bleibt roh. Ein geändertes Enddatum wird als
  23:59:59 dieses Datums auf der Uhr der Serie gespeichert, als UTC-Zeitpunkt. Ein
  UTC-UNTIL wandert nur über den Zonenwechsel. WKST bleibt erhalten.

### UTC und Anbieter

- **Was als UTC gilt:**
  - keine oder eine leere Zone;
  - die 18 Namen, die tzdata UTC gibt: `Etc/UTC` mit seinen sieben Verweisen wie
    `UTC`, `Zulu` oder `Etc/Universal`, und `Etc/GMT` mit seinen neun Verweisen
    wie `GMT`, `GMT0` oder `Greenwich`;
  - jeder Name, den tzdata nicht kennt: ein Windows-Name, ein Versatz wie
    `+05:30` oder eine Zone, die neuer ist als Aperios tzdata.

  Groß- und Kleinschreibung zählt nicht, Leerzeichen werden nicht entfernt. Das
  ist eine einzige Kern-Regel (`cal_core::series_clock`). Ansichten, Erinnerungen
  und die Gerätezone benutzen sie (26a). `Etc/GMT±N` bleibt eine benannte Zone
  und steht als „Aktuell: Etc/GMT+8 (nicht in Aperios Liste)“ da.
- **Zusammengelegte Orte (25b):** tzdata führt 139 Orte als Verweis auf eine
  andere Stadt, deren Uhr seit 1970 gleich läuft: 106, die früher eine eigene
  Zone mit Länderzeile hatten, und 33 weitere (etwa Montreal auf Toronto).
  Beispiele: Oslo, Stockholm und
  Kopenhagen verweisen auf Berlin, Amsterdam auf Brüssel, Reykjavik auf Abidjan.
  Die Liste bleibt bei 312 Zonen. Die Suche findet einen solchen Ort als „Berlin
  (auch Oslo)“, und gespeichert wird das Ziel.
- **Was die Anbieter für „keine Zone“ bekommen**, bleibt wie heute: Google
  `Etc/UTC`, Microsoft 365 `UTC`, CalDAV `Z`, Exchange keine Zone. Bis Stufe 4
  schrieb Exchange für fünf der 18 UTC-Namen (`UTC`, `Etc/UTC`, `Etc/GMT`,
  `Etc/Universal`, `Etc/Zulu`) `Id="UTC"`. Ab Stufe 4 schreiben alle UTC-Namen
  keine Zone (26a); ein Anlegen ohne Zone ist laut Microsoft-Dokumentation UTC.
- **Exchange:** Erst messen, was ein Update ohne StartTimeZone mit einer Serie
  macht, die eine Zone hat. Lässt sich das Feld entfernen, entfernt ein Wechsel
  auf UTC es. Sonst steht „UTC (ohne Sommerzeit)“ in einer Exchange-Serie mit
  Zone markiert in der Liste: „UTC (ohne Sommerzeit) (Exchange kann die Zone
  dieser Serie nicht entfernen)“, und lässt sich nicht wählen.
- **Zonen, die Exchange nicht speichern kann,** sind nach 22a markiert und
  nicht wählbar. Seit Stufe 4 sind es vier:
  - Antarctica/Troll, für das CLDR keinen Windows-Namen hat;
  - America/Scoresbysund, Antarctica/Casey und Antarctica/Vostok, deren
    Windows-Name in CLDR eine andere Uhr hat. Exchange würde dort eine andere
    Zone speichern als die gewählte.

  Die erzeugte Tabelle nennt sie, und ein Test hält die Liste fest. Eine neue
  Serie mit so einer Gerätezone beginnt auf UTC; nach der Endzeit steht dann
  „Exchange kann die Zeitzone {Stadt} nicht speichern; die Serie steht auf
  UTC.“ Kopieren oder Verschieben nach Exchange wird mit der Zone im Grund
  abgelehnt.
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
7. Suchen in der Liste: Städte unter ihrem englischen Namen; der Versatz ist der
   am Beginn der Serie.
8. Warnung nach 16b: Beim Anbieter geänderte Einzeltermine bleiben an ihrer
   alten Zeit hängen; Verweis auf den Abschnitt „Einzeln geänderte oder gelöschte
   Termine“.
9. Kopien in anderen Kalendern (23b).
10. Grenzen: Microsoft-365-Serien, Handy-Kalender, Exchange, Anzeigen auf der
    Geräte-Uhr, Sync-Konflikte.

In „Fehlersuche & Protokolle“: Eine Serie, die ihre Zone verloren hat, zeigt
UTC-Zeiten. Ganze Serie öffnen, Zone wählen, bei einer Serie nahe Mitternacht die
Wochentag-Frage beantworten.

## Stufenliste

Jede Stufe ist ein eigener PR. Wo die Oberfläche berührt wird, kommen Desktop und
Handy im selben PR.

1. **Handy-Tür zu series_shift** — `feat(mobile): a door into cal_core::series_shift`.
   cal-ffi-Export, Kotlin- und Swift-Funktion, `CalFfiModule.ts`, Installation
   in `mobile/index.ts`, Bindungen neu erzeugt. Schließt die Lücke aus #69.
2. **Zonennamen und eine UTC-Regel** — `feat(core): generated zone names and one UTC clock`.
   `cargo xtask tz-list [--check]` liest die tzdata-Dateien, die chrono-tz
   mitliefert. Er schreibt `crates/cal-core/src/series_clock/zone_names.rs`: alle
   597 Namen mit der Zone, auf die sie verweisen, und die 312 Zonen der Liste.
   Dabei prüft er an chrono-tz selbst, dass die Namen, die auf `Etc/UTC` oder
   `Etc/GMT` verweisen, genau die Zonen sind, deren Uhr von 1800 bis 2099 in
   jedem Monat UTC zeigt. Dass es 18 sind, prüft ein Test im Kern.

   Dazu gehören die Kern-Regeln `canonical_zone` und `series_clock_zone`, Türen
   in WebAssembly und cal-ffi und die Hülle `shared/seriesClock.ts`.
   `zoneOrNull`, `localTimeZone` und die Erinnerungen in host-core laufen darüber
   (26a). Die gemeinsame Fixture `seriesClock.json` lesen der Kern, die
   WebAssembly-Tür und die Handy-Tür.

   Gemessen:
   - Das WebAssembly-Modul wächst um 20,6 KB, gepackt um 6,5 KB (gzip) bzw.
     7,2 KB (brotli, so packt Tauri).
   - Ein Aufruf kostet in Node etwa 0,4 µs, einmal pro Serie beim Aufklappen.
   - WebView2 152 meldet `Asia/Calcutta`, `Europe/Kiev` und für jede
     UTC-Schreibweise `UTC`.
   - Hermes auf iOS und Android ist nur aus dem Quelltext gelesen. Das prüft der
     nächste Handy-Build.
3. **Die Weltliste mit Versatz und Suche** — `feat(core): the world zone list with offsets, and its search`.
   `cal_core::zone_list` baut auf der Namens-Tabelle aus Stufe 2 auf. Der
   Generator schreibt dafür zu jedem Namen seine Art, gelesen aus den
   Abschnitten von tzdatas `backward`.
   - Namen, Suche und die Einordnung einer gespeicherten Zone oder der Gerätezone
     brauchen keine Zeitzonen-Regeln. Sie stehen im normalen Kern, auf dem
     Desktop synchron über WebAssembly.
   - Nur die Versätze und die Reihenfolge brauchen chrono-tz. Sie stehen hinter
     dem Kern-Feature `zones`: auf dem Desktop ein Tauri-Befehl (`zone_offsets`),
     auf dem Handy eine synchrone cal-ffi-Tür. CI prüft, dass chrono-tz nicht in
     die WebAssembly-Tür gerät.

   Die Fixture `timeZoneFilter.json` lesen Kern, Handy-Tür und WebAssembly-Tür
   (diese ohne die Zeilen mit Versatz).

   Gemessen:
   - chrono-tz im WebAssembly-Modul kostete etwa 879 KB, deshalb bleibt es dort
     aus. Stufe 3 ohne chrono-tz: +85,8 KB roh, +33,4 KB gzip, +18,7 KB brotli;
     davon die Unicode-Zerlegung der Faltung 21,5 KB roh, 1,5 KB brotli.
   - Die native Handy-Bibliothek enthält chrono-tz schon heute (über host-core
     und die Adapter). Ein Prototyp mit Feature, Faltung und Suche wuchs um
     etwa 21,6 KB (arm64 `.so`), gepackt um etwa 5,3 KB.
   - Ein einfacher Prototyp auf dem Desktop (x64, release) brauchte für die
     Liste mit Versätzen etwa 0,45 ms und für eine Suche etwa 1,2 ms. Er
     rechnete dabei die Versätze bei jeder Suche neu; der gebaute Kern trennt
     beides, die Suche bekommt die Versätze mitgegeben.
   - Noch nicht gemessen: der Zuwachs der iOS-App und was ein Aufruf der
     Versätze für 312 Zonen auf einem Handy kostet.
   - WebView2 152 und Edge 154 (tzdata 2025c) stimmen mit chrono-tz 2025b
     überein.
   - Alte Android-Versionen kennen einzelne Zonen nicht. Von den 597 Namen
     fehlen Android 7.0 elf, 7.1.2 sieben, 8 und 9 sechs und 10 ohne
     Aktualisierung fünf; alle sind gelistete Zonen. Das ist aus AOSP-Dateien
     abgeleitet, nicht auf Geräten gemessen.
   - Die Vorlese-Form hat Toni gewählt: Form F, „UTC−04:00“.
4. **Exchange-Zonentabelle aus CLDR** — `feat(ews): Windows zone table generated from CLDR windowsZones`.
   `cargo xtask windows-zones` erzeugt die Tabelle des EWS-Adapters aus der
   CLDR-Datei `windowsZones.xml`. Die Datei liegt mit Lizenz und Herkunft
   (Release, Datum, sha256) in `crates/adapter-ews/cldr/`, und CI prüft die
   Tabelle mit `--check`. Jeder Name läuft dabei durch `cal_core`, also durch
   dieselbe Regel, die der Adapter beim Schreiben fragt.
   - Lesen: Die Endzone `tzone://Microsoft/Utc` heißt keine Zone (43b). Sonst
     liest sich der Name der Startzone als die Zone seiner Standardzeile, in
     tzdatas kanonischer Schreibweise. `UTC` und unbekannte Namen heißen keine
     Zone.
   - Schreiben: erst die Kern-Regel (26a), dann der Windows-Name der Zone. Ein
     zusammengelegter Ort schreibt den Namen seines Ziels (25b). Vor dem ersten
     Speichern einer Serie mit Zone fragt der Adapter den Server einmal, welche
     Namen er kennt (41a). Einen Namen, den er nicht kennt, bekommt er nicht:
     Die Serie geht ohne Zone raus, und das Protokoll nennt sie. Kann der
     Server nicht gefragt werden, gehen die CLDR-Namen raus, und das nächste
     Speichern fragt erneut.
   - Eine ganztägige Serie schreibt nie eine Zone (46a). Die Regel steht im Kern
     (`written_series_zone`), und Exchange, Microsoft 365, Google und CalDAV
     fragen sie. Microsoft 365 schreibt ganztägige Tage ohnehin als
     Mitternacht in UTC, Google und CalDAV als Datum; die Zone der Serie ging
     bei keinem der drei mit. Exchange fragt für eine ganztägige Serie auch nicht mehr, welche
     Zonen der Server kennt.
   - Die Termine im Speicher des Adapters kannten die Endzone nicht. Der
     Zustand eines Ordners trägt deshalb eine Leser-Version (`ITEM_PARSER`);
     ein Zustand von einem älteren Leser wird verworfen, und der Ordner wird
     einmal von vorn gelesen.
   - Ein Uhr-Wächter vergleicht jede Zone fünf Jahre ab der Veröffentlichung
     des Releases mit der Standardzone ihres Windows-Namens. Weicht die Uhr ab,
     wird die Zone nicht geschrieben (22a).
   - Der Sync-Token, den der Host speichert, trägt einen Fingerabdruck der
     Zonen-Übersetzung: die Zeilen der Tabelle und eine Nummer für die Leseregel.
     Nennt der Token eine andere Übersetzung, gibt der Adapter alle
     zwischengespeicherten Exchange-Termine einmal neu aus, ohne Exchange neu zu
     lesen. Sonst behielte die Ansicht eine Zone, die die alte Tabelle gelesen
     hat, und das nächste Bearbeiten schriebe sie unter dem neuen Namen an
     Exchange. Weil der Host einen Token nur zusammen mit seinen Terminen
     speichert, geht eine Neuausgabe nicht verloren, wenn eine Übertragung
     scheitert.

   Gemessen:
   - CLDR 48.2 (`release-48-2`, 17. März 2026) ist aktuell. `windowsZones.xml`
     ist von 48 bis 49-alpha2 byte-gleich und auf dem Stand von tzdata 2025b; sie
     kennt 139 Windows-Namen.
   - 311 der 312 gelisteten Zonen und 596 der 597 Namen haben in CLDR einen
     Windows-Namen, nur Antarctica/Troll nicht. 15 Zonen stehen dort nur unter
     einer alten Schreibweise, etwa Kolkata als Calcutta und Kyiv als Kiev.
   - Drei Zonen stehen unter einem Windows-Namen mit anderer Uhr: Scoresbysund,
     Casey und Vostok. Speicherbar sind also 308 gelistete Zonen, dazu die 26
     festen Versätze `Etc/GMT±N`. Der erste Lauf des Generators fand genau diese
     Zahlen.
   - Auf einem Windows-11-Rechner (Build 26220, TzVersion 7ea0002) ergeben die
     Windows-Regeln aller 139 Windows-Namen im Januar und im Juli 2026 dieselben
     Versätze wie ihre Standardzone. Darauf stützt sich der Uhr-Wächter.
     Exchanges eigene Tabellen sind nicht gemessen.
   - Die alte Handtabelle schrieb 138 gelistete Zonen. Drei Zeilen waren
     falsch: Chihuahua, Almaty und Beirut mit dem erfundenen Namen
     „Lebanon Standard Time“. 31 Windows-Namen las sie nicht, und sie
     kanonisierte nicht: `Asia/Calcutta` bekam keine Zone.
   - Beim Zurücklesen kommen von den 308 speicherbaren gelisteten Zonen 131 als
     sie selbst zurück und 177 als eine andere Zone auf derselben Uhr, etwa Wien
     als Berlin. 11 davon kommen als `Etc/GMT±N` zurück.
   - Microsoft dokumentiert: Ein Anlegen ohne Zone ist UTC. Eine Zeit mit `Z`
     zusammen mit StartTimeZone gilt als UTC. Nur die Zone zu ändern verschiebt
     die gespeicherten Zeitpunkte. Was ein Update ohne StartTimeZone macht, ist
     nicht dokumentiert.
   - EWS in Exchange Online wird ab Oktober 2026 abgeschaltet und ab April 2027
     ganz. Tonis Testserver ist ein eigener (39).
   - Live-Test an Tonis Exchange-Server (38a; Exchange 2019, Build 15.2.2562,
     15. September 2026). Die Anfragen erzeugt ein ignorierter Test aus Aperios
     Code:
     - Der Server kennt 140 Windows-Namen: alle aus CLDR außer
       „Sao Tome Standard Time“, dazu Kamchatka und Mid-Atlantic.
     - Ein erfundener Name bekommt `ErrorTimeZone`, und das ganze Anlegen
       scheitert. Daraus folgt 41a.
     - Berlin, Beirut, Wolgograd, Juba, Qyzylorda und Punta Arenas werden mit
       ihrer Zone angenommen, die Termine liegen vor und nach der Zeitumstellung
       richtig.
     - Ein Update ohne Zone lässt die Zone und die Zeitpunkte, wie sie sind.
     - Ein Update, das Beginn und Ende vor einer neuen Zone setzt, verschiebt
       die Zeitpunkte: Exchange behält die Uhrzeit und gibt ihr die neue Zone
       (08:00Z wurde zu 01:00Z). Ein Zonenwechsel muss die Zone deshalb vor
       Beginn und Ende oder getrennt senden (Stufen 9 und 12).
     - Eine Serie ohne Zone kommt mit der Startzone `Greenwich Standard Time`
       und der Endzone `tzone://Microsoft/Utc` zurück. Daraus folgt 43b.
     - Eine ganztägige Serie mit Zone legt Exchange auf die Tagesgrenzen dieser
       Zone und macht zwei Tage daraus. Outlook zeigt sie am Montag, aber über
       zwei Tage. Der „Dienstag“, der zuerst gemeldet wurde, war in Aperio
       abgelesen (siehe Runde 2). Das trifft heute schon jede ganztägige Serie
       in einer der 138 Zonen der alten Tabelle. Daraus folgt 42a.

   Live-Test Runde 2 (15. September 2026, derselbe Server). Gemeint war jeweils
   eine wöchentliche ganztägige Serie ab Montag, dem 19. Oktober 2026. Aperio
   schickt Beginn und Ende als UTC-Mitternacht. Die Wochentage wurden in Outlook
   und in Aperio abgelesen:
   - Ohne Zone (A10) und mit der Zone UTC (A11) speichert Exchange 00:00Z bis
     00:00Z am nächsten Tag. Outlook zeigt die Serie am Montag, einen Tag lang.
     Ohne Zone kommen wieder die Startzone Greenwich und die Endzone
     `tzone://Microsoft/Utc` zurück.
   - Eine Serie in Abidjan mit Uhrzeit (A12) behält die Endzone `Greenwich
     Standard Time`. Die Endzone trennt sie also von einer Serie ohne Zone
     (43b).
   - Reparatur einer Serie mit Zone Los Angeles durch ein Update auf UTC:
     - Kommt die Zone nach Beginn und Ende (B3), speichert Exchange drei Tage,
       und Outlook zeigt Montag bis Mittwoch.
     - Kommt die Zone zuerst (B4), ist die Serie danach sauber: Montag, ein Tag.
   - Aperio zeigt jede dieser Serien einen Tag zu spät, also am Dienstag. Das
     ist ein Lesefehler in Aperio, nicht in Exchange.
5. **Exchange und Microsoft 365 lesen Serien-Daten auf der Uhr der Serie** — `fix(ews, graph): read a series' dates on its own clock when writing`.
   Danach messen, wie Microsoft 365 recurrenceTimeZone beim Zurücklesen behandelt.
6. **Kalender melden, welche Zonen sie speichern** — `feat(plugin-core): calendars declare which series time zones they can store`.
   `time_zones`: alle, keine (Handy-Kalender) oder nur bestimmte (Exchange). Das
   Handy muss dafür auch die eingebauten Manifeste lesen; ob der Desktop sie
   genauso liest, wird vorher geprüft.
7. **Exchange sagt, was es nicht speichern kann** — `fix(ews): keep what Exchange can store, and say when it cannot`.
   Nach der Messung aus Stufe 4, und vor den Editoren, damit ein Wechsel auf UTC
   oder eine nicht speicherbare Zone nie still verpufft.
8. **Datum, Uhrzeit und Regel auf einer Serien-Uhr** — `feat(shared): dates, times and repeat rules on a series clock`.
   Umrechnung mit Lücke und Doppelstunde, Formularfelder mit Uhr, Regel-Ableitung
   aus dem Tages-Schlüssel, WKST, UNTIL. Noch unsichtbar. Fixture
   `wallClock.json`. Vorher messen: ob der Datums-Picker auf dem Handy eine feste
   Zone annimmt, und ob Node auf den CI-Linux-Rechnern IANA-TZ-Werte beachtet.
9. **Die Kern-Regel für den Zonenwechsel (15a)** — `feat(core): changing the clock a series repeats on`.
   `series_clock_change` mit Türen, erweitert `shared/seriesClock.ts` aus Stufe 2, Fixture
   `seriesClockChange.json`. Vorher prüfen, ob das Aufklappen mit rrule.js und die
   Passung im Kern für gezählte Wochentage und negative Monatstage übereinstimmen.
10. **Das Formular-Modell** — `feat(shared): the series clock form model`.
    Sichtbarkeit nach der Tabelle, Vorbelegung, Wechsel, Speicherfunktion,
    Rückfall auf UTC für Zonen, die das Gerät ablehnt, Kopieren und Duplizieren
    mit der Zonen-Entscheidung der Quelle.
11. **Die Auswahldialoge** — `feat(editors): the series time zone picker dialogs`.
    Desktop und Handy. Vorher messen: NVDA im verschachtelten Dialog, VoiceOver
    und TalkBack mit einer langen Liste.
12. **Die Editoren** — `feat(editors): times typed on the series clock, and the weekday question`.
    Beschriftungen, Hinweis, Auswahl, Wochentag-Frage, „diesen und alle folgenden“
    lädt auf dem Desktop die ganze Serie beim Öffnen (21a), Hilfe und Fehlersuche.
    Vorher messen: ob WebView2 bei jedem Pfeil im Wiederholungs-Feld ein
    Änderungsereignis schickt, Fokusfolge von Auswahl zu Frage, Ansage nach dem
    Schließen auf iOS.
13. **Kopien nehmen die Zone mit (23b)** — `feat(groups): carry a series' time zone to its copies`.
    Die Gruppen-Mitnahme im Kern trägt die Zone und ersetzt für einen
    Zonenwechsel die heutige Mitnahme von Beginn und Ende; das Angebot nach dem
    Speichern nennt die Zone; Kopien, die ablehnen, werden mit Grund genannt. Die
    offenen Punkte aus dem Abschnitt 23b werden hier entschieden.
14. **Optional: lesbare Sync-Konflikte bei reiner Zonenänderung** — `feat(sync): say what changed when only a series' zone differs`.

## Risiken

- **Zwei Zonen-Datenbanken.** Der Versatz kommt aus chrono-tz, Anzeige und
  Aufklappen aus Intl des Geräts. Für Zonen, die sich seit der chrono-tz-Version
  geändert haben, können Versatz im Eintrag und Uhrzeit in den Feldern
  auseinanderliegen.
  - V8 meldet alte Namen wie `Asia/Calcutta`.
  - Die tzdata in Node (2026b) ist neuer als die in chrono-tz (2025b).
  - Eine Zone, die neuer ist als Aperios tzdata, wiederholt sich nach UTC, bis
    chrono-tz aktualisiert ist.
- **Zeitzonendaten hinter der Wirklichkeit (32a).** chrono-tz 0.10.4 enthält
  tzdata 2025b, und eine neuere Version gibt es nicht. Neuere tzdata-Versionen
  ändern:
  - Casablanca und El Aaiun: ab 20. September 2026 dauerhaft UTC+00:00;
  - Vancouver, Edmonton und Inuvik: ab 1. November 2026 keine Rückstellung mehr;
  - Chisinau: die Umstellung eine Stunde später.

  Für diese Zonen zeigen die Liste und die Erinnerungen danach den alten
  Versatz, während ein Gerät mit aktuellen Daten richtig anzeigt. In der
  Fixture liegt kein Beginn auf diesen Tagen. Die Normalzeit und den Platz
  dieser Zonen in der Liste hält sie nur fest, wo beide Datenstände
  übereinstimmen, denn die Normalzeit liest das Jahr ab heute und reicht über
  diese Tage. Wie Aperio aktuelle Daten bekommt, ist eine eigene Aufgabe.
- **Erinnerungen folgen der Kern-Regel (26a).** Für zwei gespeicherte
  Schreibweisen ändern sie sich:
  - „europe/berlin“ erinnert jetzt nach Berlin statt nach UTC.
  - „ Europe/Berlin “ mit Leerzeichen erinnert nach UTC statt nach Berlin.

  Beides zeigen die Ansichten schon so.
- **Adapter lesen Zonennamen in exakter Schreibweise.** CalDAV und Microsoft 365
  geben den gespeicherten Namen direkt an chrono-tz, das Groß- und
  Kleinschreibung unterscheidet. Eine Serie mit „europe/berlin“ wiederholt sich
  in Ansichten und Erinnerungen nach Berlin. CalDAV liest und schreibt ihren
  Beginn aber als UTC-Zeit, und Microsoft 365 schreibt sie als UTC. Das war
  schon vor Stufe 2 so. Behoben ist es erst, wenn die Adapter die Kern-Regel
  fragen.
- **Versatz-Ansagen.** Toni hat die Form „UTC−04:00“ mit echtem Minuszeichen
  gewählt (Vorlese-Vergleich mit NVDA und VoiceOver, 15. September 2026). Wie
  313 ähnliche Einträge in einer langen Liste klingen, ist erst mit den Dialogen
  aus Stufe 11 zu hören. Die Form steckt in einem Übersetzungsschlüssel und lässt
  sich ohne Rust ändern.
- **Ausnahmen gegen die Regel im Formular.** Ob eine Ausnahme einen Termin
  streicht, prüft Aperio gegen Regel, Uhr und Ausnahmen, wie sie gerade im
  Formular stehen. Wurde die Regel in derselben Sitzung vorher geändert, kann eine
  Ausnahme, die zur gespeicherten Regel gehörte, als wirkungslos gelten, wörtlich
  stehen bleiben und nach dem Zonenwechsel ihren Termin verfehlen.
- **Exchange-Wechsel auf UTC.** Behält Exchange die alte Zone, wenn keine
  geschickt wird, und lässt sie sich nicht entfernen, muss dieser Wechsel
  abgelehnt werden.
- **Städtenamen aus Exchange.** Eine Wiener Serie kommt nach dem Aktualisieren
  als Berlin zurück (dieselbe Uhr). Gezählt in Stufe 4: Von den 308
  speicherbaren gelisteten Zonen kommen 177 als eine andere Zone auf derselben
  Uhr zurück. 11 davon kommen als fester Versatz `Etc/GMT±N`, der nicht in der
  Liste steht.
- **CLDR hinter tzdata.** CLDR ordnet manche Zonen einer Windows-Zone mit
  anderer Uhr zu, und der Uhr-Wächter nimmt sie aus der Tabelle. Ein neuer
  tzdata-Stand kann so auch verbreitete Zonen aus Exchange nehmen, bis CLDR und
  Windows folgen. Vancouver etwa hätte unter tzdata 2026b ab dem 1. November
  2026 eine andere Uhr als Pacific Standard Time. `cargo xtask windows-zones
  --check` nennt jede solche Zone mit „no longer written to Exchange“.
- **Feldreihenfolge im Update.** Aperio setzt beim Ändern Beginn und Ende vor
  der Zone. Ob Exchange die Zeitpunkte verschiebt, wenn im selben Update die
  Zone dazukommt, ist ungemessen. Der Live-Test (38a) prüft es.
- **EWS in Exchange Online endet** ab Oktober 2026, ganz im April 2027. Eigene
  Exchange-Server sind nicht betroffen.
- **Die Exchange-Tabelle hängt am Monorepo.** Verlässt der EWS-Adapter das
  Repository, müssen `cldr/`, die erzeugte Tabelle und
  `cargo xtask windows-zones` mitgehen. Sonst fällt die Prüfung still weg.
- **Beim Anbieter geänderte Einzeltermine** behalten nach einem Zonenwechsel
  ihren alten Zeitpunkt und können doppelt erscheinen oder Lücken lassen. Nur die
  Hilfe warnt (16b).
- **Lücken werden abgelehnt.** Serien auf der Gerätezone, die eine
  Frühjahrs-Uhrzeit bisher still gerundet haben, lassen sich so nicht mehr
  speichern.
- **Altes UNTIL im Osten.** Zonen-Serien östlich von UTC mit einem UNTIL als
  `{Datum}T235959Z` zeigen ihr Ende einen Tag später. Roh bleibt es, solange es
  unberührt ist; wer das angezeigte Datum neu tippt, verlängert die Serie um
  einen Tag.
- **Lange Listen auf dem Handy.** Virtualisierte Listen können Zeilen vor dem
  Wischen verbergen; alle 313 Zeilen zu zeichnen kann auf alten Android-Geräten
  langsam sein.
- **Große Editor-Stufe ohne Handy-Tests.** Stufe 12 berührt zwei große Editoren.
  Die Parität ruht darauf, dass die Logik in shared liegt und dort getestet ist.
- **Microsoft 365.** Eine dort mit Zone angelegte Serie lässt sich danach in
  Aperio weder zeigen noch korrigieren.
- **Kosten im Aufklappen.** Die Tür läuft in jedem Aufklappen, einmal pro Serie.
  Gemessen in Node:
  - etwa 0,4 µs pro Aufruf;
  - ein Monatsraster mit 55 Serien braucht dafür etwa 0,1 % seiner Zeit.

  Die Kosten auf dem Handy sind ungemessen.
- **Erzeugte Tabellen.** Die Generatoren hängen daran, dass chrono-tz seine
  Quelldaten mitliefert und die CLDR-Datei eingecheckt ist; ein Update lässt
  `--check` laut scheitern.

## Ungeprüft

- Wie NVDA (deutsche und englische Stimme) und VoiceOver „Startzeit (New York)“
  und ganze Einträge mit Kommas in einer langen Liste lesen. Die Versatz-Form
  selbst hat Toni verglichen (Form F).
- NVDA im verschachtelten Dialog mit Suchfeld und Liste; ob der Fokus-Rücksprung
  das Schließen der Auswahl und das Öffnen der Frage übersteht.
- Ob WebView2 bei jedem Pfeil im geschlossenen Wiederholungs-Feld ein
  Änderungsereignis schickt.
- iOS: ob eine Ansage nach dem Schließen eines Dialogs gehört wird. Android: ob
  das Öffnen einen Moment später mit TalkBack funktioniert. Ob Listenzeilen per
  Wischen erreichbar bleiben.
- Welche Zonen-Namen Intl auf Android 7 bis 10 und iOS 16.4 wirklich ablehnt.
  Für Android ist es aus AOSP-Dateien abgeleitet (Stufe 3), für iOS unbekannt.
- Welche Gerätezone iOS und Android für UTC, Indien und die Ukraine melden.
  Gemessen ist nur WebView2 152: `UTC`, `Asia/Calcutta` und `Europe/Kiev`, für
  eine emulierte Gerätezone `GMT` (ICU) `+00:00`.
- Ob der Datums-Picker auf dem Handy eine feste Zone annimmt.
- Ob sich die Zone einer Exchange-Serie entfernen lässt. Was ein Update ohne
  Zone, ein unbekannter Name und die Reihenfolge von Beginn, Ende und Zone
  bewirken, hat die erste Runde des Live-Tests an Exchange 2019 gemessen
  (Stufe 4). Offen bleiben:
  - welche Anzeigeregel Outlook für ganztägige Termine anwendet. Alle
    Beobachtungen stammen aus Berlin. Sie passen zur Regel „schwebend in der
    gespeicherten Zone“ (MS-OXOCAL 3.1.5.5.1), aber auch zu einem Abschneiden
    auf das Datum. Die dritte Runde prüft das mit einem Termin in der Zone
    Tokio (49a);
  - ob ein Update ohne Zone bei einem ganztägigen Termin aus Outlook die Zone
    behält und ihn verlängert, und ob Mitternacht in der gespeicherten Zone ihn
    auf seinem Tag lässt (47a, dritte Runde);
  - ob Zone zuerst auch bei einer Serie mit Uhrzeit die Zeitpunkte stehen
    lässt. Gemessen ist das nur an einer ganztägigen Serie (Stufen 9 und 12);
  - wie ganztägige Termine mit Teilnehmern angezeigt werden; für sie gilt die
    Regel „schwebend“ nicht;
  - welche Namen Kerio Connect und Zimbra kennen und was sie mit einem
    unbekannten Namen tun.

  Ungeprüft bleibt, ob Exchange eigene Zonendefinitionen und Namen, die nur in
  der Windows-Registry stehen (Kamchatka, Mid-Atlantic), so zurückgibt, wie
  Aperio sie liest: als keine Zone. Ungeprüft ist auch, ob Exchanges eigene
  Zeitzonen-Regeln denen von Windows gleichen, an denen der Uhr-Wächter über die
  Standardzonen gemessen ist.
- Ob der Desktop die eingebauten Manifeste für die Wiederholungs-Fähigkeiten
  genauso liest wie das Handy (src-tauri commands/calendars.rs).
- Wie Microsoft 365 recurrenceTimeZone nach Stufe 5 beim Zurücklesen behandelt.
- Ob Google und Microsoft 365 alle 312 kanonischen Namen annehmen und
  zurückgeben, besonders umbenannte (Europe/Kyiv, America/Ciudad_Juarez); ob iCloud
  TZIDs mit erzeugten VTIMEZONEs behält.
- Ob Google, Outlook und iCloud einen Beginn, der nicht zur Regel passt, als
  zusätzlichen ersten Termin zeigen.
- Ob das Aufklappen mit rrule.js und die Passung im Kern für gezählte Wochentage
  und negative Monatstage übereinstimmen.
- Abweichungen zwischen chrono-tz und den Intl-Daten der Handys. WebView2 152
  und Edge 154 stimmen überein; Node 24 (tzdata 2026b) weicht für Vancouver ab
  1. November 2026 und für Chisinau ab.
- Größenzuwachs der iOS-App und was der Aufruf der Versätze für 312 Zonen auf
  einem Handy kostet. Gemessen sind die Android-Bibliothek (mit einem Prototyp)
  und WebAssembly (Stufe 3).
- Ob Node auf den CI-Linux-Rechnern IANA-TZ-Werte beachtet.
  - Auf Windows kam `America/Los_Angeles` aus Git Bash nicht an, weil MSYS Werte
    mit `/` umschreibt.
  - Aus PowerShell beachtet Node sie.
