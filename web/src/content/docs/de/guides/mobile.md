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
- **Die Verwaltungslisten** – Kalender, Aufgabenlisten, Adressbücher – geben
  jedem Eintrag genau eine Fokusstelle: einen **Schalter**, der ihn ein- oder
  ausblendet und dabei Name, Konto und Farbe ansagt. Das Verwalten (beim
  Adressbuch: Bearbeiten und Löschen) liegt als Aktion auf derselben Stelle –
  zwölf Kalender sind damit zwölf Wischer statt sechsunddreißig. Die
  Schaltflächen bleiben für alle anderen sichtbar.
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

## Widget auf dem Startbildschirm (iOS)

**Als Nächstes** zeigt die nächsten Termine und fälligen Aufgaben auf dem
Startbildschirm. Hinzufügen wie gewohnt: Startbildschirm gedrückt halten,
**Widget hinzufügen** wählen, **Aperio** suchen und eine Größe auswählen.
VoiceOver liest jede Zeile als einen Satz – Titel, Tag, Uhrzeit – statt als
einzelne Bruchstücke, zwischen denen du wischen musst.

Die Widgets folgen **Aperios Sprache**, nicht der des Telefons – hast du Aperio
also auf einem englischen Telefon auf Deutsch gestellt, sind auch die Widgets
deutsch. Uhrzeitformat und Datumsreihenfolge bleiben die Regionseinstellungen des
Telefons, so wie sich jede andere Uhr auf dem Startbildschirm verhält.

Das Widget liest aus einer kleinen Übersicht, die die App aktuell hält und die
die nächsten sieben Tage abdeckt. Sie wird jedes Mal erneuert, wenn die App
läuft, und bei jeder Hintergrund-Synchronisation – so bleibt das Widget aktuell,
ohne den Akku zu belasten. Zwei Zustände sind bewusst unterschiedlich formuliert:

- **„Nichts geplant."** – in den nächsten sieben Tagen steht wirklich nichts an.
- **„Keine aktuellen Daten. Öffne Aperio."** – das Widget ist über das hinaus,
  was es weiß. Ein Start der App frischt es auf.

Aufgaben lassen sich **direkt im Widget abhaken**: eine Aufgabenzeile *ist* ein
Kontrollkästchen – die ganze Zeile, kein separater Knopf daneben. VoiceOver liest
sie als einen Eintrag, der mit dem Kontrollkästchen und seinem Zustand endet – was
die Zeile ist und was man mit ihr tun kann, kommen also in einem Wisch. Die Zeile
verschwindet, sobald du sie abhakst.

Ein Tipp bewirkt genau das, was ein Tipp in der App bewirkt – auch im
**durchlaufenden** Abhak-Modus: dort schaltet ein Tipp eine Aufgabe von offen auf
in Arbeit, der nächste schließt sie ab. Das entscheidet nicht das Widget, es bittet
um ein Abhaken und die App wendet deine Einstellung an. In welchem Zustand eine
Aufgabe ist, steht an der Zeile selbst: ein leerer Kreis für offen, ein halb
gefüllter für in Arbeit – und das Wort wird zusätzlich vorgelesen, damit keines
der beiden Signale allein steht.

Der Haken wird sofort vermerkt und von der App ausgeführt, sobald sie das
nächste Mal läuft – beim Öffnen oder bei einer Hintergrund-Synchronisation. Das
ist keine Verzögerung, um die du dich kümmern musst: eine Aufgabe abzuschließen
wirkt in Aperio auf Eltern- und Unteraufgaben, schaltet eine Wiederholung weiter
und stellt eine Synchronisation in die Warteschlange – und ein Widget hat weder
den Speicher noch den Zugriff, um das selbst zu tun. Termine haben keinen Knopf,
und künftige Vorkommen einer wiederkehrenden Aufgabe auch nicht: abgehakt wird
das aktuelle, denn das schaltet die Reihe weiter.

Kalender, die du auf diesem Gerät ausgeblendet hast, bleiben auch im Widget
ausgeblendet. Aufgaben kommen aus **allen** Listen, nicht nur aus den gerade in
der Aufgabenansicht ausgewählten – aber nie eine, die jemand anderem zugewiesen
ist. Eine Aufgabe auf einer geteilten Liste, die den Namen einer anderen Person
trägt und nicht deinen, ist deren Arbeit; das Widget lässt sie genauso weg wie
die Kalenderansichten. Nicht zugewiesene Aufgaben und solche, die unter anderem
dir zugewiesen sind, bleiben.

## Widgets auf dem Sperrbildschirm (iOS)

Beide Widgets lassen sich auf den Sperrbildschirm legen, unter die Uhr:
Sperrbildschirm gedrückt halten, **Anpassen** wählen, dann den Bereich unter der
Uhrzeit.

**Als Nächstes** – dieselbe Liste wie auf dem Startbildschirm, gekürzt: drei
Zeilen, jede einzeilig, jede genauso ein Kontrollkästchen. Eine Aufgabe lässt sich
also direkt vom Sperrbildschirm aus erledigen, ohne das Telefon zu entsperren
oder etwas zu öffnen.

**Nächster Termin** ist das andere: eine einzelne Zeile, die sagt, was als
Nächstes ansteht und in wie langer Zeit – „in 25 Minuten", und
sobald es begonnen hat, „Läuft bis 11:00". Es zeigt **nur Einträge mit Uhrzeit**. Ganztägige bleiben bewusst außen vor: es
gibt keinen Moment, auf den heruntergezählt werden könnte, und ein langer –
etwa ein zweiwöchiger Urlaub – würde sonst zwei Wochen lang auf „was steht als
Nächstes an" mit „Urlaub" antworten, quer durch jeden Termin, den du trotzdem
einhalten musst. Steht nichts mit Uhrzeit an, heißt es **„Nichts mit Uhrzeit."** –
was etwas anderes behauptet als „nichts geplant".

Es liest dieselbe Übersicht wie das Widget auf dem Startbildschirm, braucht also
keine eigene Einrichtung. VoiceOver liest es als einen Satz, der damit endet, ob
es sich um einen Termin oder eine Aufgabe handelt – etwas, das weder das Symbol
noch eine Farbe für sich allein sagen kann.

Der Countdown selbst wird nicht mitgesprochen, während er läuft. Eine Zahl, die
sich jede Sekunde ändert, würde ununterbrochen unterbrechen; gesprochen wird die
grobe Form, die du beim Fokussieren hörst.

## Widget auf dem Startbildschirm (Android)

**Als Nächstes** gibt es auch unter Android, auf dem Startbildschirm: freie
Stelle gedrückt halten, **Widgets** wählen, **Aperio** suchen. Es liest dieselbe
Übersicht und zeigt dieselben Zeilen, und eine Aufgabenzeile ist auch dort ein
Kontrollkästchen – TalkBack kündigt sie als solches an, mit ihrem Zustand.

Widgets auf dem Sperrbildschirm gibt es unter Android nicht (sie wurden nach
Android 11 entfernt), das Countdown-Widget hat dort also kein Gegenstück. Alles
Übrige – ausgeblendete Kalender, Aufgaben aus allen Listen, was ein Tipp im
jeweiligen Abhak-Modus bewirkt – verhält sich genau wie unter iOS.
