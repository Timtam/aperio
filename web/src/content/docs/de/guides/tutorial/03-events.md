---
title: "03 – Termine"
---

In diesem Kapitel legst du Termine an, bearbeitest und verschiebst sie und
richtest Wiederholungen ein.

## Einen Termin anlegen

1. Wechsle in eine Kalenderansicht (z. B. **Woche**, siehe
   [Kapitel 06](/de/guides/tutorial/06-views/)).
2. Navigiere mit den Pfeiltasten zum gewünschten Tag bzw. zur Uhrzeit.
3. Lege ein Ereignis an: **Termin schnell anlegen** (`Strg+N`) öffnet den
   Schnell-Dialog, **Neuer Termin** (`Strg+Umschalt+N`) das vollständige
   Formular – oder über das Kontextmenü. Der gerade markierte Zeitpunkt
   wird als Startzeit vorgeschlagen.
4. Im Dialog gibst du mindestens einen **Titel** ein.

Im Termin-Dialog kannst du außerdem festlegen:

- **Beginn und Ende** (oder **Ganztägig**),
- **Kalender**, in dem der Termin gespeichert wird,
- **Ort** und **Beschreibung**,
- **Farb-Label** (mit Farbpunkt in der Auswahl),
- **Erinnerung** (siehe [Kapitel 07](/de/guides/tutorial/07-notifications/)),
- **Teilnehmer** (Name und/oder E-Mail-Adresse),
- **Wiederholung** (siehe unten).

> **Beginn und Ende hängen zusammen:** Verschiebst du den **Beginn**, wandert
> das **Ende** um denselben Betrag mit – die Dauer bleibt erhalten, auch über
> Mitternacht und über mehrere Tage hinweg. Änderst du das **Ende**, ändert
> sich nur die Dauer; ein Ende vor dem Beginn wird auf den Beginn gesetzt. Neue
> Termine starten zur nächsten halben Stunde (an einem anderen Tag: 9:00 Uhr)
> und dauern eine Stunde. Das gilt auf dem Desktop und in der mobilen App
> gleichermaßen.

> **Eigene Farbe:** Neben dem Farb-Label-Auswahlfeld findest du die
> Schaltfläche **„Andere Farbe…"**. Damit komponierst du spontan eine
> beliebige Farbe (Hex-Wert oder Farbfeld) und wendest sie direkt an – ohne
> erst in die Einstellungen zu müssen. Optional übernimmst du sie dabei als
> benanntes Label in deine Palette. Dieselbe Möglichkeit bietet in der
> Seitenleiste das Kontextmenü **Farbe → Andere…** an Kalendern und Listen.

> **Einzelnen Termin umfärben:** Neben dem Dialog kannst du einen Termin
> direkt über sein **Rechtsklick-Menü** (Untermenü **Farbe**) umfärben –
> praktisch für eine schnelle Anpassung ohne das volle Formular. Wo der
> Anbieter des Kalenders eine Pro-Termin-Farbe speichern kann (lokale
> Kalender und farbfähige CalDAV-Server), reist die Farbe mit dem Termin und
> erscheint auch in anderen Clients. Bei iCloud, Google, Exchange/Outlook und
> abonnierten Feeds wird die Farbe stattdessen lokal auf diesem Gerät behalten
> (damit sie nie einen Sync-Fehler auslöst) und bleibt genauso angewandt.

> **Abonnierte Kalender:** Wenn ein abonnierter Kalender (iCal-Feed) eigene
> Pro-Termin-Farben setzt, werden diese jetzt ebenfalls angezeigt – nur
> lesend, da ein abonnierter Feed nicht bearbeitet werden kann.

Mit **Speichern** wird der Termin angelegt; eine Live-Region bestätigt
„Termin gespeichert".

> **Teilnehmer benachrichtigen:** Organisierst du einen Termin, ist jemand
> anderes eingeladen und unterstützt der Kalender den serverseitigen Versand
> (iCloud, Google, Exchange/Outlook, Microsoft 365), kann der Anbieter die
> Teilnehmer benachrichtigen – Aperio selbst versendet keine E-Mails. Bei
> Google und Exchange/Outlook erscheint das Kontrollkästchen **Teilnehmer
> benachrichtigen** (standardmäßig aktiv); ist es gesetzt, verschickt der
> Anbieter beim Speichern Einladungen bzw. Aktualisierungen. iCloud und
> Microsoft 365 informieren die Teilnehmer von sich aus über jede Änderung und
> können nicht still speichern. Statt des Kontrollkästchens sagt der Dialog
> das, zum Beispiel „iCloud informiert die Teilnehmer über jede Änderung“.
> Diesen Satz erreichst du mit Tab, und er wird angesagt, wenn er beim
> Bearbeiten erscheint, etwa wenn du den ersten Teilnehmer hinzufügst.
>
> Der Organisator zählt nie als Teilnehmer: Ein Termin, den du in Outlook
> angelegt hast und bei dem Outlook dich als einzigen Teilnehmer führt, hat in
> Aperio keine Teilnehmer. Bei einer Besprechung, die jemand anderes
> organisiert, kann nur diese Person Aktualisierungen verschicken; dort
> erscheinen weder das Kontrollkästchen noch der Satz. Entfernst du den
> letzten Teilnehmer, bleiben Kontrollkästchen oder Satz stehen, damit er
> eine Absage bekommen kann. Änderst du nur Titel oder Zeit, lässt Aperio die
> Teilnehmerliste beim Anbieter genau, wie sie ist, samt ihren Antworten.
>
> Löschst du eine **Besprechung, die du organisierst** (mit Teilnehmern, auf
> einem Konto mit Server-Terminplanung), fragt Aperio in **einem** Dialog nach –
> ohne versteckten zweiten Schritt. Bei einem **Serientermin** hat der Dialog
> eine Auswahlgruppe **Teilnehmer benachrichtigen / Ohne Benachrichtigung
> entfernen** (Standard: benachrichtigen) und darunter je eine Schaltfläche für
> den Umfang: **nur diesen Termin**, **diesen und alle folgenden** sowie **die
> ganze Serie**. So kannst du gezielt eine einzelne Wiederholung absagen (die
> Teilnehmer bekommen genau für dieses Datum eine Absage), die Serie ab einem
> Datum beenden (**diesen und alle folgenden** behält die früheren Termine und
> entfernt diesen sowie jeden späteren) oder alles absagen – die Auswahlgruppe
> entscheidet jeweils, ob eine E-Mail rausgeht. Bei einem Einzeltermin bleibt
> nur die Benachrichtigen-Auswahl. Bei iCloud und Microsoft 365 verschickt das
> Löschen die Absage immer. Dort gibt es keine Auswahlgruppe: Der Dialog sagt,
> wer die Teilnehmer informiert, und jeder Umfang sagt ab. Bei einer
> Besprechung, zu der du nur eingeladen bist, oder einem Termin ohne
> Teilnehmer wird ohne Rückfrage gelöscht.

> **Eine Einladung, die jemand anderes organisiert:** Bei iCloud – und auf
> jedem Kalenderserver, der die Terminplanung übernimmt – nimmt der Server bei
> einer Besprechung, zu der du eingeladen bist, nur deine eigene Antwort und
> deine eigenen Erinnerungen an. Alles andere lehnt er ab, deshalb zeigt
> Aperio so eine Besprechung schreibgeschützt: Titel, Kalender, Zeiten, Ort,
> Beschreibung und die Gästeliste stehen alle da, jedes als eigener Halt für
> den Screenreader, aber keines lässt sich ändern. Eine Zeile über den Feldern
> sagt, warum.
>
> Was du weiter tun kannst: antworten (Zusagen, Vorläufig, Absagen), eigene
> Erinnerungen setzen – angehängte, die der Kalender behält, oder eigene, die
> nur Aperio meldet –, einen Klang wählen, eine Farbe vergeben, einer
> Konferenz beitreten, die Verfügbarkeit der Teilnehmer prüfen und löschen.
> Das Antworten schließt den Editor nicht mehr, du kannst also antworten und
> danach eine Erinnerung setzen.
>
> Löschst du die Besprechung, erfährt es der Organisator: Der Dialog sagt
> vorher „Der Organisator bekommt eine Absage“, und die Schaltfläche heißt
> **Löschen und absagen**. Beim Überspringen eines einzelnen Termins steht
> derselbe Satz über den gewohnten Umfang-Schaltflächen. Die Serie eines
> anderen früher zu beenden, wird nicht angeboten – der Server würde es
> ablehnen. Auf einen anderen Tag oder eine andere Zeit ziehen geht ebenfalls
> nicht, und Aperio sagt das; in einen anderen Kalender verschieben geht
> weiter, und die Kopie hat keine Teilnehmer.
>
> Die Wiederholung steht als Satz da – „jeden Montag, 5 Mal“ –, weil es keine
> Bedienelemente zu lesen gibt. Kann Aperio eine Regel nicht in Worte fassen,
> sagt es das offen.

> **Verfügbarkeit prüfen:** Hat ein Termin Teilnehmer und unterstützt der
> Kalender die Server-Terminplanung, erscheint die Schaltfläche
> **Verfügbarkeit prüfen**, unter **Teilnehmer benachrichtigen**, wenn es da
> ist, und auch bei einer Besprechung, die jemand anderes organisiert. Sie
> fragt für das aktuell
> eingestellte Zeitfenster ab, welche Teilnehmer **frei** oder **belegt**
> sind, und zeigt das Ergebnis pro Teilnehmer mit einer Zusammenfassung an
> (die Live-Region kündigt es an). Antwortet ein Anbieter nicht (fehlende
> Berechtigung), gilt der Teilnehmer als „frei/unbekannt".

> **Einladungen beantworten (RSVP):** Öffnest du ein Meeting, zu dem du
> eingeladen wurdest (iCloud, Google, Exchange/Outlook), erscheint oben im
> Dialog **Deine Antwort** mit den Schaltflächen **Zusagen**, **Vorläufig**
> und **Absagen** – die aktuelle Antwort ist hervorgehoben. Deine Antwort
> geht automatisch an den Organisator. Bist du selbst der Organisator,
> siehst du stattdessen den Antwortstatus aller Teilnehmer; der Organisator
> ist nicht darunter.

## Termine bearbeiten, verschieben, löschen

- **Bearbeiten:** Termin markieren und mit `Eingabe` öffnen, ihn
  **doppelklicken** oder über das Kontextmenü **Bearbeiten** wählen.
- **Verschieben:** Im Dialog die Zeiten ändern – das funktioniert
  zuverlässig und screenreader-freundlich. Per Maus kannst du einen Termin
  auch auf einen **anderen Tag** in der Wochen- oder Monatsansicht ziehen,
  dann bleiben Uhrzeit und Dauer erhalten, oder in das Stundenraster der
  Tages- oder Wochenansicht, dann bekommt er die Uhrzeit, an der du ihn
  loslässt. Auf einen **Kalender in der Seitenleiste** gezogen, wandert er in
  diesen Kalender. Bei Serienterminen fragt Aperio, ob nur dieser Termin oder
  die ganze Serie verschoben werden soll. Die ganze Serie verschiebt jeden
  Termin um dieselbe Zahl von Tagen: Aus „jeden Montag“ wird „jeden
  Dienstag“, und das Enddatum wandert mit. Manche Regeln lassen sich so nicht
  verschieben, etwa „am zweiten Sonntag jedes Monats“ oder ein Tag nach dem
  28.; dann sagt Aperio, warum, und bietet an, nur diesen Termin zu
  verschieben. Siehe auch **Einzeln geänderte oder gelöschte Termine** unten.
- **Löschen:** Termin markieren und **Löschen** wählen (Standard: `Entf`).
  Vor dem Löschen wird nachgefragt.

## Wiederkehrende Termine

Im Termin-Dialog unter **Wiederholung** wählst du ein Muster:

- täglich, wöchentlich (mit Wochentagen), monatlich, jährlich,
- ein **Ende** (nie, nach X Malen, bis zu einem Datum).

Beim Bearbeiten oder Löschen eines wiederkehrenden Termins fragt Aperio vorab,
ob sich die Änderung auf **nur diesen Termin**, **diesen und alle folgenden**
oder die **ganze Serie** beziehen soll – dieselben drei Umfänge wie bei anderen
Kalendern (Google, Outlook). **Diesen und alle folgenden** teilt die Serie am
gewählten Termin: Die früheren Termine bleiben unangetastet, dieser und jeder
spätere werden geändert (beim Bearbeiten übernimmt ab hier eine neue Serie) oder
entfernt (beim Löschen). Die neue Serie wiederholt sich so, wie es das Feld für
die Wiederholung sagt: Änderst du dort die Regel, folgt die neue Serie ihr;
lässt du sie, läuft das Muster einfach weiter. Liegt vor dem gewählten Termin
keiner mehr – du hast den ersten gewählt, oder alle früheren sind gelöscht –,
gibt es nichts zu behalten: Das Bearbeiten ändert die ganze Serie, die derselbe
Eintrag bleibt, und das Löschen entfernt die Serie. Aperio sagt es dir dann.
Beim Bearbeiten legt Aperio zuerst die neue Serie an und beendet dann die alte
davor. Scheitert das Beenden, löscht Aperio die neue wieder, und nichts hat sich
geändert. Ist unklar, ob die alte schon beendet wurde, etwa nach einer
abgerissenen Verbindung, bleiben beide stehen: Deine Änderung gilt als
gespeichert, und der Editor bleibt mit einem Hinweis offen, der den Tag nennt,
ab dem die Serie möglicherweise doppelt steht, bis du ihn schließt. Scheitert
auch das Löschen der neuen, sagt Aperio auch das und bittet dich, den Kalender
zu prüfen, bevor du erneut speicherst.
**Die ganze Serie** öffnet die Serie selbst, mit ihrem eigenen Beginn und Ende,
auch wenn du sie von einem späteren Termin aus geöffnet hast. Eine neue Uhrzeit gilt für jeden Termin, ein neues Datum verschiebt den
Beginn der Serie. Der Titel des Editors nennt deine Wahl, etwa **Nur diesen
Termin bearbeiten**, und wird beim Öffnen vorgelesen. Am Desktop wiederholt das
Formular sie im schreibgeschützten Feld **Anwenden auf**, das du wie jedes andere
Feld mit `Tab` erreichst.

> **Einzeln geänderte oder gelöschte Termine:** Verschiebst du eine ganze Serie
> auf einen anderen Tag oder eine andere Uhrzeit, per Ziehen oder im Dialog,
> bleiben manche Einzeltermine an ihrem alten Datum hängen: Termine, die in
> einem externen Kalender (iCloud, Google, Exchange) für sich geändert wurden,
> und Termine, die in einem Google-Kalender gelöscht wurden, auch wenn du sie
> in Aperio gelöscht hast (womöglich auch in Exchange). Nach dem Verschieben
> kann ein gelöschter Termin wieder auftauchen, ein geänderter doppelt
> erscheinen, und an der alten Stelle kann ein anderer Termin fehlen. Aperio
> kann das nicht reparieren. Sieh dir die Serie danach in der App des Kalenders
> an; dort lassen sich solche Einzeländerungen rückgängig machen.

> **Einen schon geänderten Termin wieder ändern:** Ein Termin einer Serie, der
> in der App des Kalenders (iCloud, Google, Exchange) für sich geändert wurde,
> bleibt Teil seiner Serie, wenn du wieder nur diesen Termin änderst oder
> verschiebst: im Termin-Dialog am Desktop wie in der Handy-App und per Ziehen am
> Desktop. Einen Termin, den du in Aperio für sich geändert hast, hat Aperio
> schon zu einem eigenen Termin gemacht. Eine Ausnahme macht Exchange: Dort darf
> so ein Termin nicht auf oder über einen anderen Termin derselben Serie rücken.
> Aperio macht ihn dann zu einem eigenen Termin zur neuen Zeit, so wie jeden
> anderen Termin, den du einzeln verschiebst.

> **Tipp:** Wiederkehrende Termine aus externen Kalendern (z. B. iCloud)
> werden in allen Ansichten korrekt aufgeklappt – auch dann, wenn die erste
> Wiederholung in der Vergangenheit liegt.

> **Screenreader-Hinweis:** Beim Anlegen springt der Fokus in das
> Titelfeld des Dialogs. Mit `Tab`/`Umschalt+Tab` gehst du die Felder
> durch; `Esc` bricht ab, ohne zu speichern. In der Ansicht werden Termine
> beim Markieren mit Titel, Uhrzeit und Kalender angesagt.

## Derselbe Termin in mehreren Kalendern

Ein und dieselbe Verabredung liegt oft mehrfach vor: im Arbeitskalender, damit
die Kollegen sie sehen, noch einmal im Privatkalender, weil dieser an einen
Sprachassistenten hängt, und drittens im Kalender einer Kollegin, den Aperio
ebenfalls liest. Für jeden Anbieter sind das unabhängige Termine — Aperio kann
man es sagen.

Öffnen Sie das Kontextmenü eines Termins (Rechtsklick, `Umschalt+F10`, am
Telefon langer Druck) und wählen Sie **Gehört zusammen mit…**. Ist der Termin
bereits gruppiert, heißt der Eintrag **Gruppierung verwalten…** — dasselbe
Fenster, aber der Name sagt jetzt, dass es dort auch etwas zu verwalten gibt.
Der Dialog
listet die übrigen Termine dieses Tages; wählen Sie den Zwilling und bestätigen
Sie mit **Gruppieren**. Die Liste reicht dabei auch in **ausgeblendete
Kalender** — der Kollegenkalender ist oft genau deshalb aus, weil er laut ist,
und liegt dort doch die dritte Kopie. Solche Vorschläge tragen den Zusatz
„(Kalender ausgeblendet)", damit kein Termin aus dem Nichts auftaucht. Derselbe Dialog löst einen Termin wieder heraus
(**Diesen Termin herauslösen**) oder hebt die Gruppe ganz auf (**Gruppe
auflösen**). Die Mitglieder stehen dort als **Liste**, und jedes lässt sich
öffnen: So kommen Sie von der Gruppe direkt in den Editor der Kopie, die Sie
gerade meinen, und danach wieder zurück.

Nichts davon erreicht den Anbieter. Das Gruppieren ändert keinen der beiden
Termine, das Auflösen lässt beide genau so zurück, wie sie waren — die Kalender
behalten ihre Kopien, Aperio weiß nur, dass es eine Verabredung ist. Die
Gruppierung reist wie alles andere zwischen Ihren Geräten.

Ist die zweite Kopie die offensichtliche — gleicher Name, gleiche Zeit, anderer
Kalender —, ist sie beim Öffnen des Dialogs bereits ausgewählt, mit einer Zeile,
die sagt warum. Bestätigen ist ein Tastendruck, Widersprechen heißt, etwas
anderes zu wählen. Von sich aus gruppiert Aperio nie: In einem Büro voller
„Team-Meeting" um 10:00 würde das zwei verschiedene Besprechungen zu einer
Verabredung erklären, und eine falsche Gruppe versteckt eine echte Verpflichtung
hinter der Kopie von etwas anderem.

Gehören beide Termine bereits zu *verschiedenen* Gruppen, verweigert Aperio die
Zusammenführung, statt zu raten: Zwei Aussagen darüber, was ein Termin ist,
zusammenzulegen wäre eine Entscheidung, um die Sie nie gebeten haben. Lösen Sie
zuerst einen davon heraus.

### Was sich mit einer Gruppe ändert

**Eine Zeile statt vier.** Jede Ansicht zeigt die Verabredung einmal, und die
Zeile sagt, wofür sie steht: „ein Termin mit 2 weiteren, in Arbeit, Privat". Die
Zahl gehört der Gruppe — eine Kopie in einem abgeschalteten Kalender zählt mit
und passt so zu dem, was Sie zu haben wissen.

Sichtbar ist es auch: Eine gefaltete Zeile trägt eine kleine Marke — „3×" für
die Verabredung und ihre zwei weiteren Kopien.

Eine Ausnahme, und die ist Absicht: Sind die Kopien **auseinandergelaufen** —
eine wurde verschoben, die andere nicht —, wird NICHT gefaltet. Jede bleibt
sichtbar und sagt es, mit einer hervorgehobenen Marke („3× ≠"). Die Gruppe
stimmt dann nicht mehr, und genau das ist das Einzige, was Sie sehen müssen.

**Ein Bearbeiten statt vier.** Nach dem Speichern einer Änderung an einem
gruppierten Termin fragt Aperio, ob die anderen Kopien nachziehen sollen — und
nennt jede, die es schreiben wird, und jede, die es nicht darf. Ein
Kollegenkalender ist nur lesbar, und ihn still zu überspringen ist der Weg, auf
dem eine Gruppe am Ende zwei verschiedene Zeiten meint. Die Kopien werden nach
ihrem **Kalender** benannt: Der Titel ist auf allen derselbe — das macht sie ja
zur Gruppe.

Geht dabei etwas schief, bleibt der Dialog offen und sagt, welche Kalender
nicht geschrieben werden konnten; **Rest erneut versuchen** nimmt genau die noch
einmal vor. Halb mitgezogen ist der eine Zustand, den Sie sehen müssen. Anders
eine Kopie, deren Serie ab dem Schnitt möglicherweise doppelt steht: Ihr neuer
Teil ist angelegt, deshalb wird sie nicht noch einmal angeboten, und ein Hinweis
nennt stattdessen ihren Kalender und den Tag. Bleiben nur solche Hinweise, bleibt
der Dialog mit ihnen offen, ohne **Rest erneut versuchen**.

Mitgezogen wird nur, was der Termin IST: Titel, wann, wo, Beschreibung.
**Erinnerungen bleiben bei jeder Kopie** — die Privatkopie gibt es meist genau
deshalb, weil sie eine Erinnerung trägt, die die Arbeitskopie nicht hat. Farbe,
Kalender und Teilnehmer bleiben aus demselben Grund pro Kopie.

Die Frage kommt nach dem Speichern, nie davor: Ihre eigene Änderung steht damit
nie auf dem Spiel, und Abbrechen kostet nichts.

**Auch bei Serien, und in jedem Umfang.** Ändern Sie **nur dieses Vorkommen**,
wird bei jeder Kopie dasselbe getan, was mit Ihrem Termin geschah: das Vorkommen
aus der Serie geschnitten und ein Einzeltermin an seine Stelle gesetzt. Bei
**diesem und allen folgenden** wird die Serie jeder Kopie an derselben Stelle
geteilt — die früheren Vorkommen bleiben unangetastet, die späteren tragen die
Änderung. Läuft eine Kopie in einem anderen Takt (zweiwöchentlich gegen
wöchentlich), wird sie an ihrem eigenen nächsten Vorkommen geteilt; hat sie ab
dort keines mehr, wird sie genannt statt still übergangen, und hat sie davor
keines, wird sie als Ganzes geändert statt geteilt.

Der Dialog sagt jedes Mal dazu, welche Vorkommen betroffen sind. Und weil beide
Umfänge NEUE Einträge erzeugen, werden die anschließend wieder miteinander
verknüpft: Sonst wäre die Verabredung, die Sie gerade zu einer Zeile gemacht
haben, ab dieser Stelle wieder vier.

**Das Meeting gehört der Verabredung.** Ein Meeting-Link hängt an genau einem
Termin, und an welchem, ist ein Zufall des Moments, in dem verknüpft wurde.
Innerhalb einer Gruppe erscheint **Beitreten** an der Kopie, die Sie gerade vor
sich haben.

**Kopien werden wiedergefunden.** Termin-Kennungen gehören dem Anbieter und
ändern sich unter Aperio — eine Neu-Synchronisierung vergibt sie neu, das
Verschieben zwischen Kalendern ebenso. Eine Gruppe merkt sich Name und Beginn
jedes Mitglieds; löst eine Kennung nichts mehr auf, wird die Kopie gesucht und
die Gruppe repariert sich still. Wird nichts Passendes gefunden, bleibt alles,
wie es ist: Es könnte eine Kopie sein, die Sie gelöscht haben, und das auf
Verdacht zu entscheiden steht Aperio nicht zu.

## Zeitschritte

Unter **Einstellungen → Allgemein** legt **Zeitschritte** fest, wie weit ein
Druck der Pfeiltasten ein Uhrzeitfeld bewegt: 1, 5, 10, 15 oder 30 Minuten
(standardmäßig 15). Eine Uhrzeit, die nicht auf diesem Raster liegt, springt
weiter im Minutentakt — so wird nichts unspeicherbar, was du schon gespeichert
hast, und die genaue Eingabe funktioniert immer.

Am Telefon wirkt dieselbe Einstellung anders, weil die Plattform keine Wahl
lässt: Das Rad des nativen Pickers bewegt sich immer minutenweise. Dort
entscheidet die Einstellung, welche Minuten der Knopf **Minuten** neben einem
Uhrzeitfeld anbietet — ein Tippen statt dreißig Wischer bis zur halben Stunde.

## Signaturen

Eine Signatur ist ein benannter Textblock, der ans **Ende** einer Beschreibung
kommt — die Zugangsdaten eines Raums, ein stehender Hinweis, die Einwahl einer
Abteilung. Unter **Einstellungen → Signaturen** — am Telefon über
**Signaturen** in den Einstellungen — schreibst du sie. WELCHE ein Kalender
trägt, legst du am Kalender selbst fest, neben seinen Standard-Erinnerungen,
und ein Kalender mit Signatur setzt sie **von selbst auf neue Termine**: im
Normalfall kein einziger Druck.

Die Bindung ist ein Standard, keine Einschränkung. Jede Signatur bleibt im
Editor wählbar, egal auf welchem Kalender der Termin liegt, und der Knopf
**Signatur** neben der Beschreibung ist für genau diese Ausnahmen da — auch,
um eine wieder herauszunehmen. Wechselst du den Kalender, wird der Block gegen
den des neuen Kalenders getauscht; selbst geschriebener Text oder ein Block,
den du gelöscht hast, bleibt unangetastet.

Das Einfügen ist wiederholbar: Ein zweiter Druck **ersetzt** den Block, statt
einen weiteren anzuhängen, und der Wechsel zu einer anderen Signatur tauscht
ihn aus. Dein eigener Text darüber bleibt unangetastet — eine Signatur ist eine
Ergänzung am Ende, keine Umschreibung. Getrennt wird sie durch eine Zeile mit
`-- `, derselben Marke, die auch Mailprogramme benutzen — und woran Aperio den
eigenen Block wiederfindet.

**Nur Klartext, und das ist keine Einschränkung, die wir gewählt haben.** Eine
verschickte Einladung reist als iCalendar, und dessen Beschreibungsfeld ist als
reiner Text definiert — HTML landet dort als sichtbare Tags bei jedem
Empfänger, dessen Programm es wörtlich darstellt. Setze einen Link in eine
eigene Zeile, dann macht ihn praktisch jedes Programm anklickbar, und mehr
Formatierung braucht eine Einladung nicht.

## Zusammenfassung

Du kannst Termine anlegen, bearbeiten, verschieben, löschen und wiederholen
lassen. Als Nächstes kümmern wir uns um Aufgaben.
