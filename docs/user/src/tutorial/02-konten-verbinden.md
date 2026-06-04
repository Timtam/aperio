# 02 – Kalender und Aufgabenlisten verbinden

Aperio zeigt mehrere Quellen gleichzeitig an. In diesem Kapitel verbindest
du Konten und legst bei Bedarf lokale Kalender an.

## Ein Konto hinzufügen

1. Öffne die **Einstellungen** und gehe zum Bereich **Konten**.
2. Wähle **Konto hinzufügen** und den passenden Anbieter.
3. Folge dem anbieter­spezifischen Ablauf (siehe unten).

## Die Anbieter im Überblick

| Anbieter | Anmeldung | Was du bekommst |
|---|---|---|
| **Google** | Anmeldung im Browser (OAuth) | Kalender, Aufgaben, Kontakte |
| **iCloud / CalDAV** | Benutzername + **app-spezifisches Passwort** | Kalender, Aufgaben, Kontakte |
| **Outlook / Microsoft 365** | Anmeldung im Browser (OAuth) | Kalender, Aufgaben, Kontakte |
| **Exchange (EWS)** | Benutzername + Passwort | Kalender, Aufgaben, Kontakte |
| **Vikunja** | API-Token + Server-Adresse | nur Aufgaben |
| **Todoist** | API-Token | nur Aufgaben |

> **iCloud:** Apple verlangt ein **app-spezifisches Passwort** (in deinem
> Apple-Konto unter „Anmeldung & Sicherheit" zu erstellen), **nicht** dein
> normales Apple-Passwort.

> **Nur Kontakte (CardDAV):** Auch ein reiner CardDAV-Server ohne Kalender
> (z. B. Synology Contacts) lässt sich unter **iCloud / CalDAV** hinzufügen –
> gib einfach dessen Server-Adresse an. Aperio erkennt automatisch, dass es
> nur Kontakte gibt, und lässt die Kalender-/Aufgaben-Bereiche leer.

> **Vikunja / Todoist:** Den API-Token erzeugst du in den Entwickler- bzw.
> Integrations-Einstellungen des jeweiligen Dienstes und fügst ihn hier
> ein.

## Einen lokalen Kalender anlegen

Du brauchst kein Konto, um loszulegen. Lokale Kalender und Aufgabenlisten
liegen nur auf deinem Gerät (und werden – wenn eingerichtet – über deine
eigene [Synchronisation](08-synchronisation.md) abgeglichen):

1. Klicke unten in der **Seitenleiste** auf die passende
   Anlege-Schaltfläche: **+ Neuer Kalender**, **+ Neue Aufgabenliste** oder
   **+ Neues Kontaktbuch**.
2. Gib im Dialog einen **Namen** und optional eine **Farbe** (Farb-Label)
   ein und bestätige.

> **Farben kommen aus den Farb-Labels:** Die Farbe eines Kalenders oder
> einer Liste ist an ein Farb-Label gebunden. Färbst du ein Label später
> um, ändern sich alle daran gebundenen Kalender mit. Farb-Labels
> verwaltest du in den Einstellungen.

## Mehrere Konten verwalten

Du kannst beliebig viele Konten gleichzeitig verbinden – auch mehrere vom
selben Anbieter (z. B. zwei Google-Konten). Jede Quelle erscheint in der
Seitenleiste mit eigenem Namen und eigener Farbe und lässt sich einzeln
ein- und ausblenden.

> **Screenreader-Hinweis:** In der Seitenleiste sind Konten, Kategorien
> (Kalender / Aufgaben / Kontakte) und die einzelnen Listen als Baum
> angeordnet. Mit den Pfeiltasten nach oben/unten bewegst du dich, mit
> links/rechts klappst du Ebenen auf und zu. Das Kontextmenü (Umbenennen,
> Farbe, Mitglieder, Löschen) erreichst du mit der Anwendungstaste. Die
> Anlege-Schaltflächen (**+ Neuer Kalender** usw.) liegen unterhalb des
> Baums und sind mit `Tab` erreichbar.

## Zusammenfassung

Du hast Konten verbunden und/oder lokale Listen angelegt. Jetzt legen wir
Termine an.
