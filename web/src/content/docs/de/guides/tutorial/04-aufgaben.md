---
title: "04 – Aufgaben"
---

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
  abhaken (erneut drücken hebt es wieder auf). Das **Erledigungsdatum**
  wird festgehalten und angezeigt – in der Aufgabenliste zeigt eine
  erledigte Aufgabe *„Erledigt: <Datum>“* anstelle ihres Fälligkeitsdatums,
  im Editor erscheint eine Zeile *„Erledigt am“*. Es wird mit jedem Anbieter
  in beide Richtungen synchronisiert (Apple Erinnerungen, Google, Vikunja,
  Microsoft To Do, Exchange, Todoist).
- **Bearbeiten:** mit `Eingabe` öffnen.
- **Löschen:** **Löschen** wählen (Standard: `Entf`).

> **Status & Priorität schnell ändern:** Rechtsklick auf eine Aufgabe – oder
> die Menü-Taste (`Umschalt+F10`) – öffnet ein Kontextmenü mit den
> Untermenüs **Status** und **Priorität**. Beides lässt sich per Klick
> ändern, ohne den Editor zu öffnen; der aktuelle Wert ist mit einem Häkchen
> markiert.

> **Priorität & Anbieter:** Alle Anbieter speichern die Priorität einer
> Aufgabe – außer **Google Tasks**, das kein eigenes Prioritätsfeld hat: Eine
> Priorität, die du an einer Google-Aufgabe setzt, wird auf Google-Seite nicht
> gespeichert und erscheint nach der nächsten Synchronisation wieder als
> *Mittel*. Lokale Listen, Apple Erinnerungen / CalDAV, Exchange, Microsoft
> To Do, Vikunja und Todoist behalten sie.

> **„In Arbeit" & Anbieter:** Den Status **In Arbeit** speichern nur Anbieter
> mit einem echten Zwischenstatus: **lokale** Listen, **Apple Erinnerungen /
> CalDAV** (`IN-PROCESS`), **Exchange** und **Microsoft To Do**. **Google
> Tasks, Vikunja und Todoist** kennen nur *offen / erledigt* – setzt du dort
> eine Aufgabe auf *In Arbeit*, fällt sie beim nächsten Abgleich wieder auf
> *Offen* zurück. Bei diesen Anbietern unterbleibt deshalb auch das
> automatische **Einplanen auf heute**, das eine begonnene Aufgabe sonst
> auslöst (sie ließe sich ja gar nicht als „begonnen" merken). Das **manuelle**
> Einplanen – eine Aufgabe aus dem Backlog auf einen Tag ziehen oder per
> Plan-Dialog (`Umschalt+D`) ein Datum setzen – funktioniert unverändert. Im
> **Status-Zyklus** (siehe *Abhaken-Verhalten*) überspringt das Abhaken bei
> diesen Anbietern den Schritt *In Arbeit*: der Zyklus läuft *Offen →
> Erledigt → Offen*, damit die `Leertaste` nicht bei *Offen* hängen bleibt.

> **Abhaken-Verhalten:** Standardmäßig wechselt das Abhaken zwischen *offen*
> und *abgeschlossen*. Unter **Einstellungen → Aufgaben → Abhaken-Verhalten**
> kannst du stattdessen einen Status-Zyklus wählen: Jedes Abhaken durchläuft
> *Offen → In Arbeit → Abgeschlossen → Offen*, sodass der Status „In Arbeit"
> per `Leertaste` oder Klick erreichbar ist, ohne den Editor zu öffnen.

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
> bei **Todoist** und **Vikunja** kannst du Abschnitte an drei Stellen
> anlegen, umbenennen und löschen — je nachdem, was gerade am nächsten
> liegt: über das **⋮-Menü** am Abschnitts-Kopf (oder Rechtsklick /
> `Umschalt+F10`) in der Aufgaben-Ansicht, über das **Kontextmenü einer
> Aufgabenliste in der Seitenleiste** (*Abschnitt hinzufügen*) oder über das
> Feld **Abschnitt** im Aufgaben-Dialog. Die Änderung läuft beim jeweiligen
> Anbieter. Die **Farbe** eines Abschnitts bleibt dabei immer lokal: Sie
> lässt sich für jeden Abschnitt setzen (auch bei Todoist/Vikunja, die
> selbst keine Abschnittsfarbe kennen) und wird nicht zum Anbieter
> übertragen.

> **Aufgaben zwischen Abschnitten verschieben:** Über das **Abschnitt**-Feld
> im Aufgaben-Dialog ordnest du eine Aufgabe einem anderen Abschnitt zu
> oder löst sie mit **Kein Abschnitt** ganz heraus. Das funktioniert bei
> lokalen Listen, bei Todoist und bei Vikunja (ab 0.24); bei „Kein
> Abschnitt" landet eine Vikunja-Aufgabe im Standard-Bucket, da Vikunja
> jede Kanban-Aufgabe in einem Bucket führt.

> **Wo Abschnitte erscheinen:** In der **Aufgaben-Ansicht** erscheinen Liste
> und Abschnitt als **eigene, anspringbare Zeilen** der Baumansicht
> (Backlog → Liste → Abschnitt) — auch innerhalb der **Backlog**-Gruppe,
> sodass die Buckets einer Liste (z. B. *To-Do / Doing / Done* eines
> Vikunja-Projekts) auch ohne eingeplanten Tag sichtbar sind. Jede
> Gruppenzeile ist mit den Pfeiltasten erreichbar; `Eingabe`/`Leertaste`
> (oder Pfeil-rechts/-links) klappt sie auf bzw. zu — genau wie die
> **Erledigt**-Gruppe oder eine Aufgabe mit Unteraufgaben. Ein Abschnitt ist
> nur eine Gruppierung; der **Status** einer Aufgabe (offen / erledigt) ist
> unabhängig davon, in welchem Abschnitt sie liegt.

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

> **Wiederholung & Anbieter:** Ob – und wie viel – von einer Wiederholung
> gespeichert werden kann, hängt vom Anbieter ab. Der Editor erscheint nur
> dort, wo die Liste sie auch speichern kann, und **blendet einzelne Felder
> aus, die der Anbieter nicht kann** (statt sie beim Speichern still zu
> verwerfen). **Lokale** Listen und **Microsoft To Do** unterstützen die
> volle Wiederholung. **Vikunja** speichert einfache Wiederholungen –
> *täglich*/*wöchentlich* (mit Intervall, z. B. „alle 2 Wochen") und
> *monatlich* –, kennt aber kein *jährlich*, keine **Wochentagsauswahl**,
> keinen festen **Tag im Monat** (es wiederholt am Tag des Fälligkeitsdatums)
> und kein **Enddatum**; diese Felder sind dort ausgegraut. Bei **Google
> Tasks**, **Todoist**, **Apple Erinnerungen / CalDAV** und **Exchange** wird
> der Wiederholungs-Editor **gar nicht** angezeigt – diese Anbieter speichern
> (noch) keine Aufgaben-Wiederholung.

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
> offen), Fälligkeit, Priorität und – sofern jemand zugewiesen ist – der
> **zugewiesenen Person** angesagt (in **allen** Ansichten: Aufgaben-,
> Wochen-, Tages- und Monatsansicht sowie im Backlog). Eine **hohe**
> Priorität zeigt zusätzlich ein „↑", eine **niedrige** ein „↓"; die mittlere
> Priorität (Standard) zeigt nichts. **Liste und Abschnitt** werden nicht mehr im
> Aufgaben-Label wiederholt, sondern ergeben sich aus den übergeordneten
> **Gruppenzeilen** (Backlog → Liste → Abschnitt), die du beim Navigieren mit
> den Pfeiltasten durchläufst und mit `Eingabe`/`Leertaste` auf- und zuklappst.
> Das Abhaken über die `Leertaste` wird sofort als „erledigt" bzw. „offen"
> rückgemeldet, ohne dass sich der Fokus bewegt.

## Zusammenfassung

Du kannst Aufgaben anlegen, abhaken, einplanen, wiederholen lassen und in
geteilten Listen zuweisen. Im nächsten Kapitel lernst du die Ansichten
kennen.
