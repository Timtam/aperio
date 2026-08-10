# Termingruppen — Entwurf

Status: **Stufen 0 bis 4 gebaut**, auf beiden Plattformen und im Widget. Dieses
Dokument hat entschieden, ob und in welcher Form gebaut wird; die Stufenliste am
Ende sagt, was davon steht. Nichts davon lief bisher auf einem Gerät.

## Der Anlass

Ein und derselbe Termin liegt heute mehrfach in Aperio, und Aperio weiß nichts
davon. Der Leitfall stammt aus dem echten Gebrauch:

Ein Arbeitstermin liegt in Outlook, weil die Kollegen ihn dort sehen müssen.
Derselbe Termin liegt noch einmal im Privatkalender, weil dieser an Alexa hängt
und nur so eine gesprochene Erinnerung kommt. Dieselbe Besprechung liegt drittens
in den Kalendern der Kollegen, die Aperio ebenfalls liest.

Für Aperio sind das drei bis fünf **unabhängige** Termine, die zufällig gleich
aussehen. Daraus folgen drei Ärgernisse, und alle drei sind täglich spürbar:

Die Tagesansicht zeigt dieselbe Sache mehrfach. Für einen Screenreader-Nutzer
ist das nicht bloß unschön, sondern kostet jedes Mal Zeit beim Durchgehen.

Eine Verschiebung muss von Hand mehrfach nachgezogen werden. Wer eine Kopie
vergisst, hat ab dann Kalender, die einander widersprechen — und merkt es erst,
wenn jemand zum falschen Zeitpunkt erscheint.

Ein Meeting-Link hängt an genau einem dieser Termine. An welchem, ist eine
Zufallsentscheidung des Moments, in dem verknüpft wurde.

## Was eine Gruppe ist

Eine Gruppe ist eine Aussage von Aperio über fremde Daten: **diese Termine
meinen dieselbe Verabredung.** Sie ist kein Termin, sie ersetzt keinen, und kein
Anbieter erfährt von ihr.

Sie lebt in Aperios eigenem Datensatz und synchronisiert wie alles andere über
die Geräte. Das ist zugleich ihre Stärke und ihre Schwäche: sie kann etwas
ausdrücken, das kein Kalenderprotokoll kennt — und sie hat keinen Halt außerhalb
von uns.

## Was es löst

**Eine Zeile statt vier.** Die Gruppe wird als EIN Eintrag angezeigt, der nennt,
über welche Kalender sie reicht. Das ist der größte Alltagsgewinn und
wahrscheinlich allein den Aufwand wert.

**Ein Bearbeiten statt vier.** Beim Ändern wird nach dem Umfang gefragt — dieses
Vorkommen oder alle Mitglieder, für die Schreibrechte bestehen. Dieselbe Frage,
die bei Serien schon beantwortet wird, mit derselben Mechanik.

**Das Meeting hängt an der Gruppe.** Damit entfällt die heutige Ja/Nein-Rückfrage
beim Verknüpfen ersatzlos: es gibt nichts mehr zu verknüpfen, weil die
Zusammengehörigkeit das Modell selbst ist.

## Was es NICHT löst

Die Dublette bleibt bestehen. Alexa liest den Privatkalender, nicht Aperio — die
Kopie dort ist der Zweck der Übung und muss weiter existieren. Gruppen machen die
Duplizierung nicht überflüssig, sondern **verwaltbar**: aus vier Dingen, die man
gleich halten muss, wird eines, das man ändert.

Das ist ehrlich zu sagen, bevor jemand mehr erwartet.

## Die schwierigen Teile

### Woher die Mitgliedschaft kommt

Drei Möglichkeiten, und die Wahl entscheidet über den Charakter des Features.

**Vom Nutzer gestiftet.** „Diese beiden gehören zusammen." Immer richtig, kostet
eine Geste je Gruppe.

**Automatisch erkannt** an Titel und überlappendem Zeitfenster. Billig, und in
einem Büro voller „Team-Meeting" um 10:00 regelmäßig falsch — zwei verschiedene
Besprechungen mit gleichem Namen wären dann eine.

**Erkannt und VORGESCHLAGEN, einmal bestätigt, dauerhaft gemerkt.** Das ist die
Form, die ich empfehle. Sie hat die Trefferquote der Erkennung und die
Verlässlichkeit der Aussage, und sie ist genau die Interaktion, die es beim
Meeting-Verknüpfen heute schon gibt — nur dass die Antwort diesmal aufbewahrt
wird, statt jedes Mal neu erfragt zu werden.

### Wie eine Gruppe überlebt

Termin-Kennungen gehören dem Anbieter. Sie ändern sich bei einem
Neu-Bootstrap, beim Verschieben zwischen Kalendern, und bei Exchange auch
ungefragt. Eine Gruppe, die nur Kennungen speichert, verliert Mitglieder still.

Ein Mitglied braucht daher neben der Kennung eine **Signatur** — Titel und
Startzeitpunkt —, mit der es sich wiederfinden lässt, wenn die Kennung nicht mehr
auflöst. Selbstheilend statt baumelnd.

**Gebaut** (`shared/healEventGroups.ts` + `EventGroupsRepo::heal_member`): Die
Ansicht, die einen Bereich in der Hand hat, kann „das Mitglied liegt woanders"
von „diese Kennung löst hier nichts auf" unterscheiden — nur Mitglieder, deren
gespeicherter Beginn IN den Bereich fällt, werden überhaupt betrachtet. Gesucht
wird mit derselben strengen Regel wie bei der Erkennung: gleicher Kalender,
gleicher Titel, gleicher Beginn. Ein Beinahe-Treffer würde die Gruppe still auf
den falschen Termin umbiegen, und das ist schlimmer als eine Gruppe, der ein
Mitglied fehlt und die das sagt. Wird nichts gefunden, bleibt die Mitgliedschaft
unangetastet — es könnte eine Kopie sein, die der Nutzer gelöscht hat, und sie
auf Verdacht fallen zu lassen ändert etwas, worum niemand gebeten hat.

Die Reparatur läuft **still**: dieselben Termine meinen davor wie danach
dieselbe Verabredung, es gibt dem Nutzer nichts zu melden. Sie läuft auf allen
sechs Oberflächen — jede hat den Bereich, der den Beweis liefert.

Läuft eine Gruppe auseinander — ein Anbieter verschiebt ein Mitglied, die
anderen nicht —, ist sie eine Behauptung, die nicht mehr stimmt. Sie wird dann
NICHT gefaltet: jede Kopie bleibt sichtbar, sagt es in ihrer Beschriftung und
trägt die hervorgehobene Marke „3× ≠". Verschweigen wäre das Einzige, was hier
nicht geht.

### Rechte

Ein Kollegenkalender ist meist nur lesbar. „Alle mitziehen" muss deshalb sagen,
welche Mitglieder es NICHT konnte, statt sie stillschweigend zu überspringen —
sonst entsteht wieder genau der Widerspruch, den die Gruppe verhindern sollte.

## Vorgeschlagene Stufen

Jede Stufe ist für sich nützlich und einzeln testbar.

**Stufe 0** — Modell, Speicherung, Synchronisation. Eine Aktion „gehören
zusammen" auf zwei ausgewählten Terminen. Keine weitere Oberfläche. **GEBAUT.**
Statt einer Mehrfachauswahl — einer Zeigegeste ohne brauchbares
Tastatur-Äquivalent — trägt der Dialog den Termin, aus dessen Menü er kam, und
lässt den zweiten aus den übrigen Terminen desselben Tages BENENNEN. Dort liegt
eine Dublette per Definition.

**Stufe 1** — Zusammenfalten in den Ansichten. Eine Zeile, die ihre Kalender
nennt. Der Alltagsgewinn. **GEBAUT** auf allen sechs Oberflächen (Desktop Tag,
Woche, Monat, Agenda; mobil Tagesliste und Agenda). Gefaltet wird **pro Tag** —
ein wiederkehrender Termin rendert eine Zeile pro Tag, über eine Woche gelesen
sähen die eigenen Tage der Serie aus wie widersprüchliche Kopien. Und eine
Gruppe, deren Kopien auseinandergelaufen sind, wird NICHT gefaltet: dann stimmt
die Behauptung nicht mehr, und genau das ist das Einzige, was man sehen muss.

Gefaltet wird auch SICHTBAR: Die Zeile trägt eine Marke („3×", bei
auseinandergelaufenen Kopien „3× ≠" in der Warnfarbe, `groupBadge`). Ohne sie
war das Falten nur hörbar — die Sprachausgabe sagte „ein Termin mit 2
weiteren", der Bildschirm zeigte eine gewöhnliche Zeile.

**Stufe 2** — Bearbeitungsumfang über die Mitglieder, mit ehrlicher Meldung über
das, was nicht ging. **GEBAUT**, mit einer Abweichung vom Entwurf: gefragt wird
NACH dem Speichern, nicht davor. Die eigene Änderung des Nutzers steht damit nie
auf dem Spiel, die Frage kann nur Arbeit hinzufügen, und Abbrechen kostet nichts
— anders als ein Umfang, der in „Speichern" eingebaut ist. Mitgezogen wird nur,
was der Termin IST (Titel, wann, wo, Beschreibung); Erinnerungen, Farbe, Kalender
und Teilnehmer gehören der Kopie. Nur-lese-Kalender werden VOR der Entscheidung
genannt, nicht still übersprungen.

„**Nur dieses Vorkommen**" zieht ebenfalls mit, und zwar richtig: Was die
Bearbeitung mit dem Ausgangstermin gemacht hat — Vorkommen per EXDATE aus der
Serie schneiden und einen Einzeltermin an seine Stelle setzen —, geschieht mit
jeder Kopie. Ein Aktualisieren der Kopie-Zeile hätte JEDES Vorkommen dieser
Kopie verschoben, weil eines bearbeitet wurde; genau das verhindert die
Umfangs-Rückfrage ja. Die erzeugte Zeile ist dabei die Kopie SELBST — ihre
Erinnerung, ihre Farbe, ihr Kalender —, nur mit den geänderten Feldern
überschrieben; Beginn und Ende kommen aus dem eigenen Vorkommen der Kopie,
außer die Bearbeitung hat den Termin verschoben.

„**Dieses und alle folgenden**" zieht ebenfalls mit. Zuerst wurde die heikle
Rechnung — COUNT-Arithmetik, EXDATE-Übergabe an den Rest, die Zone des
Fortsetzungs-Teils, Benachrichtigung auf BEIDEN Schreibvorgängen und das
Zurücknehmen des Kopfes, wenn der Rest nicht angelegt werden konnte — aus den
beiden Editoren in `shared/seriesSplit.ts` gezogen (`planSeriesSplit` rechnet,
`writeSeriesSplit` schreibt in der richtigen Reihenfolge und heilt). Sie je
Mitglied zu wiederholen hätte sie sonst an vier Stellen stehen lassen.

Geteilt wird jede Kopie an IHREM eigenen nächsten Vorkommen ab dem Schnitt —
meist ist das der Schnitt selbst; eine anders getaktete Kopie (zweiwöchentlich
gegen wöchentlich) hat dort keines und wird an ihrem nächsten geteilt. Eine
Kopie, die ab dem Schnitt gar keines mehr hat, wird GENANNT statt still
übergangen.

**Die neuen Zeilen werden wieder verknüpft.** Sowohl „nur dieses Vorkommen" als
auch „dieses und alle folgenden" hinterlassen eine NEUE Zeile je Kalender —
außerhalb der Gruppe. Ohne einen zweiten Schritt hätte das Mitziehen die
Gruppierung ab dieser Stelle also aufgehoben: Was gerade eine Zeile geworden
war, wäre wieder vier. Am Ende des Mitziehens werden die neuen Zeilen darum zu
einer eigenen Gruppe zusammengefasst (die alte behält die Köpfe). Scheitert
das, sagt der Dialog genau das — geschrieben ist trotzdem alles.

**Stufe 3** — Erkennung und Vorschlag; das Meeting wandert an die Gruppe, die
heutige Verknüpfungs-Rückfrage entfällt.

Die **Erkennung** ist gebaut (`shared/suggestGroupMate.ts`): gleicher Name,
gleicher Beginn (bei ganztägigen derselbe Tag), anderer Kalender — alle drei
Bedingungen nötig. „Überlappend" hätte den Termin DAVOR angeboten, und ein
Beinahe-Treffer beim Titel ist weit öfter etwas anderes als dieselbe Sache. Der
Fund kommt als **Vorauswahl** in die Auswahlliste, mit einer Zeile, die sagt
warum; angewendet wird nichts ohne Bestätigung.

Das **Meeting an der Gruppe** ist gebaut, als Lesen statt als Umzug
(`MeetingsRepo::get_including_copies`): die Bindung liegt weiter an genau einem
Termin, aber gesucht wird über alle Kopien. Damit hört es auf, ein Zufall des
Moments zu sein, an welcher Kopie verknüpft wurde — „Beitreten" erscheint an
jeder. Eine eigene Zeile an der Gruppe hätte eine weitere Tabelle und eine
Migration gekostet, für etwas, das die Abfrage beantwortet, und hätte entscheiden
müssen, was beim Auflösen der Gruppe mit dem Meeting geschieht — danach hat
niemand gefragt.

Das **Vorschlagen von sich aus** ist gebaut: EINE Zeile über dem Tag, nur wenn
es etwas zu fragen gibt, mit zwei endgültigen Antworten — „Ja, eine Verabredung"
gruppiert, „Nein, nicht dasselbe" wird gemerkt (Migration 0037) und das Paar nie
wieder angeboten, auf keinem Gerät, weil der Vermerk synchronisiert.

Die Größe ist der Punkt: Ein Angebot, das man nicht endgültig loswird, ist eine
tägliche Störung — für einen Screenreader-Nutzer jeden Morgen eine Zeile mehr,
bevor der eigentliche Tag beginnt. Kann die Ablehnungsliste nicht gelesen
werden, bleibt die Zeile **stumm**: ein bereits abgelehntes Paar erneut
anzubieten ist der eine Fehler, den diese Funktion nicht machen darf.

Angeboten wird nur auf den EINTÄGIGEN Oberflächen. In Woche und Monat würde die
Frage einen Tag betreffen, den der Nutzer gerade nicht liest.

**Stufe 4** — Die Meeting-Zeilen automatisch mit dem Termin gruppieren, zu dem
sie gehören. **GEBAUT**, auf beiden Plattformen.

### Das Problem, das vorher versteckt wurde

Ein Videokonferenz-Konto liefert einen eigenen, nur lesbaren Kalender seiner
Meetings. Die meisten dieser Meetings haben auch einen Kalendereintrag — Aperios
eigenen oder die Einladung, die Outlook geschrieben hat —, also erschienen sie
doppelt. `withoutDuplicateMeetings` warf darum in der Ansicht die Meeting-Zeile
weg, sobald ein echter Termin dieselbe **Beitritts-URL** trug.

Das Ergebnis stimmt meistens, aber der Weg dahin ist derselbe, den dieses ganze
Dokument sonst ablehnt: **Aperio wirft Daten weg, um eine Doppelung zu
verbergen.** Greift die Regel daneben, verschwindet ein Meeting, das es wirklich
gibt, und nichts sagt es.

Der schlimmere Fall ist aber nicht das Danebengreifen, sondern der Treffer:
Wird ein Termin verschoben und sein Meeting nicht, **passt die Beitritts-URL
weiterhin** — der Filter versteckt das Meeting also genau dann, wenn die beiden
aufgehört haben, sich einig zu sein. Das ist die eine Information, die man
wirklich braucht, und sie war die einzige, die garantiert verschwand.

Eine Gruppe erzeugt dasselbe Ergebnis auf ehrliche Weise: **beide Zeilen
bleiben**, gefaltet wird eine, die Marke sagt „2×", „Beitreten" liest ohnehin
schon über alle Kopien (`MeetingsRepo::get_including_copies`), und laufen die
beiden auseinander, faltet die Gruppe nicht mehr — beide Zeilen erscheinen, mit
„≠" markiert.

### Warum hier automatisch gruppiert werden DARF

Stufe 3 lehnt automatisches Gruppieren ausdrücklich ab, und die Begründung
bleibt richtig: Ein Büro voller „Team-Meeting" um 10:00 würde zu Gruppen
führen, um die niemand gebeten hat. Diese Begründung zielt aber auf das
**Raten** — auf Namensgleichheit als Indiz.

Ein Meeting und sein Kalendereintrag sind nicht über den Namen verwandt,
sondern über eine **vom Anbieter ausgestellte Kennung**: die Beitritts-URL, die
im Termin steht. `withoutDuplicateMeetings` benutzt bereits genau die, und der
Kommentar dort sagt auch, warum sie und nichts anderes: Aperio schreibt den
Titel des Termins in das Meeting, das es anlegt — Titelgleichheit trägt hier
also fast keine Information —, und die Zeiten laufen genau dann auseinander,
wenn ein Termin verschoben wird, also genau dann, wenn man die beiden am
dringendsten als eines sehen will.

Automatisch gruppieren auf einer **Identität** ist damit etwas grundsätzlich
anderes als automatisch gruppieren auf einer **Ähnlichkeit**. Das erste ist
vertretbar, das zweite bleibt es nicht.

### Wie es gebaut ist

**Aufgelöst heißt aufgelöst.** Der Mechanismus dafür existierte schon: die
Ablehnungs-Marken aus Migration 0037, die ein Paar dauerhaft und
geräteübergreifend von Vorschlägen ausnehmen. Wer ein Mitglied aus einer Gruppe
nimmt oder eine Gruppe auflöst, schreibt jetzt genau diese Marke — für jedes
betroffene Paar, und sie **wandert über den Log mit** (`emit_declines`). Ohne
das Mitwandern hätte das andere Gerät nur „die Gruppe ist weg" gehört und sie
aus derselben Beitritts-URL sofort neu gebildet: die Gruppe käme jeden Tag
zurück, und die Funktion wäre eine Zumutung statt einer Hilfe.

Das bindet die Vorschlagszeile mit, und das ist richtig statt Nebenwirkung:
„Ich habe das herausgenommen" ist dieselbe Aussage wie „nein, nicht dasselbe".

**Eine Marke bricht die Gruppe NICHT.** Sie tut, was Migration 0037 immer
gesagt hat: Sie bringt das *Angebot* zum Schweigen — die Vorschlagszeile und die
automatische Meeting-Verknüpfung. Eine bestehende Gruppe fasst sie nicht an.

Der Versuch, das zu ändern, ist zweimal gescheitert, und beide Male hat es eine
adversarische Review gefunden. Beim Anwenden eines Datensatzes abzubrechen hing
an Ankunftsreihenfolge, Pfad und Gruppengröße. Beim *Lesen* zu filtern war
schlimmer: `ungroup` schreibt einen Stern von Marken — eine vom entfernten
Mitglied zu jedem verbliebenen —, und die Filterregel warf daraufhin ein
Mitglied pro Marke hinaus statt des einen gemeinsamen. Aus einer Vierergruppe,
aus der einer ging, wurde gar keine Gruppe mehr. Dazu wanderte die gefilterte
Mitgliederliste über den Log und **löschte** auf den anderen Geräten, was sie
hier nur verbarg.

Was damit offen bleibt, und zwar bewusst — und schmaler, als es zunächst
klingt: Auflösung und Marken schreibt ein Gerät in **denselben** Log, das
andere wendet beide in derselben Runde an, und die Ansichten lesen erst danach.
Das Fenster, das wirklich bleibt, ist die **unabhängige Neubildung**: Ein
Gerät, das die Gruppe nie hatte — frisch eingerichtet, oder der Tag gerade
offen —, bildet sie aus derselben Beitritts-URL, bevor die Marken es erreicht
haben. Dann steht die Gruppe wieder, einmal; die Marken kommen an, die
automatische Verknüpfung schweigt ab da, und das zweite Auflösen hält.
Schlimmster Fall: zweimal auflösen. Das ist der ehrliche Preis; die beiden
Versuche, ihn zu schließen, haben drei Review-Runden und 34 Funde gekostet.

**Und eine Marke lässt sich zurücknehmen.** Ausdrückliches Gruppieren ist die
gegenteilige Aussage und löscht die Marken zwischen den Paaren, die es
**benennt** (Migration 0038) — nicht über die ganze Gruppe hinweg, sonst nähme
ein „und dieses auch" eine Ablehnung zurück, über die in dieser Geste niemand
gesprochen hat.

Die Rücknahme **löscht die Zeile nicht**, sie stempelt sie. Löschen zerstörte
die Vereinigungs-Regel, auf der die Synchronisation dieser Tabelle beruht: Das
Löschen des einen Geräts und die überlebende Zeile des anderen verschmelzen
zurück zu „abgelehnt". Zwei Zeitstempel, die sich nur vorwärts bewegen,
verschmelzen dagegen zur selben Antwort, in welcher Reihenfolge sie auch
ankommen. Die spätere Aussage gewinnt, Gleichstand geht an die Ablehnung.

**Marken überleben einen ID-Wechsel — aber auf zwei verschiedene Arten, und
das ehrlich benannt.** Für Marken, deren Termin **Gruppenmitglied** ist, greift
dieselbe Reparatur wie für die Mitgliedschaft: `heal_member` und `relocate`
schreiben die Marke in derselben Transaktion um (`carry_declines`), und beim
Kalenderwechsel reist sie mit über den Log, weil die anderen Geräte einen Umzug
nicht selbst herleiten.

Für Marken über **nicht gruppierte** Paare — dem Hauptzweck einer Marke — gibt
es keine Heilung, und kann es keine geben: Eine nackte Marke hat weder
Mitgliedszeile noch Signatur, über die sich der Termin wiederfinden ließe.
Vergibt der Anbieter die Termin-ID neu, passt die Marke nicht mehr, das Paar
wird einmal erneut zur Verknüpfung angeboten, und der Nutzer lehnt einmal mehr
ab — die neue Marke nennt dann die neue ID. Das ist der Preis, und er ist
bewusst gewählt.

Der Versuch, ihn zu vermeiden, ist gescheitert und zurückgenommen: die Marke
**einseitig** über ihre stabile Meeting-Hälfte zu lesen. Das Argument dafür —
jede Marke, die über eine Verknüpfung spricht, nennt das Meeting — ist wahr und
nutzlos, denn die Regel braucht die Umkehrung: dass jede Marke, die das Meeting
*nennt*, über seine Verknüpfbarkeit spricht. Die ist falsch. `ungroup` schreibt
eine Marke zwischen dem gehenden Termin und **jedem** verbliebenen Mitglied, ein
Meeting wird also von einer Aussage über jemand anderen mitbenannt; und eine
Namens-Ablehnung konnte ebenfalls ein Meeting nennen. Beides hätte das Meeting
dauerhaft und global vom Verknüpfen ausgeschlossen — wegen einer Ablehnung, die
der Nutzer über es nie ausgesprochen hat. Ein wiederholtes Angebot nach einem
ID-Wechsel ist das billigere Übel.

**Ein Meeting wird nie auf Ähnlichkeit angeboten.** Die Namens-Vorschläge
schlossen Meeting-Zeilen nicht aus — ein Fehler, der älter ist als Stufe 4 und
den erst der einseitige Versuch sichtbar gemacht hat. Ein Meeting trägt eine
Identität; danach zu raten war nie die Absicht, und die Antwort auf die falsche
Frage wurde als Marke aufbewahrt. `findGroupSuggestions` überspringt sie jetzt.

Ein Altbestand, der `cleared_at` nicht kennt, liest eine zurückgenommene Marke
als gültige Ablehnung — belanglos, solange nichts ausgeliefert ist, notiert für
den Tag, an dem gemischte Versionen ein Sync-Ziel teilen.

**Ein gelöschter Termin hat nichts gesagt.** `ungroup` nimmt deshalb einen
Grund entgegen (`Removal::ByUser` / `Removal::Bookkeeping`). Nur das Herausnehmen
durch den Nutzer schreibt eine Marke; Aufräumen schreibt keine — weder bei der
Mitgliedschaft eines gelöschten Termins noch bei einer Kopie, die der
Serien-Übertrag gerade aus einer Gruppe in die nächste hebt, die sonst den Termin überlebte und eine ID
bände, die der Anbieter neu vergeben kann.

**Woher die Gruppe kommt, steht nicht an der Gruppe.** Der Entwurf wollte eine
Herkunft (`origin: 'meeting-link'`) — das wäre falsch: `group_events` tritt
einer bestehenden Gruppe bei, eine Gruppe kann also halb vom Nutzer und halb
automatisch sein. Die richtige Körnung ist das **Paar**, und genau die haben
die Ablehnungs-Marken.

**Nicht eindeutig heißt nicht gruppieren.** Eine Beitritts-URL kann auf mehrere
Termine passen — ein Dauer-Meetingraum etwa, auf den zwei Termine verweisen.
Trifft die Kennung mehr als einen Termin, wird **nichts** gruppiert: dieselbe
Eindeutigkeits-Regel, die die Selbstheilung schon anwendet, aus demselben Grund
— eine falsche Gruppe ist schlimmer als keine, weil sie authoritativ aussieht.

Gezählt werden dabei **Termine, nicht Zeilen**, und daran hängt mehr, als es
aussieht:

*Eine Serie zählt einmal.* Ein wiederkehrender Termin liefert eine Zeile pro
Tag; Zeilen zu zählen hätte dieselben Daten in der Wochenansicht mehrdeutig
gemacht und in der Tagesansicht nicht — eine Eigenschaft des geöffneten
Zeitraums, nicht der Daten. Mitgliedschaften laufen ohnehin über den
Serien-Master, und der wird gezählt.

*Eine Gruppe zählt einmal.* Kopien eines Termins in mehreren Kalendern tragen
dieselbe Beitritts-URL — genau das macht eine weitergeleitete Einladung. Sie
einzeln zu zählen hieße, ausgerechnet den Fall abzulehnen, für den es diese
Funktion gibt: einen Termin, den der Nutzer **bereits** für eine Sache erklärt
hat. „Zu welchem Termin gehört dieses Meeting?" hat dort genau eine Antwort, und
die Gruppe ist sie. Das Meeting tritt der Gruppe bei, statt eine zweite zu
eröffnen. Ein Anspruchsteller **außerhalb** der Gruppe macht es wieder
mehrdeutig und stoppt alles.

**Ein wiederkehrender Termin wird nicht angefasst.** Mitglieder einer Gruppe
sind Serien. Webex listet ein wiederkehrendes Meeting als eine Zeile **pro
Termin** mit je eigener ID, und eine Serien-ID liefert die Listen-Antwort nicht
— es gibt also keine Serie, die ein Mitglied benennen könnte. Und ein Meeting,
das *nicht* wiederkehrt, während der Termin es tut, ist an den meisten Tagen
tatsächlich nicht da. In beiden Fällen behauptete die Gruppe an jedem Tag außer
einem eine Kopie, die es dort nicht gibt. Der Filter versteckt die Doppelung
weiter wie bisher; von Hand gruppieren geht unverändert.

**Ein Meeting pro Konto und Termin.** Aus demselben Grund: Ist der
Meeting-Kalender in der Gruppe schon vertreten, kommt nichts mehr hinzu. Ohne
diese Regel reichte die Tagesansicht jeden Morgen eine andere Meeting-ID
herein, jede davon ein neues Mitglied — die Gruppe wüchse täglich, und die Zahl
auf der Zeile mit ihr.

**Der Kalender war immer schon ein Kalender.** Der Entwurf behauptete, der
Meeting-Kalender sei nicht anwählbar; das stimmte nicht. `VcCalendar` liefert
ihn seit jeher aus `list_calendars` — nur lesbar, benannt nach dem Konto —, er
steht also in der Seitenleiste und auf dem Kalender-Bildschirm und lässt sich
an- und abschalten wie jeder andere. Es war nichts zu bauen; die halbe Stufe
existierte bereits.

**Der Filter bleibt, mit einer Ausnahme.** `withoutDuplicateMeetings` kommt
nicht weg: Für ein Paar, das nicht eindeutig ist, ist er die einzige
Doppelungs-Bremse, und beim allerersten Laden greift er, bevor die erste Gruppe
geschrieben ist. Er lässt jetzt Zeilen in Ruhe, die **Mitglied einer Gruppe**
sind — dann faltet die Gruppe, und die Faltung zeigt eben beide Zeilen, sobald
sie sich uneins sind. Genau deshalb ist die Ausnahme keine Kosmetik.

Damit wanderte auch das Filtern: `useEvents` (Desktop) und `load()` (mobil)
liefern jetzt **alles**, und gefiltert wird dort, wo die Gruppen bekannt sind —
`useEventGroups` bzw. die Ansichten selbst. Das Paaren braucht beide Zeilen, und
die gab es vorher nach dem Filter nicht mehr.

**Schreibrechte klären sich von selbst.** Der Meeting-Kalender ist nur lesbar,
und das ist überall richtig behandelt: Gruppieren geht auf nur lesbaren Zeilen
(die Gruppe ist Aperios Aussage, kein Schreibvorgang), und das Mitziehen nennt
sie als das, was es nicht schreiben kann, statt sie still zu überspringen.

### Was es wert ist

Der sichtbare Gewinn ist klein — die Doppelung verschwand vorher auch schon.
Der Gewinn liegt darin, dass sie **richtig** verschwindet: nachvollziehbar, mit
beiden Zeilen erhalten, mit einer Zahl, die sagt was es gibt, mit einer Stelle,
an der man nachsehen kann — und mit dem einen Fall, der vorher garantiert
unsichtbar war: Termin verschoben, Meeting nicht.

## Größenordnung

Vergleichbar mit den Widgets: ein eigenes Vorhaben über Kern, Synchronisation und
beide Oberflächen, kein Nebenbei. Stufe 0 und 1 zusammen sind der ehrliche
Einstieg; alles danach ist optional und lässt sich am Gebrauch entscheiden.
