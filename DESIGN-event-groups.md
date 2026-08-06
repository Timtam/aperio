# Termingruppen — Entwurf

Status: ENTWURF, nichts davon gebaut. Zweck dieses Dokuments ist zu entscheiden,
ob und in welcher Form das gebaut wird — nicht, wie man es baut.

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

Offen bleibt der Fall, dass ein Anbieter ein Mitglied verschiebt und die Kopien
auseinanderlaufen. Dann ist die Gruppe eine Behauptung, die nicht mehr stimmt.
Sie muss das anzeigen können statt es zu verschweigen.

### Rechte

Ein Kollegenkalender ist meist nur lesbar. „Alle mitziehen" muss deshalb sagen,
welche Mitglieder es NICHT konnte, statt sie stillschweigend zu überspringen —
sonst entsteht wieder genau der Widerspruch, den die Gruppe verhindern sollte.

## Vorgeschlagene Stufen

Jede Stufe ist für sich nützlich und einzeln testbar.

**Stufe 0** — Modell, Speicherung, Synchronisation. Eine Aktion „gehören
zusammen" auf zwei ausgewählten Terminen. Keine weitere Oberfläche.

**Stufe 1** — Zusammenfalten in den Ansichten. Eine Zeile, die ihre Kalender
nennt. Der Alltagsgewinn.

**Stufe 2** — Bearbeitungsumfang über die Mitglieder, mit ehrlicher Meldung über
das, was nicht ging.

**Stufe 3** — Erkennung und Vorschlag; das Meeting wandert an die Gruppe, die
heutige Verknüpfungs-Rückfrage entfällt.

## Größenordnung

Vergleichbar mit den Widgets: ein eigenes Vorhaben über Kern, Synchronisation und
beide Oberflächen, kein Nebenbei. Stufe 0 und 1 zusammen sind der ehrliche
Einstieg; alles danach ist optional und lässt sich am Gebrauch entscheiden.
