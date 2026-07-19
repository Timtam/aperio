---
title: "Fehlersuche & Protokolle"
---

Wenn etwas nicht wie erwartet funktioniert, sind Aperios Protokolle der
schnellste Weg zur Ursache. Aperio führt auf deinem Gerät eine rollierende
Protokolldatei — auch im normalen (Release-)Build — die du exportieren und
einem Fehlerbericht beilegen kannst.

## Der Protokolle-Bereich

Öffne **Einstellungen → Protokolle**. Dort kannst du:

- **Den Detailgrad einstellen.** *Normal* ist die Voreinstellung und für den
  Alltag richtig. Wechsle nur zu *Debug* oder *Trace*, während du ein Problem
  nachstellst — sie protokollieren deutlich mehr und machen das Protokoll
  umfangreicher. Die Auswahl wird auf diesem Gerät gemerkt und **nicht** auf
  deine anderen Geräte synchronisiert.
- **Das aktuelle Protokoll ansehen** — die letzten Zeilen der aktuellen
  Protokolldatei, mit einer **Aktualisieren**-Schaltfläche.
- **Das Protokoll in eine Datei exportieren** — Speicherort wählen und dem
  Bericht beilegen.
- **Das Protokoll in die Zwischenablage kopieren** — praktisch zum Einfügen in
  ein Ticket oder einen Chat.
- **Protokolle löschen** — entfernt die gespeicherten Protokolldateien (die
  aktuelle Sitzung protokolliert weiter).

## Datenschutz

Der Export ist zum Teilen gedacht, daher ist **Persönliche Daten entfernen**
standardmäßig aktiv: E-Mail-Adressen und Zugriffstokens werden vor dem
Verlassen des Geräts durch Platzhalter ersetzt. Passwörter, die
Sync-Passphrase oder Konto-Tokens protokolliert Aperio ohnehin nie — die
liegen ausschließlich im Schlüsselbund deines Betriebssystems. Lass die
Schwärzung aktiviert, sofern der Support nicht ausdrücklich ein
ungeschwärztes Protokoll anfordert.

## Wo die Protokolle liegen

Die Protokolldateien liegen in deinem Datenverzeichnis im Ordner `logs/`
(`aperio.log.<Datum>`). Einstellungen → Protokolle zeigt den genauen Pfad mit
einer **Pfad kopieren**-Schaltfläche. Dateien, die älter als 14 Tage sind,
werden automatisch entfernt.

## Ein Konto aktualisiert sich nicht mehr

Wenn ein verbundenes Konto nicht mehr aktualisiert werden kann — meist, weil
das Passwort bzw. App-Passwort geändert oder widerrufen wurde — zeigt Aperio
weiter die zuletzt bekannten Daten und warnt dich, statt still zu scheitern:

- **Desktop:** Das Konto in der Seitenleiste trägt eine Warnung, und eine
  höfliche Screenreader-Ansage verweist auf **Einstellungen → Konten**. Dort
  listet das betroffene Konto jeden fehlschlagenden Kalender bzw. jede Liste,
  den Fehler des Anbieters und den Zeitpunkt der letzten erfolgreichen
  Aktualisierung. Deuten die Fehler auf ein Anmeldeproblem hin, öffnet eine
  Schaltfläche **Passwort neu eingeben** direkt den Verbinden-Dialog.
- **Mobil:** Die Sync-Schaltfläche in der Kopfzeile wird zur Warnung (ihre
  Beschriftung nennt das Problem), die Details stehen im Bereich
  **Synchronisierung**, und das betroffene Konto erhält auf dem
  Konten-Bildschirm eine Schaltfläche **Neu verbinden**, um das Passwort neu
  einzugeben bzw. die Anbieter-Anmeldung zu wiederholen.

Die Warnung verschwindet von selbst, sobald eine Aktualisierung wieder
gelingt.

## Einen Fehler melden

1. Stelle in Einstellungen → Protokolle die Stufe auf **Debug**.
2. Stelle das Problem nach.
3. **Exportiere** das Protokoll (oder kopiere es) und lege es deinem Bericht
   bei — zusammen mit dem, was du getan und was du erwartet hast.
