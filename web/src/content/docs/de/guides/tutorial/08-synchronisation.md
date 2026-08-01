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

Ein Speicherort ist in Aperio ein **Konto** wie jedes andere: Du legst ihn
unter **Einstellungen → Konten** an, und in der Synchronisation wählst du
nur noch aus, welches deiner Konten den Datensatz hält.

1. Öffne die **Einstellungen** und gehe zu **Konten**.
2. Lege ein Konto für deinen Speicherort an (z. B. WebDAV, SFTP, FTPS,
   Dropbox, Google Drive oder ein lokaler bzw. geteilter Ordner) und gib
   Adresse und Zugangsdaten an.
3. Wechsle auf **Synchronisation**. Unter **Synchronisationsziel** stehen
   genau die Konten, die einen Datensatz halten können.
4. Wähle das gewünschte Konto in der Liste und aktiviere
   **Über … synchronisieren**. Aperio prüft die Verbindung, bevor die Wahl
   gespeichert wird – schlägt sie fehl, bleibt alles wie vorher.
5. Richte denselben Speicher auf deinem zweiten Gerät ein. Beide Geräte
   teilen sich nun denselben Stand.

Beim **allerersten Start** fragt der Einrichtungsassistent den Speicherort
direkt ab – dort entscheidest du auch, ob ein vorhandener Datensatz
übernommen oder ein neuer angelegt wird.

Kann Aperio nach einem Neustart über das gewählte Konto nicht
synchronisieren – gesperrter Schlüsselbund, fehlende Zugangsdaten, ein auf
diesem Gerät nicht bestätigter Server-Fingerabdruck oder ein fehlendes
Plugin –, sagt das Synchronisationsziel das bei diesem Konto und bietet
**… erneut versuchen** an. Der Versuch nennt den tatsächlichen Grund und
bietet die Reparatur an, wo sie in einer Bestätigung besteht. **Trennen**
bleibt ebenfalls erreichbar.

Welches Konto ein Gerät verwendet, ist eine **gerätelokale** Entscheidung:
Die Konten selbst wandern zwischen deinen Geräten, die Wahl des Ziels nicht.
So kann ein Laptop über das Internet und ein Rechner über eine Freigabe im
selben Netz auf denselben Datensatz zugreifen.

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
musst. Das gilt auch für Konten, die du **vor** dem Aktivieren der
Verschlüsselung angelegt hast – deren Zugangsdaten werden beim Einschalten
automatisch nachgezogen. **Ohne** Verschlüsselung bleiben Zugangsdaten ausschließlich lokal auf
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
