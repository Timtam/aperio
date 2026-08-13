---
title: "05 – Kontakte"
---

In diesem Kapitel richtest du dein Adressbuch ein, lernst, wie eine
Telefonnummer sagt, welches Telefon sie ist, und siehst, woher die Geburtstage
kommen.

Aperio führt Kontakte neben deinen Kalendern und Aufgabenlisten, aus denselben
Konten, die du in [Kapitel 02](/de/guides/tutorial/02-konten-verbinden/)
verbunden hast: Ein iCloud- oder Nextcloud-Konto bringt seine
CardDAV-Adressbücher mit, ein Google- oder Microsoft-Konto seine Kontakte, ein
Exchange-Konto deine eigenen plus das Firmenverzeichnis – und es gibt ein
**lokales Adressbuch**, das gar kein Konto braucht.

Aus den Kontakten schöpft die Teilnehmerauswahl, wenn du jemanden zu einem
Termin einlädst ([Kapitel 03](/de/guides/tutorial/03-termine/)), und aus ihnen
entstehen die **Geburtstagskalender**.

## Der Überblick

Öffne **Kontakte** in der Seitenleiste (am Telefon: den Reiter **Kontakte**).
Die Liste ist nach Adressbuch gruppiert, die Anzahl der Einträge steht in der
Gruppenüberschrift.

- `Eingabe` öffnet den ausgewählten Kontakt zum Bearbeiten.
- `Einfügen` legt einen neuen an.
- `Entfernen` löscht den ausgewählten Kontakt (mit Rückfrage).
- Die Kontextmenü-Taste oder `Umschalt+F10` öffnet sein Menü.
- Die **Suche** greift in deine eigenen Adressbücher *und* in verbundene
  Verzeichnisse wie die Globale Adressliste – auch auf Personen, die in keinem
  deiner Bücher stehen.

Am Telefon sind die Adressbücher **standardmäßig zugeklappt**: Du bekommst eine
kurze Liste von Buch-Überschriften und klappst nur das auf, das du brauchst –
so begräbt ein Firmenverzeichnis mit tausenden Einträgen nicht dein
persönliches Buch. Jede Kontaktzeile trägt **Bearbeiten** und **Löschen** als
Aktionen an der Zeile selbst.

> **Nur-Lese-Bücher.** Verzeichnisse – die Globale Adressliste, Google
> Directory und „Weitere Kontakte", Microsofts „Vorgeschlagene Personen" – sind
> ihrer Natur nach nur lesbar. Ihre Einträge öffnen sich in einem reinen
> Ansichts-Editor: Du kannst jedes Feld lesen, aber Speichern und Löschen sind
> nicht da.

## Was in einem Kontakt steht

- **Anzeigename** (erforderlich), Vorname, Nachname.
- **Organisation**, **Position** und **Abteilung**.
- **E-Mail-Adressen**, **Telefonnummern** und **Webseiten** – beliebig viele,
  jede mit einer eigenen Bezeichnung (siehe unten).
- **Postanschriften** – Straße, PLZ, Ort, Region, Land, jede mit einer
  Bezeichnung.
- **Geburtstag** und **Jahrestag**.
- **Notizen**.
- Ein **Foto**, das du aus einer Datei setzen kannst (am Telefon aus der
  Fotomediathek) und wieder entfernen.

Ein Kontakt kann auch eine **Verteilerliste** sein: Setze den Haken bei *Dies
ist eine Verteilerliste (Gruppe)*, und die Personenfelder weichen einem
Mitglieder-Editor – ein Mitglied pro Zeile, entweder
`Name <adresse@example.com>` oder eine nackte E-Mail-Adresse.

## Bezeichnungen: welche Nummer ist welche

Eine E-Mail-Adresse, eine Telefonnummer oder eine Webseite ist nie nur der
Wert. Jede steht in ihrer eigenen Zeile, und davor steht eine **Bezeichnung**:

**Privat · Dienstlich · Mobil · Fax · Sonstige · Ohne Bezeichnung · Eigene …**

Wählst du **Eigene …**, erscheint ein Textfeld – dann heißt eine Nummer eben
*Ferienhaus* oder *Empfang*, wenn sie das ist.

Am Desktop ist die Bezeichnung eine Auswahlliste oben in der Zeile. Am Telefon
ist sie ein **Knopf**, der die Auswahl in einem Dialog öffnet; der Knopf sagt
selbst, was gewählt ist – du hörst „Telefonnummer 2, Bezeichnung: Mobil", ohne
irgendetwas zu öffnen.

Eine Zeile fügst du mit **Telefonnummer hinzufügen** an (bzw. *E-Mail-Adresse
hinzufügen* / *Webseite hinzufügen*), entfernst sie mit dem Knopf **Entfernen**
am Ende ihrer Zeile.

> **Hinweis für Screenreader:** Jede Zeile ist eine kleine Gruppe für sich:
> Bezeichnungs-Knopf bzw. -Auswahl, dann der Wert, dann Entfernen. Die
> Bezeichnung kommt immer vor dem Wert – wanderst du durch einen Kontakt mit
> vier Nummern, hörst du zuerst, welche es ist, und erst dann die Ziffern. Die
> Zeilennummer steckt im Namen jedes Bedienelements („Telefonnummer 2
> entfernen"), du weißt also jederzeit, wo du bist.

### Was die Anbieter aus einer Bezeichnung machen

Jedes System legt eine Telefonnummer unter irgendeiner Art von Bezeichnung ab,
aber sie sind sich nicht einig, wie. Aperio speichert dein Wort, und jedes
Konto übersetzt es beim Hinausschreiben:

- **CardDAV / iCloud / Nextcloud** und **Google** behalten, was du getippt
  hast. Eine eigene Bezeichnung wie *Ferienhaus* übersteht den Umlauf
  unverändert.
- **Exchange** hat ein festes Vokabular und feste Plätze: vier Sprechnummern,
  eine Faxnummer und drei E-Mail-Adressen pro Kontakt. Eine Bezeichnung, für
  die es kein Wort hat, rutscht auf den nächsten freien Sprech-Platz – **die
  Nummer reist immer mit, nur das Wort kann ersetzt werden.** Eine fünfte
  Sprechnummer lässt sich gar nicht ablegen.
- **Outlook / Microsoft 365** hat drei Telefon-Sammlungen – eine Mobilnummer,
  private Nummern, geschäftliche Nummern. Eine zweite Nummer mit der
  Bezeichnung *mobil* landet bei den geschäftlichen, statt die erste zu
  verdrängen.
- Das **lokale Adressbuch** speichert alles genau so, wie du es getippt hast.

Für Webseiten gilt dasselbe: CardDAV und Google behalten beliebig viele,
Exchange und Outlook genau eine (bevorzugt eine mit der Bezeichnung
*dienstlich*, sonst die erste).

> **Jahrestag bei Outlook.** Microsoft-365-Kontakte haben ein Feld für den
> Geburtstag, aber **keines für einen Jahrestag**. Ein Jahrestag, den du an
> einem Outlook-Kontakt einträgst, hat dort keinen Ort und ist nach der
> nächsten Synchronisation wieder leer. Alle anderen Kontoarten behalten ihn.

## Geburtstage im Kalender

Jedes Adressbuch, in dem Geburtstage stehen, erscheint zusätzlich als
**Nur-Lese-Geburtstagskalender** in der Kalenderliste, den du wie jeden anderen
Kalender ein- und ausblenden kannst. Diese Einträge entstehen aus den Kontakten
selbst – dort gibt es nichts zu bearbeiten; ändere den Geburtstag am Kontakt,
und der Kalender zieht nach.

Einem Geburtstagskalender kannst du eigene **Standard-Erinnerungen** geben, um
ein paar Tage vorher gewarnt zu werden statt erst am Morgen selbst.

## Synchronisation und Datenschutz

Unter **Einstellungen → Kontakte** steuerst du:

- das **Sync-Intervall** und einen Knopf **Jetzt synchronisieren**;
- ob **Nur-Lese-Verzeichnisse** (Globale Adressliste, Vorgeschlagene Personen,
  Weitere Kontakte, Workspace-Verzeichnis) bei jedem Sync mitgezogen werden.
  Standardmäßig bleiben sie außen vor, weil sie sehr groß sein können – die
  Suche erreicht sie ohnehin, bei Bedarf;
- **Cache leeren**, was die Momentaufnahmen der externen Adressbücher im
  Arbeitsspeicher verwirft. Deine eigenen lokalen Kontakte bleiben unberührt;
  der nächste Sync holt den Rest wieder.

Aperio synchronisiert deine Kontakte **direkt mit den verbundenen Anbietern**
und hält Namen, Adressen, Nummern, Geburtstage und Organisationsfelder im
Arbeitsspeicher, damit Suche und Teilnehmerauswahl flüssig bleiben. Was ein
Anbieter selbst erhebt und wie lange er es aufbewahrt, regelt dessen eigene
Datenschutzerklärung; die Einstellungsseite verlinkt die von Google und
Microsoft, und für CardDAV-, iCloud- und Exchange-Server gilt die Erklärung des
jeweiligen Servers.

## Zusammenfassung

Du kannst Kontakte über mehrere Adressbücher hinweg führen, jede Telefonnummer,
E-Mail-Adresse und Webseite bezeichnen und Geburtstage im Kalender erscheinen
lassen. Im nächsten Kapitel lernst du die Ansichten kennen.
