---
title: "03 – Termine"
---

In diesem Kapitel legst du Termine an, bearbeitest und verschiebst sie und
richtest Wiederholungen ein.

## Einen Termin anlegen

1. Wechsle in eine Kalenderansicht (z. B. **Woche**, siehe
   [Kapitel 05](/de/guides/tutorial/05-ansichten/)).
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
- **Erinnerung** (siehe [Kapitel 06](/de/guides/tutorial/06-benachrichtigungen/)),
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
Telefon langer Druck) und wählen Sie **Gehört zusammen mit…**. Der Dialog
listet die übrigen Termine dieses Tages; wählen Sie den Zwilling und bestätigen
Sie mit **Gruppieren**. Derselbe Dialog löst einen Termin wieder heraus
(**Diesen Termin herauslösen**) oder hebt die Gruppe ganz auf (**Gruppe
auflösen**).

Nichts davon erreicht den Anbieter. Das Gruppieren ändert keinen der beiden
Termine, das Auflösen lässt beide genau so zurück, wie sie waren — die Kalender
behalten ihre Kopien, Aperio weiß nur, dass es eine Verabredung ist. Die
Gruppierung reist wie alles andere zwischen Ihren Geräten.

Gehören beide Termine bereits zu *verschiedenen* Gruppen, verweigert Aperio die
Zusammenführung, statt zu raten: Zwei Aussagen darüber, was ein Termin ist,
zusammenzulegen wäre eine Entscheidung, um die Sie nie gebeten haben. Lösen Sie
zuerst einen davon heraus.

## Zusammenfassung

Du kannst Termine anlegen, bearbeiten, verschieben, löschen und wiederholen
lassen. Als Nächstes kümmern wir uns um Aufgaben.
