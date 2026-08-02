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
Dropbox, die Drive-Ablage deines Google-Kontos oder einen anderen Ordner,
der zwischen deinen Geräten geteilt wird. Es gibt keinen Aperio-Server; deine Daten bleiben bei dir.

## Synchronisation einrichten

Ein Speicherort ist in Aperio ein **Konto** wie jedes andere: Du legst ihn
unter **Einstellungen → Konten** an, und in der Synchronisation wählst du
nur noch aus, welches deiner Konten den Datensatz hält.

1. Öffne die **Einstellungen** und gehe zu **Konten**.
2. Lege ein Konto für deinen Speicherort an (z. B. WebDAV, SFTP, FTPS oder
   Dropbox) und gib Adresse und Zugangsdaten an. Zwei Einträge brauchen kein
   eigenes Konto: **ein Ordner auf diesem Gerät** gehört zu Aperio selbst,
   und **Google Drive** reitet auf einem Google-Konto, das du vielleicht
   schon für deine Kalender hast. Für beide überspringst du diesen Schritt
   und wählst sie im nächsten.
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

Protokolle, die einen Server über seinen Schlüssel identifizieren – SFTP
etwa –, lassen dich dessen Fingerabdruck beim ersten Mal bestätigen und
lehnen danach alles ab, was nicht dazu passt. Der bestätigte Fingerabdruck
steht beim Konto, daneben **Pin verwerfen**. Verwirf ihn, wenn der
Server-Schlüssel bekanntermaßen aus gutem Grund gewechselt hat, etwa nach
einer Neuinstallation: Die nächste Verbindung fragt dann nach dem neuen.
Der Pin gehört allein diesem Gerät und wird nirgendwohin übertragen.

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

## Deine Geräte

Jedes Gerät, das sich mit deinem Datensatz verbindet, trägt sich dort ein. Unter
**Einstellungen → Synchronisation → Geräte** siehst du die Liste und kannst sie
in Ordnung halten.

**Gerätename.** Trag hier ein, wie dieses Gerät auf deinen anderen Geräten heißen
soll – „Arbeitsrechner", „Telefon". Ohne Namen steht dort nur eine lange
Zeichenkette, die niemandem sagt, welches Gerät gemeint ist. Aperio schlägt den
Namen vor, den dein Rechner bzw. Telefon ohnehin trägt; gespeichert wird er erst,
wenn du auf **Gerätenamen speichern** gehst. Der Name gilt nur für dieses Gerät
und erreicht die anderen beim nächsten Abgleich.

**Zuletzt gesehen.** Jede Zeile sagt, wann das Gerät zuletzt einen Abgleich
abgeschlossen hat. Bei Geräten, die sich noch nie mit einer Version gemeldet
haben, die das festhält, steht „unbekannt" – nie ein erfundenes Datum.

**Geräte entfernen.** Nach ein paar Neuinstallationen oder Testgeräten stehen
Einträge in der Liste, hinter denen kein Gerät mehr steckt. Die kannst du
entfernen. Was dabei passiert – und was nicht:

- Es werden **keine Daten gelöscht**. Nur der Eintrag verschwindet, der dieses
  Gerät als Teilnehmer führt.
- Alte Einträge kosten tatsächlich etwas: Aperio hebt beim Aufräumen alte
  Protokolldateien so lange auf, bis **jedes** eingetragene Gerät sie gelesen
  hat. Ein Eintrag, hinter dem nichts mehr steckt, hält sie damit für immer.
- Du kannst dich nicht ernsthaft vertun. Läuft das Gerät noch, trägt es sich beim
  nächsten Abgleich einfach wieder ein.
- Das Gerät, an dem du gerade sitzt, lässt sich nicht entfernen – es würde sich
  sofort wieder eintragen. Wenn du dieses Gerät aus der Synchronisation nehmen
  willst, ist **Trennen** der richtige Weg.

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
