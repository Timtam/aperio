---
title: "07 – Benachrichtigungen"
---

In diesem Kapitel richtest du Erinnerungen für Termine und Aufgaben ein und
lernst, wie du auf sie reagierst.

## Eine Erinnerung hinzufügen

1. Öffne einen Termin oder eine Aufgabe zum Bearbeiten.
2. Im Feld **Erinnerung** wählst du, wie lange vorher du benachrichtigt
   werden möchtest (z. B. 10 Minuten, 1 Stunde, 1 Tag).
3. Du kannst mehrere Erinnerungen pro Eintrag hinzufügen.
4. **Speichern**.

## Wie Erinnerungen erscheinen

Zur fälligen Zeit zeigt Aperio eine Benachrichtigung an – als
System-Mitteilung und/oder als Hinweis in der App, je nach deinen
Einstellungen. Optional wird ein **Ton** abgespielt.

In der Benachrichtigung kannst du:

- den Eintrag **öffnen**,
- die Erinnerung **schließen** (bestätigen),
- oder **später erinnern** (Snooze) – die Erinnerung kommt nach der
  gewählten Zeit erneut.

## Standard-Erinnerungen pro Kalender

Neben den Erinnerungen an einzelnen Einträgen kannst du **Standard-Erinnerungen
pro Kalender** festlegen – nützlich etwa für abonnierte oder externe Kalender,
deren Einträge ohne Alarm ankommen. Wo ein Eintrag gilt, wählst du selbst, und
das entscheidet, ob er zusätzlich zu den eigenen Erinnerungen eines Termins
feuert oder für sie einspringt (siehe unten). Unter **Einstellungen → Kalender**
wählst du einen Kalender und legst dort seine Standard-Erinnerungen fest.

Jede Standard-Erinnerung sagt außerdem, wo sie gilt. **Nur in Aperio** ist die
Einstellung, mit der jeder Eintrag beginnt: Aperio erinnert selbst, in den
Termin wird nichts geschrieben – genau so behandelt die iOS-Kalender-App ihre
eigenen „Standardhinweise". **Neuen Terminen angehängt** schreibt diesen Eintrag
stattdessen in jeden Termin, den du in diesem Kalender anlegst und ohne eigene
Erinnerungen lässt, als dessen eigene Erinnerung. Erst dadurch erinnern auch
andere Apps, die denselben Kalender lesen – die iOS-Kalender-App oder ein
Sprachassistent, der deinen iCloud-Kalender vorliest. Hineingeschrieben wird er
nur in Termine, die du *nach* dem Umschalten anlegst; ein Termin mit eigenen
Erinnerungen oder bewusst ohne behält diese Wahl, und ein Termin ohne eigene
Erinnerungen bekommt den Eintrag in Aperio weiterhin. Exchange- und
Outlook-Kalender speichern eine einzige „Minuten vorher"-Erinnerung; dort wird
nur der erste solche Eintrag angehängt, eine feste Uhrzeit kommt in diesen
Terminen nicht an.

Eine Standard-Erinnerung kann ein Vorlauf sein („Vor Beginn") oder ein fester
Zeitpunkt. „Beim nächsten App-Start" fehlt hier mit Absicht: Diese Art wird nur
ausgelöst, wenn sie am Eintrag selbst gesetzt ist – als Kalender-Standard wäre
sie eine Einstellung, die sich speichern lässt und dann stumm bleibt.

### Ganztägige Termine und Geburtstage

Ganztägige Termine haben keine Uhrzeit, daher wird eine Erinnerung nicht „eine
Stunde vorher" (also am Vorabend) ausgelöst, sondern **zum Tageswechsel** – zur
selben Zeit wie deine Tages-Erinnerungen. Ein Vorlauf zählt dabei in ganzen
**Tagen**: „1 Woche vorher" feuert sieben Tage früher zum Tageswechsel.

Das gilt auch für die automatisch erzeugten **Geburtstagskalender** — und die
erinnern **am Tag selbst**, zur Tageswechsel-Zeit, ohne dass du etwas einrichten
musst. Ein Geburtstagskalender existiert, weil du erinnert werden willst, also
ist er von Anfang an eingeschaltet.

Unter **Einstellungen → Kalender** (auf dem Handy über **Erinnerungen** in der
Kalenderliste) änderst du das: eine Vorlaufzeit wie „1 Woche vorher", mehrere
Erinnerungen, oder alle entfernen — eine geleerte Liste bleibt leer und der
Kalender schweigt.

### Abgesagte Termine

Abgesagte Termine (z. B. eine von der Organisatorin zurückgezogene
Besprechungsserie) lösen **nie** Erinnerungen aus – unabhängig von jeder
Einstellung. Sichtbar bleiben sie standardmäßig im Kalender (wie in Outlook);
unter **Einstellungen → Allgemein** kannst du sie über **Abgesagte Termine
anzeigen** ausblenden. Wird ein abgesagter Termin angezeigt, ist er gedimmt und
sein Titel durchgestrichen, und Screenreader kündigen ihn mit einem
nachgestellten „abgesagt“ an. Löschst du eine einzelne Wiederholung eines
Serientermins, verschwindet diese Wiederholung vollständig – sie bleibt nicht
als abgesagte Zeile stehen.

## Benachrichtigungstöne

Du kannst festlegen, welchen Ton eine Erinnerung abspielt – auf mehreren
Ebenen, wobei jede die darüberliegende überschreibt:

1. **Globaler Standard** – Einstellungen → Kalender → *Benachrichtigungstöne*.
2. **Pro Kalender / pro Aufgabenliste** – im selben Einstellungsbereich, in
   der jeweiligen Kalender- bzw. Listenzeile.
3. **Pro Termin / pro Aufgabe** – im Termin- bzw. Aufgabendialog (beim
   Bearbeiten eines bestehenden Eintrags).
4. **Pro Erinnerung** – direkt an einer einzelnen Erinnerungszeile.

Jede Ebene bietet dieselben Optionen:

- **Systemstandard** – der Benachrichtigungston deines Betriebssystems.
- **Kein Ton** – eine rein visuelle Benachrichtigung, ohne Ton.
- **Eigener Ton** – importiere eine eigene Audiodatei (`.mp3`, `.ogg`,
  `.wav`, `.m4a`, `.aac`, `.flac`, bis 5 MB). Mit **Testen** hörst du sie
  probeweise, mit **Entfernen** löschst du einen importierten Ton.

Alles unterhalb der globalen Ebene bietet zusätzlich **Standard verwenden**
– das bedeutet „die darüberliegende Ebene erben". Importierte Töne und deine
Auswahl werden mit deinen anderen Geräten synchronisiert (die Audiodatei
reist mit der Einstellung), sodass eine Erinnerung überall gleich klingt.

> **Lautstärke:** Aperio hat bewusst keinen eigenen Lautstärkeregler – nutze
> den App-Lautstärkemixer deines Betriebssystems (Windows und macOS haben
> beide einen).

## Einstellungen für Benachrichtigungen

In den **Einstellungen** unter **Benachrichtigungen** legst du fest:

- ob System-Benachrichtigungen verwendet werden,
- ob und welcher **Ton** abgespielt wird (siehe *Benachrichtigungstöne*
  oben),
- Standard-Snooze-Dauer,
- Standard-Vorlaufzeit für neue Termine.

> **Hinweis:** Damit System-Benachrichtigungen erscheinen, muss Aperio die
> entsprechende Berechtigung deines Betriebssystems haben. Beim ersten Mal
> wirst du danach gefragt.

> **Screenreader-Hinweis:** Erinnerungen werden über eine Live-Region
> angesagt, sodass du sie auch ohne sichtbaren Fokuswechsel mitbekommst.
> Die Schaltflächen „Öffnen", „Schließen" und „Später erinnern" sind per
> `Tab` erreichbar und klar beschriftet.

## Im Hintergrund laufen (Infobereich)

Damit Erinnerungen auch dann feuern, wenn du das Fenster nicht offen hast,
kann Aperio in den **Infobereich** (System Tray) ausgeblendet werden, statt
sich zu beenden. Unter **Einstellungen → Allgemein**:

- **Beim Schließen in den Infobereich minimieren** – der Schließen-Knopf
  blendet Aperio aus, statt zu beenden.
- **Beim Minimieren in den Infobereich** – der Minimieren-Knopf legt das
  Fenster in den Infobereich statt in die Taskleiste.

Ein Klick auf das Infobereich-Symbol holt das Fenster zurück; über dessen
Menü kannst du Aperio wirklich beenden. Beide Optionen sind standardmäßig
aus. Hat dein System keinen Infobereich (z. B. GNOME ohne
AppIndicator-Erweiterung), sind die Schalter deaktiviert und das Fenster
verhält sich normal.

## Bei der Anmeldung starten

Damit Erinnerungen auch nach einem Neustart wieder ausgelöst werden, ohne dass
du Aperio von Hand öffnest, kannst du unter **Einstellungen → Allgemein →
Systemstart** die Option **„Aperio bei der Anmeldung starten"** aktivieren.
Aperio startet dann automatisch, sobald du dich an diesem Computer anmeldest.
Eine zweite Option, **„Minimiert im Infobereich starten"** (standardmäßig an,
sofern ein Infobereich vorhanden ist), steuert, ob der Autostart ein Fenster
öffnet oder direkt im Infobereich startet — ein Klick auf das Symbol holt es
hervor. Die Einstellungen gelten nur für dieses Gerät; durch Entfernen des
Häkchens wird der Autostart wieder abgeschaltet.

## Zusammenfassung

Du kannst Erinnerungen anlegen, ihre Töne einstellen und auf sie mit
Schließen oder Snooze reagieren. Weiter geht es mit der Suche.
