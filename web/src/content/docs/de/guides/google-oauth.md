---
title: "Google einbinden – eigene OAuth-Zugangsdaten (Übergangslösung)"
---

> **Hinweis:** Diese Anleitung ist eine **Übergangslösung**. Aperio bringt
> noch keine verifizierte Google-App-Registrierung mit — deshalb brauchst du
> aktuell einmalig ein eigenes (kostenloses) Projekt in der Google Cloud
> Console. Sobald Aperio offizielle Zugangsdaten mitliefert, entfällt dieser
> ganze Abschnitt und es reicht „Konto hinzufügen → Google → Anmelden".

Was dich erwartet: etwa **15 Minuten** Klickarbeit, komplett **kostenlos**,
keine Kreditkarte nötig. Du brauchst nur dein Google-Konto. Am Ende hast du
zwei Zeichenketten — **Client-ID** und **Client-Secret** —, die du in Aperio
einträgst.

> **Warum überhaupt?** Die Client-ID identifiziert gegenüber Google die
> *Anwendung* (nicht dich). Da Aperio noch nicht bei Google als verifizierte
> App registriert ist, registrierst du dir quasi deine private „Aperio-App"
> selbst. Dein Google-Passwort bekommt Aperio dabei nie zu sehen — die
> Anmeldung läuft immer über die normale Google-Anmeldeseite im Browser.

## Schritt 1: Projekt anlegen

1. Öffne [console.cloud.google.com](https://console.cloud.google.com) und
   melde dich mit deinem Google-Konto an. (Beim allerersten Besuch musst du
   den Nutzungsbedingungen zustimmen.)
2. Klicke oben in der Leiste auf die **Projektauswahl** (zeigt anfangs
   „Projekt auswählen") und dann auf **„Neues Projekt"**.
3. Projektname z. B. `Aperio`, Organisation leer lassen → **„Erstellen"**.
4. Warte kurz und wähle das neue Projekt in der Projektauswahl aus (oben
   muss jetzt „Aperio" stehen).

## Schritt 2: APIs aktivieren

Aperio spricht drei Google-Dienste an. Aktiviere für dein Projekt:

1. Menü (☰) → **„APIs und Dienste" → „Bibliothek"**.
2. Suche nacheinander die folgenden Einträge, öffne sie und klicke jeweils
   auf **„Aktivieren"**:
   - **Google Calendar API** (Termine)
   - **Google Tasks API** (Aufgaben)
   - **People API** (Kontakte und Teilnehmer-Vorschläge)

> **Optional – Google Drive als Sync-Speicher:** Wenn du zusätzlich die
> [Geräte-Synchronisation](/de/guides/tutorial/08-synchronisation/) über Google Drive
> laufen lassen willst, aktiviere außerdem die **Google Drive API**.
> Dasselbe Projekt und dieselben Zugangsdaten aus dieser Anleitung kannst du
> dann auch im Sync-Dialog verwenden.

## Schritt 3: OAuth-Zustimmungsbildschirm einrichten

Das ist die Seite, die Google dir beim Verbinden im Browser anzeigt.

1. Menü (☰) → **„APIs und Dienste" → „OAuth-Zustimmungsbildschirm"**.
   (Google baut die Console gelegentlich um — der Bereich heißt teils auch
   **„Google Auth Platform"** mit den Unterseiten *Branding*, *Zielgruppe*,
   *Clients*.)
2. Beim ersten Mal startet ein Einrichtungs-Assistent:
   - **App-Name:** `Aperio`
   - **Nutzer-Support-E-Mail:** deine E-Mail-Adresse
   - **Zielgruppe / User Type:** **Extern**
   - **Kontaktdaten des Entwicklers:** deine E-Mail-Adresse
3. Alles Weitere (Logo, Domains, Scopes) kannst du **leer lassen** —
   Aperio fordert die nötigen Berechtigungen beim Anmelden selbst an.
   Assistent abschließen.

## Schritt 4: Dich selbst als Testnutzer eintragen

Frisch angelegte Apps stehen im Status **„Testing"** — anmelden dürfen sich
nur eingetragene Testnutzer.

1. Im Bereich OAuth-Zustimmungsbildschirm / **„Zielgruppe"** (Audience) den
   Abschnitt **„Testnutzer"** suchen.
2. **„+ Add users"** → die Gmail-Adresse(n) **jedes** Google-Kontos
   eintragen, das du in Aperio verbinden möchtest → speichern.

> **Wichtig — der 7-Tage-Haken:** Im Status „Testing" erklärt Google
> Anmeldungen nach **7 Tagen** für abgelaufen — du müsstest das Konto in
> Aperio dann jede Woche neu verbinden. Die Lösung: Klicke auf derselben
> Seite auf **„App veröffentlichen"** (Status „In Produktion"). Deine
> Anmeldung bleibt dann dauerhaft gültig. Beim Verbinden zeigt Google dafür
> einmalig eine Warnseite („Google hat diese App nicht überprüft") — das ist
> unbedenklich, denn es ist *deine eigene* App-Registrierung: auf
> **„Erweitert"** und dann **„Aperio öffnen (unsicher)"** klicken.
> **Empfehlung: veröffentlichen.**

## Schritt 5: OAuth-Client (Typ „Desktop-App") anlegen

Jetzt entstehen die beiden Zeichenketten für Aperio:

1. Menü (☰) → **„APIs und Dienste" → „Anmeldedaten"** (Credentials).
2. **„+ Anmeldedaten erstellen" → „OAuth-Client-ID"**.
3. **Anwendungstyp: „Desktop-App"** (wichtig — nicht „Webanwendung").
   Name beliebig, z. B. `Aperio Desktop` → **„Erstellen"**.
4. Ein Dialog zeigt nun:
   - die **Client-ID** — lange Zeichenkette, endet auf
     `.apps.googleusercontent.com`
   - das **Client-Secret** — beginnt üblicherweise mit `GOCSPX-`
5. Beide kopieren (z. B. in einen Editor zwischenlagern). Du kannst sie
   jederzeit wieder einsehen: **Anmeldedaten** → deinen Client anklicken.

## Schritt 6: In Aperio eintragen

1. In Aperio: **Einstellungen → Konten → Konto hinzufügen** → Anbieter
   **Google** wählen.
2. In das Feld **„Google OAuth Client-ID"** die Client-ID einfügen, in
   **„Google OAuth Client-Secret"** das Secret.
3. Auf **„Hinzufügen"** klicken. Aperio öffnet deinen Browser mit der
   Google-Anmeldung:
   - Google-Konto auswählen.
   - Falls die Warnseite „Google hat diese App nicht überprüft" erscheint
     (siehe Schritt 4): **„Erweitert" → „Aperio öffnen (unsicher)"**.
   - Den angefragten Berechtigungen (Kalender, Aufgaben, Kontakte)
     **zustimmen**.
4. Der Browser-Tab schließt sich automatisch; deine Google-Kalender,
   -Aufgabenlisten und -Kontakte erscheinen in der Seitenleiste.

## Wenn etwas hakt

| Symptom | Ursache & Lösung |
|---|---|
| **„Fehler 403: access_denied"** beim Anmelden | Dein Konto ist nicht als Testnutzer eingetragen (Schritt 4) — eintragen **oder** App veröffentlichen. |
| Warnseite **„Google hat diese App nicht überprüft"** | Normal bei eigener Registrierung. **„Erweitert" → „Aperio öffnen (unsicher)"**. |
| Nach etwa einer Woche **abgemeldet** | Die App steht noch im Status „Testing" (7-Tage-Limit). In der Console **„App veröffentlichen"**, dann das Konto in Aperio über **Einstellungen → Konten** neu verbinden. |
| **„accessNotConfigured"** / „API has not been used" | Eine der APIs aus Schritt 2 ist nicht aktiviert — nachholen (der Fehlertext nennt die fehlende API). |
| **„invalid_client"** | Client-ID oder -Secret falsch bzw. mit Leerzeichen kopiert — beide neu aus der Console kopieren. |

> **Sicherheit:** Client-ID und -Secret identifizieren nur deine
> App-Registrierung — sie gewähren für sich genommen keinen Zugriff auf dein
> Konto. Die eigentlichen Zugangs-Tokens entstehen erst durch deine
> Anmeldung im Browser und werden von Aperio lokal gespeichert; bei
> aktivierter [E2E-Verschlüsselung](/de/guides/tutorial/08-synchronisation/) wandern
> sie zusätzlich verschlüsselt zwischen deinen Geräten.
