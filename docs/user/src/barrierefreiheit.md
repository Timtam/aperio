# Barrierefreiheit

Aperio wurde von Grund auf so entwickelt, dass es **mit einem Screenreader
vollständig nutzbar** ist. Diese Seite erklärt die wichtigsten Konzepte und
gibt Tipps für die gängigen Screenreader.

## Das Grundprinzip: Anwendungsmodus

Aperio läuft im **Anwendungsmodus** (`role="application"`). Das bedeutet:

- Du musst **nicht** in den Lesemodus (Browse-/Virtual-Modus) wechseln.
- Deine **Pfeiltasten** steuern direkt den Kalender und die Listen – nicht
  den virtuellen Cursor des Screenreaders.
- Tastenanschläge kommen direkt bei Aperio an, sodass alle Kürzel
  funktionieren.

Das ist anders als bei klassischen Webseiten und ermöglicht ein flüssiges,
app-artiges Arbeiten.

## Live-Ansagen

Wichtige Statusänderungen werden über **Live-Regionen** angesagt, ohne dass
sich der Fokus bewegt, zum Beispiel:

- „Termin gespeichert", „Aufgabe erledigt",
- der Wechsel der Ansicht und der fokussierte Zeitraum,
- der Status der Synchronisation,
- fällige Erinnerungen.

## Tipps nach Screenreader

### NVDA

- Aperio aktiviert den Fokusmodus automatisch (Anwendungsmodus). Solltest du
  versehentlich im Lesemodus landen, bringt dich `NVDA+Leertaste` zurück.
- Mit `NVDA+Tab` lässt du dir das aktuell fokussierte Element erneut ansagen.

### JAWS

- Auch hier sorgt der Anwendungsmodus dafür, dass die Pfeiltasten direkt an
  Aperio gehen. Bei Bedarf den Formularmodus mit `Eingabe` erzwingen.
- `Einfügen+Tab` wiederholt die Ansage des Fokus.

### VoiceOver (macOS)

- Mit aktiviertem VoiceOver navigierst du Aperio über die Pfeiltasten; das
  „Schnellnavigation"-Verhalten ist im Anwendungsbereich nicht nötig.
- `VO+F` lässt dich nach Bedienelementen suchen.

### Narrator (Windows)

- Der Scan-Modus ist in Aperio nicht erforderlich; falls aktiv, schaltest du
  ihn mit `Feststelltaste+Leertaste` um.

## Was du erwarten kannst

- **Alle** Funktionen sind ohne Maus erreichbar.
- Schaltflächen, Menüs, Dialoge und Listen sind korrekt ausgezeichnet und
  beschriftet.
- Optische Kürzungen (z. B. abgeschnittene Termin-Titel in der
  Monatsansicht) ändern **nichts** an der vorgelesenen Information – du
  bekommst immer den vollständigen Text.

## Rückmeldung

Findest du eine Stelle, die sich mit deinem Screenreader nicht gut bedienen
lässt, freuen wir uns über einen Hinweis im
[Projekt auf GitHub](https://github.com/Timtam/aperio). Barrierefreiheit ist
ein zentrales Ziel von Aperio.
