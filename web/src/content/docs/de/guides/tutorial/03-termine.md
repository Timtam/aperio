---
title: "03 – Termine"
---

In diesem Kapitel legst du Termine an, bearbeitest und verschiebst sie und
richtest Wiederholungen ein.

## Einen Termin anlegen

1. Wechsle in eine Kalenderansicht (z. B. **Woche**, siehe
   [Kapitel 06](/de/guides/tutorial/06-ansichten/)).
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
- **Erinnerung** (siehe [Kapitel 07](/de/guides/tutorial/07-benachrichtigungen/)),
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

> **Teilnehmer benachrichtigen:** Hat ein Termin Teilnehmer und unterstützt
> der Kalender den serverseitigen Versand (iCloud, Google, Exchange/Outlook),
> erscheint das Kontrollkästchen **Teilnehmer benachrichtigen** (standardmäßig
> aktiv). Ist es gesetzt, verschickt der Anbieter beim Speichern automatisch
> Einladungen bzw. Aktualisierungen – Aperio selbst versendet keine E-Mails.
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
> nur die Benachrichtigen-Auswahl. Bei einer Besprechung, zu der du nur
> eingeladen bist, oder einem Termin ohne Teilnehmer wird ohne Rückfrage
> gelöscht. (Auf iCloud/CalDAV entscheidet der Server über den Absageversand –
> dort ist „ohne Benachrichtigung" nicht garantiert.)

> **Verfügbarkeit prüfen:** Unter demselben Schalter erscheint die
> Schaltfläche **Verfügbarkeit prüfen**. Sie fragt für das aktuell
> eingestellte Zeitfenster ab, welche Teilnehmer **frei** oder **belegt**
> sind, und zeigt das Ergebnis pro Teilnehmer mit einer Zusammenfassung an
> (die Live-Region kündigt es an). Antwortet ein Anbieter nicht (fehlende
> Berechtigung), gilt der Teilnehmer als „frei/unbekannt".

> **Einladungen beantworten (RSVP):** Öffnest du ein Meeting, zu dem du
> eingeladen wurdest (iCloud, Google, Exchange/Outlook), erscheint oben im
> Dialog **Deine Antwort** mit den Schaltflächen **Zusagen**, **Vorläufig**
> und **Absagen** – die aktuelle Antwort ist hervorgehoben. Deine Antwort
> geht automatisch an den Organisator. Bist du selbst der Organisator,
> siehst du stattdessen den Antwortstatus aller Teilnehmer.

## Termine bearbeiten, verschieben, löschen

- **Bearbeiten:** Termin markieren und mit `Eingabe` öffnen, ihn
  **doppelklicken** oder über das Kontextmenü **Bearbeiten** wählen.
- **Verschieben:** Im Dialog die Zeiten ändern – das funktioniert
  zuverlässig und screenreader-freundlich. Per Maus kannst du einen Termin
  auch auf einen **anderen Tag** in der Wochen- oder Monatsansicht ziehen
  (Uhrzeit und Dauer bleiben erhalten) oder auf einen **Kalender in der
  Seitenleiste**, um ihn in diesen Kalender zu verschieben. Bei
  Serienterminen fragt Aperio, ob nur dieser Termin oder die ganze Serie
  verschoben werden soll.
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
entfernt (beim Löschen).

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
einmal vor. Halb mitgezogen ist der eine Zustand, den Sie sehen müssen.

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
dort keines mehr, wird sie genannt statt still übergangen.

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
Abteilung. Unter **Einstellungen → Signaturen** schreibst du sie und bindest je
eine an einen Kalender; der Editor bietet dann diese mit einem einzigen Druck
an, und einen Dialog für die Ausnahmen.

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
