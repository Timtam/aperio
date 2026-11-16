---
title: Datenschutzerklärung
description: Wie Aperio mit deinen Daten umgeht.
lastUpdated: false
---

Aperio ist eine barrierefreiheit-zuerst entwickelte Kalender-, Aufgaben- und
Kontakte-Anwendung für Desktop und Mobilgeräte (Android und iOS). Diese Erklärung
beschreibt, welche Daten die App und diese Website verarbeiten. Kurz gesagt:
**Aperio betreibt keinen eigenen Server und enthält keine Analyse, Werbung,
Tracking oder Absturzberichte; deine Daten bleiben auf deinem Gerät oder in den
von dir gewählten Konten und Speichern.** Nur wenn du selbst Feedback sendest,
geht diese Nachricht an den Entwickler (siehe
[Feedback und Protokolle senden](#feedback-und-protokolle-senden)).

## Verantwortlicher

Verantwortlich für die Datenverarbeitung im Zusammenhang mit dieser Website und
der Anwendung Aperio ist:

> Toni Barth · Robert-Koch-Str. 44 · 06110 Halle (Saale) · Deutschland
> E-Mail: contact@toni-barth.online

## Was Aperio verarbeitet

Aperio ist **local-first**. Deine Kalender, Aufgaben und Kontakte werden **auf
deinem Gerät** gespeichert. Wenn du ein externes Konto verbindest (Google,
iCloud/CalDAV, Microsoft/Exchange, Vikunja, Todoist, einen iCal-Feed oder den
Kalender deines Geräts), spricht Aperio in deinem Auftrag **direkt** mit dem
jeweiligen Anbieter, um ausschließlich die von dir angeforderten Einträge zu
lesen und zu schreiben. Aperio betreibt **keinen eigenen Server**, der deine
Inhalte empfängt, speichert oder weiterleitet.

Die optionale geräteübergreifende Synchronisation läuft über **von dir gewählten
Speicher** (z. B. WebDAV, SFTP, Dropbox, Google Drive oder einen lokalen Ordner)
und kann **Ende-zu-Ende-verschlüsselt** werden, sodass der Sync-Speicher deine
Daten nie unverschlüsselt sieht.

Zugangsdaten und OAuth-Tokens der von dir verbundenen Anbieter werden
**ausschließlich auf deinem Gerät** gespeichert und nur zur Kommunikation mit
diesen Anbietern verwendet.

## Berechtigungen der mobilen App

Die Android- und iOS-App fordern nur die Berechtigungen an, die eine Funktion
benötigt, und nur, wenn du die Funktion nutzt:

- **Kalender / Erinnerungen** (`READ_CALENDAR` / `WRITE_CALENDAR` unter Android,
  Kalender- und Erinnerungszugriff unter iOS): nur, wenn du den integrierten
  *Gerätekalender* aktivierst — um die bereits auf deinem Gerät vorhandenen
  Kalender und Erinnerungen zu lesen und zu schreiben. Nutzt du ihn nicht, wird
  keine Kalenderberechtigung angefordert. Auf einem Telefon, auf dem das System
  noch nie gefragt hat (nach dem Umzug auf ein neues), fragt Aperio beim Start,
  und nur, solange es ein solches Konto gibt. Android kann nicht sagen, ob es je
  gefragt hat; Aperio merkt sich deshalb auf dem Telefon, dass es gefragt hat.
  Der Vermerk geht nicht ins Backup, ein neues Telefon wird also wieder
  gefragt.
- **Fotos**: nur das einzelne Bild, das du ausdrücklich auswählst (z. B. ein
  Kontaktfoto); die App hat keinen pauschalen Zugriff auf deine Fotomediathek,
  und das Bild wird lokal verarbeitet.
- **Benachrichtigungen**: um deine Erinnerungen anzuzeigen, die **lokal** auf
  deinem Gerät geplant werden. Es gibt **keinen Push-Server** — keine
  Fernnachricht erreicht die App.

Die App enthält **keine Analyse, keine Werbe-SDKs, kein Tracking und keine
Absturzberichte** und erhebt weder eine Werbe-ID noch deinen Standort.

## Google-Nutzerdaten

Die Verwendung und Weitergabe von Informationen, die Aperio von Google-APIs
erhält, erfolgt im Einklang mit der
[Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy)
einschließlich der Limited-Use-Anforderungen. Google-Daten (Kalender, Aufgaben,
Kontakte) werden ausschließlich genutzt, um die von dir in der App aufgerufenen
Funktionen bereitzustellen, lokal auf deinem Gerät verarbeitet und nicht an
Dritte verkauft, übertragen oder weitergegeben.

## Feedback und Protokolle senden

Unter **Einstellungen → Feedback** (und über **Protokolle senden** im Bereich
Protokolle) kannst du dem Entwickler eine Nachricht schicken. Das passiert nur,
wenn du auf **Senden** drückst; vorher verlässt nichts das Gerät, und alles,
was mitgeht, steht im Formular.

- **Was gesendet wird:** deine Nachricht; deine E-Mail-Adresse, wenn du eine
  angibst, damit eine Antwort dich erreicht; eine Zeile mit der Version von
  Aperio, dem Betriebssystem und seiner Version (auf dem Telefon auch die
  Build-Nummer, unter Android das Gerätemodell); auf deinen Wunsch die
  Protokolle der App als ZIP-Datei, höchstens die neuesten 2 MB, in denen
  E-Mail-Adressen und Zugriffstokens vorher durch Platzhalter ersetzt sind. Wie
  bei jeder Verbindung erhält der Dienst technisch deine IP-Adresse.
- **Über wen:** Die Nachricht geht an ein Formular bei
  [Static Forms](https://www.staticforms.dev/privacy-policy), das sie per
  E-Mail an den Verantwortlichen zustellt und in seinem Posteingang aufbewahrt.
  Static Forms ist Auftragsverarbeiter; ein Vertrag zur Auftragsverarbeitung
  besteht. Die Formulardaten liegen in der EU (AWS, Frankfurt). Die
  Benachrichtigungs-E-Mail versendet Static Forms über AWS in den USA.
- **Spam-Prüfung:** Static Forms bewertet jede Einsendung mit einem KI-Spamfilter.
  Dafür gehen nur die zur Bewertung nötigen Felder an einen KI-Anbieter;
  Anhänge, Zugangsdaten und Tokens werden vorher entfernt und E-Mail-Adressen
  maskiert. Der Anbieter speichert nichts und trainiert nicht mit den Daten;
  die Auswertung kann außerhalb der EU stattfinden. Die beteiligten
  Unterauftragnehmer nennt die
  [Liste von Static Forms](https://www.staticforms.dev/dpa/sub-processors).
  Gegen Bots schützt das Formular außerdem eine Rechenaufgabe, die Aperio auf
  deinem Gerät löst (ALTCHA); dafür werden keine Daten über dich übertragen.
- **Zweck und Grundlage:** deine Rückmeldung zu lesen, zu beantworten und
  Fehler in Aperio zu beheben (Art. 6 Abs. 1 lit. f DSGVO); das Absenden ist
  freiwillig.
- **Speicherdauer:** Einsendungen werden gelöscht, sobald die Rückmeldung
  bearbeitet ist, spätestens nach 12 Monaten. Möchtest du eine Nachricht früher
  gelöscht haben, schreib an contact@toni-barth.online.

## Diese Website

Diese Dokumentationsseite wird als statische Dateien über **GitHub Pages**
bereitgestellt. Der Hoster verarbeitet ggf. technische Zugriffsdaten (z. B.
IP-Adresse, User-Agent) in Server-Logs, soweit zur Auslieferung der Seite
technisch erforderlich; siehe
[GitHubs Datenschutzerklärung](https://docs.github.com/de/site-policy/privacy-policies/github-general-privacy-statement).
Diese Seite setzt keine Tracking-Cookies und bindet keine Drittanbieter-Analyse
ein. Die Seitensuche läuft vollständig in deinem Browser.

## Deine Rechte

Nach der DSGVO hast du das Recht auf Auskunft, Berichtigung, Löschung,
Einschränkung der Verarbeitung, Datenübertragbarkeit und Widerspruch. Zur
Ausübung wende dich an contact@toni-barth.online. Außerdem hast du das Recht,
dich bei einer Datenschutz-Aufsichtsbehörde zu beschweren.

_Zuletzt aktualisiert: 16. November 2026._
