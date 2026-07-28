---
title: "Videokonferenzen mit Cisco Webex"
---

Aperio kann zwei Dinge mit Webex, und die sind voneinander unabhängig.

**Beitreten** geht sofort, bei jedem Meeting, aus jedem Werkzeug. Eine
Einladung, die Outlook, eM Client oder Webex selbst geschrieben hat, enthält
einen Beitrittslink, und Aperio findet ihn — im Ortsfeld oder irgendwo im Text,
in jeder Sprache. Dafür brauchst du kein Webex-Konto in Aperio und musst nichts
einrichten. Jeder Termin mit einer Konferenz bekommt einen **Beitreten**-Eintrag:
im Termin-Editor, im Kontextmenü (`Umschalt+F10` oder Menütaste auf einem
Termin) und im Rotor auf dem Telefon.

**Erstellen** braucht ein Konto. Sobald eines verbunden ist, bekommt ein Termin
den Knopf **Meeting erzeugen**: Aperio legt bei Webex ein Meeting für diesen
Termin an, schreibt dessen Link in den Termin, wo ihn jede andere Kalender-App
lesen kann, und merkt sich das Meeting, damit es es auch wieder abräumen kann.

## Webex-Konto verbinden

**Einstellungen → Konten → Konto hinzufügen**, dann **Cisco Webex** wählen.

Was danach kommt, hängt von der Version ab, die du benutzt:

- Fragt sie nur nach einem Namen, bringt diese Version Aperios eigene
  Webex-Registrierung mit. Namen eintragen, **Hinzufügen** — der Browser öffnet
  die Webex-Anmeldung, du erteilst die Zustimmung, der Tab schließt sich von
  selbst.
- Fragt sie zusätzlich nach **Client-ID** und **Client-Secret**, bringt diese
  Version keine mit, und du legst einmalig eine eigene Integration an. Das
  dauert etwa fünf Minuten und ist kostenlos; der nächste Abschnitt führt
  hindurch.

Zwei Optionen lohnen einen Moment:

**Persönlichen Raum verwenden.** Aus als Voreinstellung. Eingeschaltet verlinkt
Aperio deinen dauerhaften persönlichen Raum, statt pro Termin ein neues Meeting
anzulegen. Das braucht keine Planungslizenz und hat kein Tageslimit — aber alle
Termine teilen sich denselben Link und denselben Raum, direkt aufeinander
folgende Meetings können sich also begegnen. Ausgeschaltet bekommt jeder Termin
sein eigenes Meeting.

**Webex eigene Einladungen senden lassen.** Aus als Voreinstellung, und das
solltest du so lassen. Webex-Mails bringen einen Kalenderanhang mit;
eingeschaltet landet bei allen Teilnehmern ein **zweiter** Termin neben dem,
den Aperio schon verschickt hat.

## Eigene Integration anlegen

Nur nötig, wenn das Formular nach Client-ID und Secret fragt.

1. [developer.webex.com/my-apps](https://developer.webex.com/my-apps) öffnen und
   mit dem Webex-Konto anmelden.
2. **Create a New App → Integration.**
3. Name und Beschreibung vergeben — die sind für dich, niemand sonst sieht sie.
   Ein Icon ist Pflicht; ein beliebiges quadratisches PNG genügt.
4. **Redirect URI:** genau das hier eintragen:

   ```
   http://127.0.0.1:8080/oauth/webex
   ```

   Das muss zeichengenau stimmen. Es ist eine Loopback-Adresse — die Seite
   verlässt deinen Rechner nie; Aperio hört dort auf den Moment, in dem die
   Anmeldung zurückkommt.
5. **Scopes:** `meeting:schedules_read`, `meeting:schedules_write` und
   `meeting:preferences_read` anhaken. Webex fügt `spark:kms` von selbst hinzu;
   das ist normal und kein Grund zur Sorge.
6. Speichern. Webex zeigt **Client-ID** und **Client-Secret**. Beides in Aperios
   Formular übertragen.

Das Secret landet im Schlüsselbund des Systems, nie in Aperios Kontodatenbank —
was deshalb zählt, weil genau diese Datenbank auf deine anderen Geräte
synchronisiert wird.

> **Zum „mobile SDK".** Wenn Webex fragt, ob die Integration ein mobiles SDK
> verwendet: **nein**. Aperio spricht mit der Meetings-REST-API, nicht mit
> Webex' eigenem App-SDK.

## Meeting für einen Termin anlegen

Termin öffnen, bei einem neuen erst speichern, dann **Meeting erzeugen**. Aperio

- legt das Meeting bei Webex mit Titel und Zeit des Termins an,
- schreibt den Beitrittslink in das Ortsfeld (falls es leer war) und hängt einen
  kurzen Block mit Link und Passwort an die Beschreibung,
- und merkt sich, dass dieses Meeting zu diesem Termin gehört.

Wen du einlädst, sieht den Link in einem ganz gewöhnlichen Termin — egal, welche
Kalender-App er benutzt.

**Meeting entfernen** erscheint, sobald ein Termin eines hat. Es löscht das
Meeting bei Webex und nimmt den Link wieder aus dem Termin.

Den Entfernen-Knopf siehst du nur bei Meetings, die **Aperio angelegt hat**. Ein
Termin mit dem Webex-Link eines Kollegen bekommt einen Beitreten-Knopf und sonst
nichts — dieses Meeting zu löschen steht dir nicht zu.

## Was du wissen solltest

**Ein Meeting pro Termin, auch bei Wiederholungen.** Eine Serie teilt sich ein
Meeting, genau wie ein wiederkehrendes Meeting in Webex selbst.

**Entfernen geht von dem Gerät, das es angelegt hat.** Der Vermerk, welches
Meeting zu welchem Termin gehört, bleibt auf dem Rechner, der es erzeugt hat —
er wird nicht synchronisiert, weil er Buchhaltung über ein Webex-Objekt ist und
nicht Teil deines Termins. Auf einem anderen Gerät kannst du den Termin trotzdem
löschen; das Meeting bleibt dann bei Webex stehen, wo du es in Webex' eigener
Oberfläche entfernen kannst.

**Einen Termin zu verschieben verschiebt das Meeting nicht.** Webex' API kennt
in dem Satz, den Aperio nutzt, kein Ändern. Wenn sich eine Zeit wesentlich
ändert: Meeting entfernen und neu anlegen.

**Fürs Planen braucht es eine Lizenz.** Pro Termin ein Meeting anzulegen setzt
ein Webex-Konto voraus, das Meetings planen darf. Wenn deines das nicht darf,
schalte **Persönlichen Raum verwenden** ein — das funktioniert auch ohne.

## Wenn etwas nicht klappt

**„Kein Plugin bedient diese Adapter-Art."** Das Webex-Plugin ist nicht geladen
oder wurde unter **Einstellungen → Plugins** abgeschaltet.

**Die Anmeldung kommt nie zurück.** Prüfe die Redirect-URI deiner Integration
zeichenweise, samt Port und `/oauth/webex`. Falls Port 8080 auf deinem Rechner
schon belegt ist, sagt Aperio das, bevor der Browser überhaupt aufgeht.

**Plötzlich „Bei Webex erneut anmelden".** Die Webex-Registrierung deiner
Version hat sich geändert — das passiert beim Wechsel zwischen einer offiziellen
Version und einer selbst gebauten. Konto neu verbinden.

Aperios Protokoll (**Einstellungen → Fehlersuche**) hält den fehlgeschlagenen
Anfragepfad samt Status fest, deine Token niemals.
