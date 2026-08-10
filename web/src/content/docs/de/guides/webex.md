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

Mehr ist hier nicht zu entscheiden. Ob ein Meeting ein frisches ist oder dein
dauerhafter Raum, und ob Webex die Teilnehmer anmailen soll, wird beides pro
Meeting beantwortet — das zweite beantwortet Aperio für dich. Siehe *Wer die
Teilnehmer benachrichtigt* weiter unten.

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

Termin öffnen, bei einem neuen erst speichern, dann eines von beiden wählen:

- **Meeting erzeugen** legt bei Webex ein frisches Meeting mit eigenem Link und
  eigenem Passwort an, nur für diesen Termin.
- **Persönlichen Raum verlinken** zeigt stattdessen auf deinen dauerhaften Raum.
  Der braucht keine Planungslizenz und hat kein Tageslimit, ist aber immer
  derselbe Raum unter demselben Link — direkt aufeinander folgende Termine
  können sich dort begegnen.

Die Wahl gilt pro Termin, nicht pro Konto: was ein Meeting sein soll, ist eine
Eigenschaft dieses Meetings, und gefragt wird in dem Moment, in dem du die
Antwort kennst. In beiden Fällen

- legt das Meeting bei Webex mit Titel und Zeit des Termins an,
- übergibt Webex die Teilnehmer des Termins, damit das Meeting weiß, für wen
  es ist,
- schreibt den Beitrittslink in das Ortsfeld (falls es leer war) und hängt den
  vollständigen Einwahlblock an die Beschreibung,
- und merkt sich, dass dieses Meeting zu diesem Termin gehört.

Der Block enthält alles, was Webex herausgibt, je Angabe eine beschriftete
Zeile:

```
Meeting beitreten: https://example.webex.com/…
Meeting-Kennnummer (Zugriffscode): 2731 234 5678
Meeting-Passwort: Tmv36kRq3vJ
Passwort für Telefon- und Videosysteme: 98476838
Über Telefon beitreten: +49-619-6781-9736 (Germany Toll)
Über Telefon beitreten: +1-408-418-9388 (US Toll)
Globale Einwahlnummern: https://example.webex.com/globalcallin.php?…
Über Videogerät oder -anwendung beitreten: 27312345678@example.webex.com
```

Die zwei Passwörter sind kein Versehen. Webex vergibt ein alphanumerisches für
die App und ein numerisches für Tastenfelder — `Tmv36kRq3vJ` lässt sich am
Telefon gar nicht eingeben. Cisco druckt in seinen eigenen Einladungen beide,
und hier steht es genauso.

Je Angabe eine Zeile, und Klartext statt Auszeichnung. Beides zugunsten dessen,
der sich einwählt oder mit einem Screenreader liest: ein umgebrochener Wert
bliebe beim Entfernen des Meetings stehen, und Auszeichnung, die der empfangende
Client nicht darstellt, wird als wörtliche spitze Klammern vorgelesen.

**Sprache der Einladung.** Unter den beiden Knöpfen steht ein drittes
Bedienelement, vorbelegt mit der Sprache, in der Aperio läuft. Es bestimmt die
Sprache der Beschriftungen oben — und gilt pro Termin, denn ein deutscher
Nutzer, der englische Kollegen einlädt, will eine englische Einladung. Die Wahl
muss vor dem Anlegen fallen: Der Block wird in den Termin geschrieben und geht
so an alle, und keine Kalender-App kann ihn nachträglich übersetzen. Die Wörter
kommen vom Webex-Adapter selbst, nicht von Aperio — eine Sprache, die Aperio nie
gesehen hat, ist damit eine Plugin-Aktualisierung und keine neue App-Version.
Was der Adapter nicht übersetzt hat, fällt auf Englisch zurück.

Wen du einlädst, sieht den Link in einem ganz gewöhnlichen Termin — egal, welche
Kalender-App er benutzt.

**Meeting entfernen** erscheint, sobald ein Termin eines hat. Es löscht das
Meeting bei Webex und nimmt den Link wieder aus dem Termin.

Den Entfernen-Knopf siehst du nur bei Meetings, die **Aperio angelegt hat**. Ein
Termin mit dem Webex-Link eines Kollegen bekommt einen Beitreten-Knopf und sonst
nichts — dieses Meeting zu löschen steht dir nicht zu.

## Meetings ohne Kalendereintrag

Ein Meeting, das du direkt in Webex' eigener Weboberfläche anlegst, existiert
nur dort. Es hat keinen Kalendereintrag, also hat es nie eine Kalender-App
angezeigt — die erste Erinnerung ist der Beginn.

Sobald ein Webex-Konto verbunden ist, legt Aperio einen **schreibgeschützten
Kalender mit dem Namen dieses Kontos** an, in dem genau diese Meetings stehen.
Er verhält sich wie jeder andere Kalender: in der Seitenleiste ein- und
ausschalten, in Tages- und Wochenansicht sehen, aus dem Kontextmenü beitreten.

Ein Meeting, das schon einen Kalendereintrag hat — weil Aperio es angelegt hat
oder eine Einladung es mitgebracht hat —, steht nicht doppelt da. Aperio führt
die beiden über den Beitrittslink zusammen, der eindeutig ist, und
**gruppiert** sie: Der Tag zeigt eine Zeile mit der Marke „2×", und wer die
Gruppe öffnet, sieht beide Kopien benannt und kommt zu jeder von ihnen.

Das ist der Erwähnung wert, weil hier das frühere Verhalten still danebenlag:
Wird der Termin verschoben und das Meeting nicht, passt der Beitrittslink
weiterhin — das Meeting einfach zu verstecken würde es also genau dann
verstecken, wenn die beiden aufgehört haben, sich einig zu sein. Eine Gruppe tut
das nicht: Sie faltet dann nicht mehr und zeigt beide Zeilen, markiert mit „≠".

Nimmst du die beiden auseinander, bleiben sie auseinander — auf allen Geräten,
nicht nur auf diesem.

Der Kalender lässt sich nicht bearbeiten. Er zeigt, was bei Webex existiert; um
ein Meeting anzulegen, trägst du einen Termin in einen deiner eigenen Kalender
ein und benutzt dort **Meeting erzeugen**. Dein dauerhafter persönlicher Raum
taucht ebenfalls nicht auf: er ist immer offen, und das ist gerade kein Termin.

## Was du wissen solltest

**Ein Meeting, das die Einladung mitgebracht hat.** Wenn Webex dir eine
Einladung mailt und dein Kalender daraus einen Termin macht, hat dieser Termin
ein Meeting — nur hat Aperio es nicht angelegt. Der Editor bietet dann
**Meeting übernehmen**: er schlägt das Meeting über seinen Beitrittslink nach,
und danach lässt es sich entfernen wie ein selbst angelegtes. In den Termin wird
nichts geschrieben — der Link steht ja schon drin.

**Wer die Teilnehmer benachrichtigt.** Webex kann allen selbst eine Einladung
mailen, und seine Mails bringen einen Kalenderanhang mit. Das ist eine Dublette,
wenn dein eigener Kalender die Leute schon serverseitig einlädt — Exchange,
Google und ein CalDAV-Server mit Scheduling tun das —, denn dann bekommt jeder
zwei Einladungen und zwei Einträge. Auf einem Kalender, der überhaupt niemanden
einladen kann (lokaler Kalender, abonnierter Feed, einfaches CalDAV), ist Webex'
Mail dagegen die einzige Einladung, die es je geben wird, und sie zu unterdrücken
heißt, dass niemand Bescheid weiß.

Deshalb ist das keine Einstellung. Wenn Aperio ein Meeting anlegt, schaut es auf
den Kalender des Termins und bittet Webex genau dann um die Mail, wenn dieser
Kalender es nicht kann. Ein Termin ohne Teilnehmer mailt so oder so niemandem.

Beim Entfernen gilt dasselbe Prinzip, mit einer Vereinfachung: Gefragt wird nur
noch, ob der Kalender absagen kann, nicht wer auf dem Termin steht. Ein Kalender,
der einladen konnte, sagt auch ab, und Webex bleibt still; einer, der es nicht
konnte, bekommt Webex' Absage — sonst erfahren die Teilnehmer nie, dass das
Meeting ausfällt, und stehen einfach da.

Die Teilnehmerfrage entfällt auf dem Weg hinaus mit Absicht. Ein Meeting, das du
aus Webex' eigener Oberfläche übernommen hast, hat Eingeladene, von denen der
Termin nie wusste, und wenn du auf Entfernen drückst, kann der Termin längst weg
sein. Webex mailt nur die Leute, die es hat — es zu bitten kostet also nichts,
wenn da niemand ist.

**Wer wirklich eingeladen ist.** Die Teilnehmer eines solchen Termins sind das,
was die Einladungsmail adressiert hat — oft nur du und Webex' eigene
Versandadresse (`messenger@webex.com`). Aperio zeigt zusätzlich Webex' eigene
Eingeladenen-Liste unter „Beim Anbieter eingeladen", damit du siehst, wer
tatsächlich kommt. Wenn Webex die Auskunft verweigert — die Eingeladenen eines
Meetings zu lesen, an dem man nur teilnimmt, ist nicht immer erlaubt —, fehlt
der Abschnitt einfach.

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

**Einen verlinkten persönlichen Raum entfernen.** *Meeting entfernen* nimmt den
Link wieder aus dem Termin. Der Raum selbst bleibt — er gehört zu deinem Konto,
nicht zu einem einzelnen Termin, und Webex kennt keinen Weg, ihn zu löschen.

**Fürs Planen braucht es eine Lizenz.** Pro Termin ein Meeting anzulegen setzt
ein Webex-Konto voraus, das Meetings planen darf. Wenn deines das nicht darf,
nimm im Termineditor **Persönlichen Raum verlinken** — der Raum braucht keine
Planungslizenz. Das ist ein Knopf neben *Meeting erzeugen*, keine Einstellung:
vorher ist nichts einzuschalten, und die Wahl bleibt pro Termin.

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
