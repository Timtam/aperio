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
> Beim Löschen eines Termins mit Teilnehmern wird entsprechend eine Absage
> ausgelöst.

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

- **Bearbeiten:** Termin markieren und mit `Eingabe` (oder über das
  Kontextmenü **Bearbeiten**) öffnen.
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

Beim Bearbeiten oder Löschen eines wiederkehrenden Termins fragt Aperio, ob
sich die Änderung auf **nur diesen Termin**, **diesen und alle folgenden**
oder **alle** beziehen soll.

> **Tipp:** Wiederkehrende Termine aus externen Kalendern (z. B. iCloud)
> werden in allen Ansichten korrekt aufgeklappt – auch dann, wenn die erste
> Wiederholung in der Vergangenheit liegt.

> **Screenreader-Hinweis:** Beim Anlegen springt der Fokus in das
> Titelfeld des Dialogs. Mit `Tab`/`Umschalt+Tab` gehst du die Felder
> durch; `Esc` bricht ab, ohne zu speichern. In der Ansicht werden Termine
> beim Markieren mit Titel, Uhrzeit und Kalender angesagt.

## Zusammenfassung

Du kannst Termine anlegen, bearbeiten, verschieben, löschen und wiederholen
lassen. Als Nächstes kümmern wir uns um Aufgaben.
