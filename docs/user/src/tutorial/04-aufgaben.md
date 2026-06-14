# 04 – Aufgaben

In diesem Kapitel legst du Aufgaben an, ordnest sie Listen zu, planst sie in
deinen Tag oder deine Woche ein und arbeitest sie ab.

## Eine Aufgabe anlegen

1. Öffne die **Aufgaben-Ansicht** oder eine Aufgabenliste in der
   Seitenleiste.
2. Lege eine Aufgabe an: **Aufgabe schnell anlegen** (`Alt+N`) öffnet den
   Schnell-Dialog, **Neue Aufgabe** (`Alt+Umschalt+N`) das vollständige Formular.
3. Gib einen **Titel** ein. Optional:
   - **Aufgabenliste**, in der sie gespeichert wird,
   - **Fälligkeitsdatum** (und optional Uhrzeit),
   - **Priorität**,
   - **Beschreibung**,
   - **Farb-Label**,
   - bei kollaborativen Listen (z. B. Todoist) eine **zugewiesene Person**.

## Erledigen und bearbeiten

- **Erledigt markieren:** Aufgabe markieren und mit `Leertaste`
  abhaken (erneut drücken hebt es wieder auf).
- **Bearbeiten:** mit `Eingabe` öffnen.
- **Löschen:** **Löschen** wählen (Standard: `Entf`).

> **Erledigte ausblenden:** Abgehakte Aufgaben wandern in eine
> eingeklappte Gruppe **Erledigt (N)** ganz unten in der Liste, damit die
> offenen Aufgaben übersichtlich bleiben. Die Gruppe zeigt die Anzahl an.
> Sie ist eine normale Zeile in der Aufgaben-Baumansicht: mit den
> Pfeiltasten erreichbar, `Eingabe`/`Leertaste` (oder Pfeil-rechts/-links)
> klappt sie auf bzw. zu — genau wie eine Aufgabe mit Unteraufgaben. Der
> Auf-/Zu-Zustand wird gemerkt.

> **Abschnittsfarben:** Aufgaben-Abschnitte können eine eigene Farbe
> bekommen (im Aufgaben-Dialog beim Anlegen oder Bearbeiten eines
> Abschnitts — oder direkt am Abschnitts-Kopf in der Aufgaben-Ansicht per
> Rechtsklick bzw. der **⋮**-Schaltfläche). Aufgaben **ohne** eigene Farbe
> übernehmen die Farbe ihres Abschnitts; verschiebst du eine solche
> Aufgabe in einen anderen Abschnitt, färbt sie automatisch mit.
> Reihenfolge: eigene Aufgaben-Farbe → Abschnitt → Aufgabenliste.

> **Abschnitte anlegen, umbenennen, löschen:** Bei **lokalen** Listen sowie
> bei **Todoist** und **Vikunja** legst du Abschnitte direkt im
> Aufgaben-Dialog an, benennst sie um und löschst sie — die Änderung läuft
> beim jeweiligen Anbieter. Die **Farbe** eines Abschnitts bleibt dabei
> immer lokal: Sie lässt sich für jeden Abschnitt setzen (auch bei
> Todoist/Vikunja, die selbst keine Abschnittsfarbe kennen) und wird nicht
> zum Anbieter übertragen.

> **Aufgaben zwischen Abschnitten verschieben:** Über das **Abschnitt**-Feld
> im Aufgaben-Dialog ordnest du eine Aufgabe einem anderen Abschnitt zu
> oder löst sie mit **Kein Abschnitt** ganz heraus. Das funktioniert bei
> lokalen Listen, bei Todoist und bei Vikunja (ab 0.24); bei „Kein
> Abschnitt" landet eine Vikunja-Aufgabe im Standard-Bucket, da Vikunja
> jede Kanban-Aufgabe in einem Bucket führt.

> **Per Maus (Drag & Drop):** Du kannst eine Aufgabe auch auf einen
> **Abschnitts-Kopf** (in der Aufgaben-Ansicht) oder auf eine **Liste in
> der Seitenleiste** ziehen, um sie dorthin zu verschieben; einen **Termin**
> ziehst du auf einen **Kalender in der Seitenleiste**. Für Tastatur- und
> Screenreader-Nutzung bleibt der Verschieben/Kopieren-Dialog (bzw. das
> Abschnitt-Feld) der Weg.

## Aufgaben einplanen

Aperio unterscheidet zwischen Aufgaben mit und ohne festen Termin:

- **Backlog:** Aufgaben ohne **geplanten Tag** sammeln sich hier – auch
  solche mit Deadline, aber ohne festen Bearbeitungstag.
- **Einplanen:** Gib einer Aufgabe ein Fälligkeitsdatum (oder ziehe sie in
  der Wochenplanung auf einen Tag), um sie fest einzuplanen.
- **Automatisch auf heute:** Setzt du eine Backlog-Aufgabe auf **„In
  Bearbeitung"**, plant Aperio sie automatisch für **heute** ein – die Arbeit
  hat ja begonnen. (Abschaltbar unter *Einstellungen → Aufgaben*.)
- **Backlog-Liste:** Die **Wochen- und die Monatsansicht** zeigen **links
  neben dem Raster** eine feste **Backlog**-Liste mit allen ungeplanten
  Aufgaben. Zieh eine Aufgabe von dort auf einen Tag, um sie einzuplanen –
  oder zieh eine eingeplante Aufgabe zurück auf die Liste, um sie wieder in
  den Backlog zu legen. Ohne Maus: Aufgabe in der Liste fokussieren und
  **Umschalt+D** (Plan-Dialog) drücken bzw. das Kontextmenü nutzen. Die
  **Breite** der Liste lässt sich durch Ziehen ihrer rechten Kante anpassen
  (wird gespeichert) – die Ansicht daneben passt sich entsprechend an.
- **Deadline:** Eine Aufgabe mit Deadline erscheint in der Wochen- und
  Tagesplanung als **Fälligkeitsmarker** an ihrem Deadline-Tag („fällig
  bis …") – als einzelner Punkt, nicht als Balken über alle Tage bis dahin.
  Solange kein Arbeitstag gesetzt ist, liegt sie **zusätzlich im Backlog**,
  damit du sie auf einen konkreten Arbeitstag ziehen kannst.

## Wiederkehrende Aufgaben

Wie Termine können auch Aufgaben sich wiederholen (täglich, wöchentlich,
monatlich, jährlich). Hakst du eine wiederkehrende Aufgabe ab, erzeugt
Aperio automatisch die nächste Fälligkeit.

## Mitglieder und Zuweisungen

Bei geteilten Listen (z. B. Todoist) kannst du:

- über das Kontextmenü der Liste **Mitglieder verwalten** (einladen,
  entfernen),
- einzelnen Aufgaben eine **Person zuweisen**.

> **Vikunja – Personen finden:** Im Dialog **Mitglieder verwalten** suchst
> du nach Personen. Vikunja findet einen **Benutzernamen nur exakt** (der
> vollständige Name, Groß-/Kleinschreibung egal) – ein Teil davon genügt
> nicht. Teiltreffer funktionieren nur beim **Anzeigenamen**, die **E-Mail**
> muss wiederum exakt sein. Die gesuchte Person muss sich außerdem in ihren
> Vikunja-Einstellungen als **auffindbar** markiert haben (per Name bzw. per
> E-Mail). Eine Liste „aller Nutzer" bietet Vikunja bewusst nicht an.
> Berechtigungen (Lesen / Schreiben / Admin) änderst du danach direkt in der
> Mitgliederliste.

> **Screenreader-Hinweis:** Aufgaben werden mit Titel, Status (erledigt /
> offen), Fälligkeit und Priorität angesagt. Eine **hohe** Priorität zeigt
> zusätzlich ein „↑", eine **niedrige** ein „↓"; die mittlere Priorität
> (Standard) zeigt nichts. Das Abhaken über die `Leertaste` wird sofort als
> „erledigt" bzw. „offen" rückgemeldet, ohne dass sich der Fokus bewegt.

## Zusammenfassung

Du kannst Aufgaben anlegen, abhaken, einplanen, wiederholen lassen und in
geteilten Listen zuweisen. Im nächsten Kapitel lernst du die Ansichten
kennen.
