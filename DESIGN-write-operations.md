# Design-Dokument: Vorgänge mit Schritten (Schreiben, das wartet, weitermacht und zurücknimmt)

**Status:** Recherchiert am 2026-10-19 (Erhebungen, drei Bauweisen, drei Gutachten, Plan, Kritik mit 28 Punkten). Richtung entschieden am 2026-10-23: ein eigener Bogen **nach PR 7, 9 und 10** (Toni). Die Detailfragen in §8 werden zu Beginn des Bogens gestellt.
**Bezug:** Entscheidungen 270-278 (PR 7: „ausgelastet“, „teilweise gespeichert“), 202-213 und 263-269 (Schrittläufe neuer Serien bei Google und Exchange), 106 (Schreibschutz pro Kalender), 112/113 (kein Merkbuch eigener Schreibvorgänge, Nachlesen), 243-253 (Verlust ansagen und fragen), TODO B9, B13, B14.
**Nach Freigabe:** in `DESIGN.md` einarbeiten und dieses Dokument entfernen.

---

## 1. Die Idee (Toni)

Bleibt ein Schreibvorgang heute hängen (Anbieter drosselt, Internet weg, eine Speicherung aus mehreren Anfragen bricht in der Mitte ab), muss der Nutzer alles neu machen. Stattdessen:

- Adapter melden **Vorgänge** beim Kern an. Ein Vorgang führt mehrere **Schritte** aus, jeder meldet Erfolg oder Misserfolg, und es gibt eine **Rücknahme**.
- Der Zustand ist **speicherbar**, damit der Kern später **von selbst weitermacht**: beim nächsten Aktualisieren, nach einer Zeit, beim Aufwecken im Hintergrund.
- Jeder Vorgang trägt das Element im **Zielzustand** mit. Die Oberfläche zeigt es mit dem Hinweis, dass die Werte noch nicht beim Anbieter stehen, und eine weitere Änderung desselben Elements **hängt sich hinter** den laufenden Vorgang, statt mit ihm zu kollidieren.

In verteilten Systemen heißt das Muster *Saga*: Schritte mit Gegenbewegung, ein Ablaufplaner, ein gespeichertes Journal.

## 2. Urteil und Grenzen

**Machbar, und es passt zu Aperio.** Das Schwierigste ist schon da: die Schrittläufe neuer Serien (`crates/adapter-google/src/series_create.rs`, `crates/adapter-ews/src/series_create.rs`), die Begriffe „nichts geändert / steht vielleicht / teilweise gespeichert“ (`WriteRefusal`, `WriteLeftover`, `Error::never_landed`, `Error::after_landed`), die eigene Kennung bei Google-Terminen und die UID mit `If-None-Match` bei CalDAV, Migrationen, die nur anfügen, und Aktualisierungs-Auslöser auf beiden Geräten. Weil vor dem Release keine Abwärtskompatibilität nötig ist, dürfen Plugin-ABI, Traits und Fehlerweg ersetzt werden; das macht es bezahlbar.

**Grenzen, die das Design nennt statt versteckt:**

1. Ein Anlegen mit verlorener Antwort lässt sich nur dort fortsetzen, wo Aperio die Kennung selbst wählt: heute Google-Termine und CalDAV. Bei Exchange, Graph, Google Tasks, Google People, Vikunja, Todoist und Webex bleibt es „steht vielleicht schon, du entscheidest“. Aperio sucht ein Element nie über seinen Titel.
2. Verschickte Post (Einladungen, Absagen) lässt sich nicht zurückholen. Manche Rücknahmen verlieren Daten (Exchange-Löschungen von Vorkommen, Ausnahmen, die das Umschreiben einer Serie verwirft, Graph `/cancel`): Sie werden vorher gefragt, nie still gemacht.
3. Ein Handy lässt sich nicht wecken, wenn das Netz zurückkommt, unter iOS gar nicht. Ein wartender Vorgang geht beim nächsten Öffnen, Abgleich oder Hintergrundfenster weiter, das kann Stunden dauern. Das iOS-Fenster ist etwa 30 s lang.
4. Das Journal bleibt auf dem Gerät, auf dem die Änderung entstand. Andere Geräte zeigen den alten Stand, bis sie ankommt; wiederholt man die Änderung dort, kann sie doppelt entstehen.
5. Der Umfang ist groß: nach der Kritik etwa **25 bis 30 PRs** mit Live-Runden je Anbieter. Der erste spürbare Gewinn (Einzelspeicherungen warten statt zu scheitern) kommt nach etwa vier PRs; Speicherungen aus mehreren Schritten setzen erst in der zweiten Hälfte fort.

## 3. Bauweise

Gewählt (einstimmig von den Gutachten): **Adapter erklären Vorgänge aus Schritten, ein Ablaufplaner in `host-core` führt sie aus.** Dazu Anleihen aus der einfachen Warteschlange (anfangs wartet nur, was sicher nichts geschrieben hat; Schlüssel einmal ziehen; vorläufige Kennungen) und aus dem Mischentwurf (kein Fremdschlüssel auf das Konto; Basis, Ziel und geänderte Felder in eigener Tabelle; Zeiten als Epoch-Millisekunden; Befehle geben ein `SaveOutcome` zurück statt zu werfen).

### 3.1 Regeln einmal im Kern (`cal_core::operation`, rein und WASM-tauglich, liest keine Uhr)

- **IntentKind:** Einzelschreibvorgänge (Termin, Aufgabe, Kontakt, Kontaktgruppe: anlegen, ändern, löschen, Ausnahme, Foto) und zusammengesetzte (Verschieben, Teilen einer Serie, Vorkommen herauslösen, Anlegen mit Unteraufgaben, Status-Kaskade, Tagesstart-Sammelaktion, Folge-Instanz einer Wiederholung, Meeting anhängen/lösen, Gruppen-Übertrag). Die Kritik verlangt, **jede** Schreibstelle der Traits aufzuzählen (auch Einladungsantwort, Abschnitte, Listen/Kalender anlegen und löschen, Mitglieder, Umbenennen) und jede entweder als Art zu führen oder ausdrücklich als „wartet nie“ zu benennen.
- **Intent** mit den Einmal-Schaltern, jetzt bewusst gespeichert (`send_invitations`, `accepts_exception_loss`, `keep_attendees`, `keep_fields`, `clear_attendees`, `truncate_tail_overrides`).
- **Target** je Element: Basis (Zeile und etag, beim Anlegen keine), Zielzeile, geänderte Felder, Wirkung (anlegen, ändern, löschen, hinein-, herausverschieben).
- **Schrittklassen:** lesen, rücknehmbar, Drehpunkt (verschickt Post oder ist unumkehrbar), wiederholbar. **Kosten der Rücknahme:** sauber, laut (verschickt Absagen), verlustreich, keine. **Anker:** eigener Schlüssel, Nachlesen, keiner.
- **StepOutcome:** gelandet, warten (ausgelastet mit Wartezeit, offline, Anmeldung, Taktung), unbekannt (gesendet, Antwort verloren), abgelehnt, braucht den Nutzer (Konflikt, Zustimmung). **CheckOutcome:** gelandet, nicht gelandet, nicht entscheidbar.
- `next_action`, `backoff` (30 s, verdoppelt, höchstens 1 h; eine Wartezeit des Anbieters gewinnt immer), `fold_or_chain`, `rebase` (Termine über `event_diff`; `task_diff` und `contact_diff` neu), `pending_mark` (nach TypeScript erzeugt).
- **WriteError** ersetzt die Wort-Marken am Anfang einer Meldung: Klasse, `landed: never | maybe | partly{row}`, Ablehnung, Rest, Wartezeit, Reichweite der Auslastung, Schritt, eingeladen, Detail. `WriteRefusal` und `WriteLeftover` aus PR 7 bleiben als Aufzählungen; „nie losgeschickt“ (`Unreached`) wird eine eigene Klasse.

### 3.2 Plugin-Grenze (ABI-Revision 5, Mindest-ABI 5)

- Die Fehler-Nutzlast jeder Schnittstelle ist das `WriteError`-JSON. Es geht unverändert durch `CommandError` (Desktop) und `StoreError` (Handy); das schließt B9, braucht aber eigene Swift- und Kotlin-Arbeit und einen Gerätetest.
- Lesen über die Kennung kommt dazu: `get_event(id)`, `get_task(list, id)`, `get_contact(id)` (auch für den Host-eigenen lokalen Adapter und den Gerätekalender).
- Ändern und Löschen bekommen eine Vorbedingung (etag oder ChangeKey). Google kennt `If-Match` und antwortet 412; Aperio schickt es heute nur nicht.
- Neue Familie `OperationsVtable`: `op_plan` (sendet nichts, zieht die Schlüssel), `op_step`, `op_check` (liest nur über Schlüssel oder Kennung; ohne Anker zwingend „nicht entscheidbar“), `op_compensate`, `op_upgrade`. Ein Block `operations` im Manifest ersetzt `recurrence.deletions_in_steps`.
- Arten, die ein Plugin nicht erklärt, wickelt der Host als einschrittige Vorgänge um den heutigen Trait-Aufruf.
- SDK-Regeln mit Prüfgerüst: höchstens ein nicht-idempotentes Schreiben je Schritt; nie länger schlafen als die Taktung, sondern „warten“ zurückgeben; Plan-Daten ohne Geheimnisse und ohne URLs, die selbst berechtigen; Fehlerinjektion, die die Antwort nach dem Landen verliert.

### 3.3 Speicherung

Eine Migration fügt nur Tabellen an (die Datenbanken auf Tonis Geräten migrieren ohne Datenverlust): `write_operations`, `write_operation_steps`, `write_operation_targets`. Kein Fremdschlüssel aufs Konto; wird ein Konto entfernt (lokal oder über den Abgleich), macht ein Haken in `host-core` die offenen Vorgänge vorher zu angesagten „aufgegebenen“. Nicht auf der Abgleich-Liste, nie im Ereignisprotokoll: Zwei Geräte führen nie denselben Vorgang aus. Fotos als BLOB.

### 3.4 Ablaufplaner (einer je Prozess, in `host-core`)

- Je Konto eine serielle Spur, Spuren parallel; innerhalb einer Kette strikt der Reihe nach.
- Ein Auslastungs-Tor je Konto, das Ablaufplaner und Cache-Aktualisierung teilen: Die Drossel-Regel gibt es einmal.
- Vor jedem Schritt „Versuch offen“ festschreiben, dann senden, dann das Ergebnis mit dem neuen Plan festschreiben. Beim Weitermachen wird ein offener Versuch zuerst geprüft: gelandet geht weiter, nicht gelandet sendet neu, nicht entscheidbar fragt den Nutzer.
- Vor einem fortgesetzten Ändern oder Löschen: die Vorbedingung senden, wo der Anbieter sie kennt; sonst über die Kennung lesen und das etag mit der Basis vergleichen (ein kleines Fenster bleibt, das sagt das Dokument dort).
- Auslöser: die Speicherung selbst; der App-Start; der Anfang jeder Aktualisierungsrunde; am Desktop ein Zeitgeber; am Handy Vordergrund, Abgleich, ein eigener Aufruf vor der Hintergrund-Aktualisierung und die Hintergrundzeit beim Verlassen (229). Am Handy auf einer eigenen Warteschlange (B13, B14).

### 3.5 Zielzustand, von Anfang bis Ende (Tonis Vorschlag)

- **Gespeichert:** das ganze Element, wie es sein soll, dazu die Basis, von der die erste Änderung ausging.
- **Zusammengeführt** nach einer Kernregel: Solange der letzte Vorgang der Kette noch nichts gesendet hat, wird die neue Änderung **hineingefaltet** (Anlegen und Ändern bleibt Anlegen; Anlegen und Löschen verwirft beides und sagt es an; Ändern und Löschen wird Löschen). Sendet er schon, **hängt** sich die neue Änderung dahinter, höchstens ein Nachfolger, alles Weitere faltet in ihn. Wird der Vorgänger abgelehnt oder zurückgenommen, wartet der Nachfolger als Frage.
- **Gezeigt** über **einen** Lesepfad in `host-core` (die doppelten Lesepfade von Desktop und Handy werden vorher zusammengelegt): Ein Ändern zeigt das Ziel, ein Anlegen erscheint unter einer vorläufigen Kennung, ein Verschieben im Zielbehälter. Auch Erinnerungen, Suche und Serienzeilen lesen durch diesen Pfad.
- **Angesagt** auf beiden Geräten: ein Zeilenzusatz („, noch nicht gesendet“, „, wartet auf deine Entscheidung“, „, Änderung nicht gespeichert“) mit sichtbarer Markierung daneben, nie nur Farbe; ein Hinweis im Editor; ein Satz beim Speichern („Auf diesem Gerät gespeichert. Aperio schickt es an … , sobald … antwortet.“); höfliche Sammelansagen beim Landen, bestimmte bei Fragen und endgültiger Ablehnung (mit Benachrichtigung im Hintergrund); eine Liste „Wartende Änderungen“ mit Jetzt versuchen, Abbrechen, Öffnen und Entscheiden.
- **Endet** mit dem Landen (die Antwort des Schreibens oder ein Nachlesen, das das Ziel schon zeigt, gibt die Überlagerung frei), mit der Rücknahme (der Anbieterstand zeigt sich wieder, das Ziel bleibt zum erneuten Speichern) oder mit der endgültigen Ablehnung (die Zeile zeigt den Anbieterstand mit „, Änderung nicht gespeichert“; der Bildschirmleser liest nie ungespeicherte Werte als echt).
- **Anbieterstand geändert:** Nur beim Anbieter geänderte Felder werden ins Ziel übernommen (der Editor sagt das), nur vom Nutzer geänderte werden gesendet, dasselbe Feld auf beiden Seiten wird gefragt (meins, deins, beide). Für ein Löschen heißt „beim Anbieter weg“: erledigt.

### 3.6 Wiederholen und Zurücknehmen

Ausgelastet, offline und Anmeldung warten; Versuche allein geben dabei nie auf. Unbekanntes wird geprüft. Abgelehnt vor einem Drehpunkt: rückwärts zurücknehmen, jede Rücknahme selbst ein Schritt, der warten kann (sauber ohne Frage; laut nur während der Speicherung selbst, später mit Frage; verlustreich immer mit Frage). Nach einem Drehpunkt nur noch vorwärts. Ein Schritt, der Post verschickt, geht nie verspätet ohne Zustimmung hinaus (§8).

### 3.7 Zusammengesetzte Vorgänge wandern einmal in den Kern

Verschieben über Kalender und Listen (heute doppelt in Desktop und Handy, mit stummem Duplikat), die Folge-Instanz einer Wiederholung, das Teilen einer Serie, das Herauslösen eines Vorkommens, Aufgaben mit Kindern verschieben, Anlegen mit Unteraufgaben, Status-Kaskaden, Tagesstart-Sammelaktionen, der Gruppen-Übertrag, Meetings. Nebentabellen (Farbe, Gruppe, eigene Erinnerungen, Meeting-Bindung) ziehen als lokale Schritte über die erzeugte Kennung um, nie über den Titel.

### 3.8 Über App-Updates hinweg

`intent_json`, `plan_json`, `target_json` und `base_json` tragen je eine Version und werden beim Start in Rust hochgestuft; ein CI-Fixture je Version muss angenommen werden. Ein Vorgang, der nicht mehr verstanden wird, wartet als Frage mit lesbarem Ziel, er wird nie verworfen.

## 4. Korrekturen aus der Kritik (gehören in die Schritte)

1. **Schreibschutz pro Element statt pro Kalender** für wartende Vorgänge; ein Vorgang zählt nur als „im Flug“, solange ein Schritt wirklich sendet. Sonst schaltet ein wartender Vorgang Entscheidung 106 stundenlang für den ganzen Kalender ab.
2. **Stabile Identität:** Exchange-Kennungen tragen den ChangeKey und ändern sich mit jeder Änderung, Graph-Kennungen beim Verschieben. Ketten, Überlagerung und Journal hängen an einer Identität, die der Adapter erklärt; ein Nachfolger setzt auf der gelandeten Zeile auf, nicht auf dem Ziel. Ein fortgesetztes Exchange-Schreiben schickt den ChangeKey der Basis.
3. **„Nie losgeschickt“ nur bei Verbindungsfehlern**, nicht bei einem fehlerhaft gebauten Aufruf (der ist dauerhaft), und nie, wenn ein früherer Versuch derselben Anfrage schon gesendet war (CalDAV-Wiederholung). Ein teilweise gelandeter Vorgang hat Vorrang vor seiner Klasse.
4. **403 aufteilen:** Eine Berechtigung, die das Konto nie bekommt, darf nicht auf „Anmeldung“ warten. Ein aufgebrauchtes Tageskontingent dagegen soll bis zum Zurücksetzen warten statt zu scheitern.
5. **Abhaken mit Wiederholung:** Die Folge-Instanz läuft heute nur im Host-Befehl. Wartet das Abhaken, muss die Folge-Instanz als abhängiger Schritt mitwarten; sonst warten wiederkehrende externe Aufgaben bis dahin gar nicht.
6. **Abläufe der Oberfläche** (Teilen, Herauslösen, Verschieben/Kopieren, Gruppen-Übertrag, Unteraufgaben, Meetings) dürfen bis zu ihrem Umzug in den Kern kein „wartet“ bekommen, sonst bleibt eine halbe Folge zurück.
7. **Abhängigkeiten im Bogen:** Der fortsetzbare Google-Serienlauf braucht die Überlagerung für Termine und Anlagen; das alte Manifest-Feld wird gelesen, bis Exchange umgestellt ist.
8. **Es gibt kein 5-s-Budget**, und ein Zeitlimit des Hosts beendet keinen laufenden Plugin-Aufruf. Ein Register „Schritt läuft“ je Vorgang sperrt jedes Weitermachen, bis der Aufruf wirklich zurückkam.
9. **Handy-Grenzen:** Vorgänge bekommen einen eigenen Teil des Hintergrundfensters mit kürzeren Zeitlimits je Schritt; Schritte des Rust-Planers brauchen auf iOS eine eigene Hintergrundzeit (eine native Schnittstelle). Ein beendetes Handy macht erst beim Öffnen weiter.
10. **Ein offenes Handy** wiederholt zur fälligen Zeit auch ohne eingerichteten Geräteabgleich.
11. **Zwei Ablaufplaner auf einem Journal** (zweiter Start, kopierter portabler Datenordner): ein Mietvertrag in SQLite und die Geräte-Kennung je Vorgang. Ein fremder Vorgang läuft nicht, er wird gefragt.
12. **Vorläufige Kennungen** dürfen nie in abgeglichene Nebentabellen oder ins Ereignisprotokoll; Nebenänderungen an einem wartenden Element werden Schritte des Vorgangs.
13. **Lokale Schritte und ihr Abgleich-Ereignis:** Das Ereignis landet in einer SQLite-Zeile in derselben Transaktion und wird von dort idempotent ins Protokoll geschoben.
14. **Gespeicherte Zeilen versionieren** (siehe 3.8).
15. **Landen ohne Rückfall:** Nachlesen direkt nach dem Schreiben kann veraltet antworten (107/108). Die Antwort des Schreibens gilt, wo es eine gibt; sonst ein Nachlesen, das das Ziel zeigt, mit einer je Anbieter gemessenen Wartezeit (Live-Runde 7 vor dem ersten Warten).
16. **Rebase rückt die Basis vor**, sonst endet CalDAV in einer 412-Schleife; für ein Löschen ist „weg“ erledigt.
17. **Vorbedingungen echt senden:** `If-Match` bei Google, Basis-ChangeKey bei Exchange; Lesen-und-Vergleichen nur, wo es nichts anderes gibt.
18. **Graph ImmutableId** kommt vor dem ersten wartenden Graph-Termin, als eigener PR mit Umzug der Nebentabellen, nicht als bloßer Header.
19. **Verschieben in eine andere Liste:** Zuständige und Abschnitt nur behalten, wo es sie im Ziel gibt; sonst mit Satz weglassen; die alte übergeordnete Aufgabe neu zuordnen oder leeren.
20. **Der gemeinsame Lesepfad verändert Verhalten** (Erinnerungen lesen heute live, Desktop und Handy blenden Abgesagtes verschieden aus): als Verhaltensänderung behandeln und testen.
21. **Einladungsantworten** (verschicken Post an den Organisator) brauchen eine Art und dieselbe Spät-Post-Regel.
22. **Widget und Siri:** Ein „Umschalten“ wird vor der Übergabe in seinen Zielstatus aufgelöst; Siri-Anlagen erst übergeben, wenn Anlagen warten dürfen und die Antwort sicher „nie losgeschickt“ war.
23. **Ein stabiler Zeilenschlüssel** fehlt heute (Kennung und ChangeKey ändern sich); er kommt vor den wartenden Anlagen, sonst springt der Fokus.
24. **Ansagen nach dem Hintergrund:** ein Feld „angesagt“, und beim nächsten Vordergrund werden noch nicht angesagte Ergebnisse einmal höflich gesagt.
25. **B9 schließen** braucht Swift- und Kotlin-Arbeit und einen Gerätetest (↻).
26. **Konto-, Behälter- und Plugin-Änderungen** zwischen Einreihen und Weitermachen (geänderte Server-Adresse, gelöschter Kalender, entferntes Plugin) machen den Vorgang zu einer Frage.
27. **Gespeicherte Fehlertexte** nur bereinigt (keine URL-Abfragen, keine Antwortkörper).
28. **Umfang** neu geschätzt auf 25 bis 30 PRs; O1 und O4 je aufteilen; Plugin- und Nutzer-Doku in jede Checkliste.

## 5. Der Bogen (vorläufig, wird zu Beginn mit §4 neu geschnitten)

- **O1** Schreib-Ergebnisse als Daten (`WriteError`, ABI 5), alle Adapter melden Folgeschritte nach einem Anlegen als `partly{row}`, Desktop- und Handy-Oberflächen lesen sie; hier kommt „Editor bleibt auf dem Angelegten“ für abgelehnte oder verlorene Folgeschritte. Laut Kritik mehrere PRs.
- **O2** Ein Lesepfad in `host-core` (Überlagerungs-Haken, noch leer).
- **O3** Ehrliches Verschieben, einmal im Kern; Tagesstart-Sammelaktionen mit Satz.
- **O4** Wartende Änderungen für Aufgaben: Journal, Ablaufplaner, Überlagerung, Liste „Wartende Änderungen“. Erster spürbarer Gewinn. Laut Kritik zwei PRs.
- **O5** Wartende Änderungen für Termine und Kontakte, mit Konflikt-Dialog.
- **O6** Auch Anlagen warten: einmal gezogene Schlüssel, vorläufige Kennungen.
- **O7** Schritt-Vorgänge in der Plugin-ABI; Googles Serienlauf macht weiter.
- **O8** Exchanges Serienlauf macht weiter.
- **O9** Zusammengesetzte Host-Vorgänge machen weiter: Verschieben, Folge-Instanz.
- **O10** Abläufe der Oberfläche ziehen in den Kern (Teilen, Herauslösen, Unteraufgaben, Kaskaden, Übertrag).
- **O11** Folgeschritte der Adapter als Plugin-Vorgänge, je Adapter ein PR (Google, Exchange, Graph mit ImmutableId, Vikunja, Todoist nach Prüfung seiner API).
- **O12** Meetings als systemübergreifende Vorgänge.
- **O13** Markierungen beim Anbieter machen verlorene Anlagen entscheidbar (je Anbieter nach Messung).

## 6. Was PR 7 schon tut und was es bis zum Bogen tut

PR 7 bleibt, wie gebaut: `Error::provider_busy`, `Error::never_landed`, `Error::after_landed`, `WriteRefusal` und `WriteLeftover` (sie werden in O1 zu Teilen von `WriteError`; Sätze und Schlüssel bleiben), Googles Drossel-Erkennung, Durchläufe, die bei Auslastung stoppen, die Taktung der Aktualisierung und die Sätze auf allen Oberflächen. Neue Oberflächen lesen Fehler nur über die geteilten Helfer (`writeNeverLanded`, `eventWriteFailureReason`, `writeErrorMessage`), nie über eigene Marken-Erkennung; dann tauscht O1 eine Stelle.

**Anlegen mit gescheitertem Folgeschritt, bis der Bogen kommt (Toni, 2026-10-23: „ehrlicher Abschluss“):**

- Google-Gruppe, deren Mitglieder nicht ankamen: Der Editor schließt mit einem Hinweis, der sich nur schließen lässt („Die Gruppe wurde angelegt, aber ihre Mitglieder nicht hinzugefügt …“), und die Daten laden neu. Kein zweites Anlegen.
- Google-Foto: wird weiter verschluckt; der Editor vergleicht `has_photo` der zurückgegebenen Zeile mit dem gesendeten Foto und sagt „ohne Foto gespeichert“.
- Unteraufgaben-Entwürfe: Der Editor bleibt auf der angelegten Aufgabe, behält die noch nicht angelegten Entwürfe, und Speichern legt nur diese an (Tonis ursprünglicher Wunsch, ohne Kern-Umbau).
- Vikunja (Zuständige), Todoist (Abschließen), Graph (Foto, Erweiterung): Diese Folgeschritte laufen über `Error::after_landed` und nehmen denselben Weg wie die Google-Gruppe.

## 7. Risiken

- Entscheidung 112: Die Überlagerung darf das verworfene Merkbuch eigener Schreibvorgänge nicht nachbauen; sie gilt nur für nicht gelandete Vorgänge und endet mit dem Landen (113). Die damaligen Einwände sind vor O4 nachzuprüfen.
- Doppelte Schreibvorgänge durch falsche Einordnung, falsche Prüfungen oder zwei Ablaufplaner (§4 Punkte 3, 8, 11).
- Überschreiben neuerer Änderungen ohne Vorbedingung (§4 Punkt 17).
- Verspätete Post (§8).
- Plan-Zustand über App-Updates (§3.8).
- Geheimnisse in Plan-Daten und gespeicherten Fehlertexten (§4 Punkt 27).
- Parität: Desktop und Handy tragen während des Umzugs zwei Wege; jeder PR hält beide im Gleichschritt.
- Todoist nutzt womöglich schon abgekündigte Schnittstellen; vor seinem O11-Teil prüfen.

## 8. Offene Fragen an Toni (zu Beginn des Bogens)

Aus dem Plan:

1. Ein wartendes Löschen: bleibt das Element sichtbar („, wird gelöscht“), bis es ankommt, oder verschwindet es sofort? (Vorschlag: sichtbar bleiben; sonst ändern sich angesagte Zahlen vor der Zusage, und bei endgültiger Ablehnung tauchte es wortlos wieder auf.)
2. Neue Serie mit Löschungen, der Anbieter wird mittendrin ausgelastet: pausieren und später fertig machen, oder wie heute zurücknehmen (203)? (Vorschlag: pausieren, wo noch niemand eingeladen ist, also bei Exchange immer, bei Google ohne Eingeladene; bei Google mit Eingeladenen jetzt zurücknehmen. Dazu eine Live-Runde, ob Google still anlegen und nach dem Beweis einladen kann.)
3. Wie spät darf ein wartender Schritt noch Post verschicken (Einladungen, Absagen), ohne zu fragen? (Vorschlag: innerhalb von 24 Stunden nach dem Speichern und vor Beginn des Termins; sonst wartet er als Frage, mit Benachrichtigung.)
4. Wann hört eine Änderung, die weiter wartet, von selbst auf? (Vorschlag: nach 7 Tagen fragen, weiter versuchen oder verwerfen.)
5. Sollen andere Geräte erfahren, dass auf diesem Gerät etwas wartet? (Vorschlag: vorerst nicht; nach O5 über einen reinen Hinweis entscheiden, nie über den Vorgang selbst.)
6. Den Desktop beenden, während etwas wartet (Schließen in den Infobereich ist aus): still beenden oder einmal fragen? (Vorschlag: einmal fragen, nur wenn etwas wartet.)

Aus der Kritik:

7. Ein 403, über das das Konto nie hinwegkommt (keine Schreibrechte, nur der Organisator darf), oder ein aufgebrauchtes Tageskontingent: warten (das Kontingent bis zum Zurücksetzen) oder sofort ablehnen, die Änderung in der Liste behalten?
8. Wartende Änderungen an Gerät und Installation binden, damit ein kopierter portabler Datenordner oder eine zweite Desktop-Instanz sie nie erneut sendet (sondern fragt)?
9. Soll ein offenes Handy wartende Änderungen auch ohne eingerichteten Geräteabgleich nach eigener Uhr wiederholen, wie der Desktop?
10. Bis Teilen, Herauslösen, Verschieben/Kopieren, Gruppen-Übertrag und Unteraufgaben in den Kern gezogen sind (O10): ihre Schreibvorgänge vom Warten ausnehmen (sie scheitern wie heute), damit nie eine halbe Folge zurückbleibt?
11. Eine wiederkehrende Aufgabe bei Google, Todoist oder Vikunja abhaken, während der Anbieter ausgelastet ist: wartet das Abhaken zusammen mit dem Anlegen der nächsten Instanz, oder warten solche Aufgaben erst, wenn das gebaut ist?
12. Einladungsantworten (Zusagen und Absagen schicken Post an den Organisator): warten wie andere Änderungen, unter derselben Spät-Post-Regel?
13. Darf „Jetzt versuchen“ eine Wartezeit übergehen, die der Anbieter selbst genannt hat, oder nur Aperios eigene Pause?
14. Soll das Handy beim Verlassen sagen, dass etwas wartet, da eine weggewischte iOS-App bis zum nächsten Öffnen keine Hintergrundzeit bekommt?
15. Soll der Schreibschutz pro Element den heutigen pro Kalender für wartende Änderungen ersetzen, damit andere Termine im selben Kalender den Schutz aus 106 behalten?
