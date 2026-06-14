---
title: "08 – Synchronisation"
---

Konten wie Google oder iCloud werden direkt mit dem jeweiligen Anbieter
abgeglichen. **Lokale** Kalender und Aufgabenlisten kannst du zusätzlich
über deinen **eigenen Speicher** zwischen mehreren Geräten synchronisieren.
Darum geht es in diesem Kapitel.

## Wie die Synchronisation funktioniert

Aperio legt deine lokalen Daten als Änderungsprotokoll ab und gleicht sie
über einen Speicherort ab, den **du** bestimmst – zum Beispiel WebDAV,
Dropbox oder einen anderen Ordner, der zwischen deinen Geräten geteilt
wird. Es gibt keinen Aperio-Server; deine Daten bleiben bei dir.

## Synchronisation einrichten

1. Öffne die **Einstellungen** und gehe zu **Synchronisation**.
2. Wähle den **Speicher-Typ** (z. B. WebDAV oder ein lokaler/geteilter
   Ordner) und gib die Zugangsdaten bzw. den Pfad an.
3. **Speichern** – Aperio führt den ersten Abgleich durch.
4. Richte denselben Speicher auf deinem zweiten Gerät ein. Beide Geräte
   teilen sich nun denselben Stand.

## Abgleich und Konflikte

- Der Abgleich passiert automatisch im Hintergrund und bei Programmstart.
- Du kannst einen Abgleich auch **manuell** anstoßen.
- Bearbeiten zwei Geräte denselben Eintrag, erkennt Aperio den **Konflikt**
  und löst ihn nachvollziehbar auf bzw. fragt nach, welche Fassung gelten
  soll.

## Ende-zu-Ende-Verschlüsselung & Zugangsdaten

In den Synchronisations-Einstellungen kannst du **Ende-zu-Ende-Verschlüsselung**
mit einem Passwort aktivieren. Der Speicher sieht dann nur noch verschlüsselte
Daten – das Passwort verlässt nie dein Gerät, und ohne es lassen sich die Daten
nicht wiederherstellen (bewahre es also sicher auf).

Ist die Verschlüsselung aktiv, werden zusätzlich die **Zugangsdaten deiner
Konten** (Passwörter, Tokens) verschlüsselt mitsynchronisiert. So funktionieren
deine Konten auf jedem Gerät, **ohne** dass du die Zugangsdaten erneut eingeben
musst. **Ohne** Verschlüsselung bleiben Zugangsdaten ausschließlich lokal auf
dem jeweiligen Gerät. Schaltest du die Verschlüsselung wieder ab, werden sie aus
dem Sync-Speicher entfernt und nur noch lokal aufbewahrt.

> **Hinweis:** Externe Konten (Google, iCloud, Outlook, Vikunja, Todoist)
> brauchen diese Einrichtung **nicht** – sie synchronisieren über ihren
> eigenen Dienst. Die Synchronisation hier betrifft nur deine **lokalen**
> Kalender und Listen.

> **Screenreader-Hinweis:** Der Status des letzten Abgleichs (Zeitpunkt,
> Erfolg oder Fehler) steht in den Synchronisations-Einstellungen und wird
> bei Änderungen über eine Live-Region angesagt. Konfliktdialoge sind als
> Dialog ausgezeichnet und mit der Tastatur vollständig bedienbar.

## Zusammenfassung

Du kannst deine lokalen Daten über deinen eigenen Speicher zwischen Geräten
synchronisieren und weißt, wie Konflikte behandelt werden. Zum Abschluss
sehen wir uns die Tastaturkürzel an.
