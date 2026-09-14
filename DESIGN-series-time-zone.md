# Zeitzone einer Serie — Entwurf

Status: **entschieden, noch nicht gebaut.** Toni hat die Form am 14. September
2026 festgelegt (Entscheidungen 13b, 14a, 15a, 16b, 17b, 18a, 20a, 21a, 22a, 23b
und 24a). Die Planung lief in zwei Runden: drei Varianten mit je einer
Gegenprüfung, dann zwei Planer (Bedienung, Unterbau) mit je einem Kritiker und
einer Zusammenführung. Danach wurde dieses Dokument selbst gegen die
Entscheidungen, den Code und die Planung geprüft. Der Code-Stand dahinter ist
main nach #69 und #70. Die Stufenliste am Ende sagt, was in welcher Reihenfolge
gebaut wird. Nichts davon lief bisher auf einem Gerät.

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
| neuer Termin | ja | aus | wählbar; Vorgabe Gerätezone, UTC, wenn das Gerät UTC meldet oder der Kalender die Gerätezone nicht speichern kann (22a) | Uhr der Serie |
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
UTC-04:00“.

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
läuft auf UTC) und der UTC-Eintrag. Danach folgen alle Einträge nach
Standard-Versatz, dann nach Stadt.

**Die Suche** findet Teile von Stadt, Zonen-Id, alten Namen, Region in der
Sprache der Oberfläche und Versatz („+2“, „+02:00“, „5:30“). Anfrage und Liste
werden auf beiden Seiten mit derselben festen Tabelle gefaltet: klein, ä/ö/ü zu
a/o/u, „ue/oe/ae“ zu u/o/a, ß zu ss, kombinierende Zeichen weg. So finden
„Zürich“ und „Zuerich“ beide Zurich. Wird ein Eintrag über einen alten Namen
gefunden, sagt er das: „Kyiv, Europa, UTC+03:00 (auch Kiev)“. Die Zählzeile sagt
„Eine Zeitzone“ oder „12 Zeitzonen“, bei keinem Treffer „Keine Zeitzone passt zu
‚{Suche}‘.“

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
- **Zusammengelegte Orte (25b):** tzdata führt 106 Orte als Verweis auf eine
  andere Stadt, deren Uhr seit 1970 gleich läuft. Beispiele: Oslo, Stockholm und
  Kopenhagen verweisen auf Berlin, Amsterdam auf Brüssel, Reykjavik auf Abidjan.
  Die Liste bleibt bei 312 Zonen. Die Suche findet einen solchen Ort als „Berlin
  (auch Oslo)“, und gespeichert wird das Ziel.
- **Was die Anbieter für „keine Zone“ bekommen**, bleibt wie heute: Google
  `Etc/UTC`, Microsoft 365 `UTC`, CalDAV `Z`, Exchange keine Zone.
- **Exchange:** Erst messen, was ein Update ohne StartTimeZone mit einer Serie
  macht, die eine Zone hat. Lässt sich das Feld entfernen, entfernt ein Wechsel
  auf UTC es. Sonst steht „UTC (ohne Sommerzeit)“ in einer Exchange-Serie mit
  Zone markiert in der Liste: „UTC (ohne Sommerzeit) (Exchange kann die Zone
  dieser Serie nicht entfernen)“, und lässt sich nicht wählen.
- **Zonen, die Aperios Windows-Tabelle nicht abbilden kann,** sind nach 22a
  markiert und nicht wählbar. Eine neue Serie mit so einer Gerätezone beginnt auf
  UTC; nach der Endzeit steht dann „Exchange kann die Zeitzone {Stadt} nicht
  speichern; die Serie steht auf UTC.“ Kopieren oder Verschieben nach Exchange
  wird mit der Zone im Grund abgelehnt.
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
   Dabei prüft er, dass genau die 18 UTC-Namen zu jeder Zeit UTC zeigen.

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
   Danach messen, wie Microsoft 365 recurrenceTimeZone beim Zurücklesen behandelt.
6. **Kalender melden, welche Zonen sie speichern** — `feat(plugin-core): calendars declare which series time zones they can store`.
   `time_zones`: alle, keine (Handy-Kalender) oder nur bestimmte (Exchange). Das
   Handy muss dafür auch die eingebauten Manifeste lesen; ob der Desktop sie
   genauso liest, wird vorher geprüft.
7. **Exchange sagt, was es nicht speichern kann** — `fix(ews): keep what Exchange can store, and say when it cannot`.
   Nach der Messung aus Stufe 4, und vor den Editoren, damit ein Wechsel auf UTC
   oder eine nicht abbildbare Zone nie still verpufft.
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
- **Erinnerungen folgen der Kern-Regel (26a).** Für zwei gespeicherte
  Schreibweisen ändern sie sich:
  - „europe/berlin“ erinnert jetzt nach Berlin statt nach UTC.
  - „ Europe/Berlin “ mit Leerzeichen erinnert nach UTC statt nach Berlin.

  Beides zeigen die Ansichten schon so.
- **Versatz-Ansagen ungetestet.** Liest ein Screenreader „UTC+02:00“ schlecht,
  sind 313 ähnliche Einträge schwer zu unterscheiden. Die Form steckt in einem
  Übersetzungsschlüssel und lässt sich ohne Rust ändern.
- **Ausnahmen gegen die Regel im Formular.** Ob eine Ausnahme einen Termin
  streicht, prüft Aperio gegen Regel, Uhr und Ausnahmen, wie sie gerade im
  Formular stehen. Wurde die Regel in derselben Sitzung vorher geändert, kann eine
  Ausnahme, die zur gespeicherten Regel gehörte, als wirkungslos gelten, wörtlich
  stehen bleiben und nach dem Zonenwechsel ihren Termin verfehlen.
- **Exchange-Wechsel auf UTC.** Behält Exchange die alte Zone, wenn keine
  geschickt wird, und lässt sie sich nicht entfernen, muss dieser Wechsel
  abgelehnt werden.
- **Städtenamen aus Exchange.** Eine Wiener Serie kommt nach dem Aktualisieren
  als Berlin zurück (dieselbe Uhr).
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

- Wie NVDA (deutsche und englische Stimme) und VoiceOver die Versatz-Texte,
  „Startzeit (New York)“ und die Einträge mit Kommas lesen.
- NVDA im verschachtelten Dialog mit Suchfeld und Liste; ob der Fokus-Rücksprung
  das Schließen der Auswahl und das Öffnen der Frage übersteht.
- Ob WebView2 bei jedem Pfeil im geschlossenen Wiederholungs-Feld ein
  Änderungsereignis schickt.
- iOS: ob eine Ansage nach dem Schließen eines Dialogs gehört wird. Android: ob
  das Öffnen einen Moment später mit TalkBack funktioniert. Ob Listenzeilen per
  Wischen erreichbar bleiben.
- Welche Zonen-Namen Intl auf Android 7 bis 9 und iOS 16.4 ablehnt.
- Welche Gerätezone iOS und Android für UTC, Indien und die Ukraine melden.
  Gemessen ist nur WebView2 152: `UTC`, `Asia/Calcutta` und `Europe/Kiev`, für
  ein GMT-Gerät `+00:00`.
- Ob der Datums-Picker auf dem Handy eine feste Zone annimmt.
- Was Exchange bei einem Update ohne StartTimeZone macht und ob sich das Feld
  entfernen lässt; wie viele Zonen nach der CLDR-Tabelle abbildbar sind.
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
- Abweichungen zwischen chrono-tz und den Intl-Daten der Geräte.
- Größenzuwachs der nativen Bibliothek und Aufrufkosten auf dem Handy. Für
  WebAssembly sind beide gemessen.
- Ob Node auf den CI-Linux-Rechnern IANA-TZ-Werte beachtet.
  - Auf Windows kam `America/Los_Angeles` aus Git Bash nicht an, weil MSYS Werte
    mit `/` umschreibt.
  - Aus PowerShell beachtet Node sie.
