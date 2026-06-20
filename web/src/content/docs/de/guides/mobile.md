---
title: "Mobile App"
---

Aperio gibt es auch als **mobile App für iOS und Android**. Sie baut auf
**demselben Kern** wie die Desktop-App auf – deine Kalender, Aufgabenlisten,
Konten und die Synchronisation funktionieren also genau gleich. Und wie die
Desktop-App ist sie von Grund auf darauf ausgelegt, **vollständig mit einem
Screenreader bedienbar** zu sein (VoiceOver unter iOS, TalkBack unter Android).

Diese Seite behandelt das, was an der mobilen App **besonders** ist. Alles zu den
**Funktionen selbst** – Termine, Aufgaben, Ansichten, Erinnerungen, Suche,
Synchronisation, Kontakte und Farbetiketten – steht im
[Tutorial](/de/guides/tutorial/01-installation/) und gilt hier genauso.

## Die App bekommen

Die mobile App befindet sich derzeit in der **Beta-Phase**:

- **iOS** – Verteilung über **TestFlight**. Du bekommst einen Einladungslink und
  installierst sie über Apples TestFlight-App.
- **Android** – direkte Installation aus einem bereitgestellten Build.

Beide werden aus demselben Rust-Kern wie die Desktop-App gebaut – eine Aufgabe
oder ein Termin, den du am Handy anlegst, verhält sich also identisch zu einem am
Desktop.

## Zurechtfinden

Die App hat eine **untere Tab-Leiste** mit vier Tabs:

- **Aufgaben** – deine Aufgabenlisten, gruppiert und zusammenklappbar, mit dem
  ausführlichen Aufgaben-Editor.
- **Kalender** – die Ansichten Tag, Woche, Monat, Jahr und Agenda sowie die
  Kalenderverwaltung.
- **Kontakte** – deine Adressbücher und Kontakte.
- **Einstellungen** – Konten, Synchronisation, Erinnerungen, Farbetiketten,
  Protokolle und die allgemeinen Einstellungen.

Editoren (für eine Aufgabe, einen Termin oder einen Kontakt) öffnen sich als
**Vollbild** über dem aktuellen Tab. Jeder hat eine Aktion **Speichern** und
**Abbrechen**; mit der System-**Zurück**-Geste oder dem Abbrechen-Knopf verlässt
du ihn, ohne zu speichern.

## Mit einem Screenreader arbeiten

Die mobile App folgt denselben **Barrierefreiheits-Grundsätzen** wie die
Desktop-App (die gemeinsamen Konzepte stehen unter
[Barrierefreiheit](/de/guides/barrierefreiheit/)), angepasst an die Arbeitsweise
von VoiceOver und TalkBack:

- **Ein Stopp pro Eintrag.** Jede Aufgabe, jeder Termin und jeder Kontakt ist ein
  einzelner Fokus-Stopp. Mit Wischen nach links oder rechts bewegst du dich
  zwischen Einträgen, Überschriften und Bedienelementen.
- **Aktionen statt Tastenkürzel.** Wo der Desktop Tastenkürzel nutzt, bietet die
  mobile App **benutzerdefinierte Aktionen** am fokussierten Eintrag – eine
  Aufgabe erledigen oder wieder öffnen, bearbeiten, löschen, neu terminieren,
  Status oder Priorität ändern, verschieben und so weiter:
  - Mit **VoiceOver** wischst du mit einem Finger nach oben oder unten durch die
    verfügbaren Aktionen und tippst dann doppelt, um die ausgewählte auszuführen.
  - Mit **TalkBack** öffnest du das **Aktionsmenü** (nach oben, dann nach rechts
    wischen) und wählst eine Aktion.
- **Live-Ansagen.** Statusänderungen werden ohne Fokuswechsel angesagt –
  „Aufgabe erledigt", „Termin gespeichert", das Synchronisationsergebnis, fällige
  Erinnerungen – genau wie am Desktop.
- **Gruppenüberschriften** (z. B. eine Aufgabenliste oder ein Abschnitt) sind
  zusammenklappbare Schalter, die ansagen, ob sie auf- oder zugeklappt sind.
- **Datum und Uhrzeit** nutzen die **nativen Auswahlfelder**, lesen und verhalten
  sich also so, wie du es von anderen Apps auf deinem Handy kennst.

## Einstellungen nur für Mobil

Ein paar Einstellungen gibt es nur auf dem Handy, unter **Einstellungen →
Allgemein**. Alle drei werden **nur auf diesem Gerät** gespeichert (sie werden
nicht synchronisiert):

- **Hintergrund-Synchronisation.** Lässt das System die App wecken, um zu
  synchronisieren, während sie im Hintergrund oder geschlossen ist – so kommen
  Änderungen anderer Geräte und neue Erinnerungen an, ohne dass du die App
  öffnest. Den genauen Zeitpunkt bestimmt das Betriebssystem (nicht sofort; unter
  Android mindestens alle 15 Minuten, unter iOS in vom System gewählten Fenstern),
  es ist also ein „best effort". Beim Öffnen synchronisiert die App immer
  vollständig, es geht also nie etwas verloren. Hintergrund-Runden siehst du im
  Sync-Protokoll mit der Kennzeichnung **Hintergrund**. Standard: an.
- **App-Symbol-Badge.** Zeigt auf dem App-Symbol eine Zahl für die heute noch
  offenen Aufgaben plus die heute noch anstehenden Termine. Benötigt die
  Benachrichtigungsberechtigung. Standard: an.
- **Haptisches Feedback.** Eine kurze Vibration, wenn eine Aktualisierung
  externer Daten beginnt und endet. Standard: an.

## Erinnerungen und Benachrichtigungen

Erinnerungen werden als **lokale Benachrichtigungen** zugestellt. Bei der ersten
Nutzung fragt die App nach der **Benachrichtigungsberechtigung** – erteile sie,
damit Erinnerungen (und das App-Symbol-Badge) erscheinen können. Erinnerungstöne,
Vorlaufzeiten und Schlummern funktionieren wie unter
[Benachrichtigungen](/de/guides/tutorial/06-benachrichtigungen/) beschrieben.
