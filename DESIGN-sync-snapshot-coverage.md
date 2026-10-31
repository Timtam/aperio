# Schnappschuss nennt, was er enthält

Stand 2026-10-30 (Befund 2026-10-27). Befund von Toni: Ein neues Gerät (Laptop) trat dem Sync
bei, übernahm den Schnappschuss vom 19.10. und alle Konten. Aufgaben, die seit
dem 19.10. erledigt oder neu angelegt wurden, fehlten oder standen weiter als
überfällig da.

## Befund (Protokoll des Laptops, Sync-Ordner auf dem NAS)

- Schnappschuss `snapshot_timestamp` = 2026-10-19T16:07:24Z, erstellt vom
  iPhone beim Start einer Sitzung (sein eigener Sitzungsstempel war der
  Inhalts-Horizont `max(own_newest_log, cursor)`).
- `accept_remote` setzte den Cursor auf diesen Zeitpunkt und holte nur
  Dateien mit späterem Namen: genau 5 (22.10. bis 27.10. früh), 13
  Änderungen. Spätere Runden: `fetched=0`.
- Die Desktop-Sitzung `2026-10-19T07-58-29Z_ab0d…` begann vor dem
  Schnappschuss und lief bis zum 22.10. Was danach in sie geschrieben wurde,
  steht in einer Datei mit „altem“ Namen. Der Laptop übersprang sie.

## Ursache

Der Abruf entscheidet nach dem **Namen** einer Datei (Sitzungsbeginn), der
Schnappschuss deckt aber **Ereignisse** ab. Eine Sitzungsdatei wächst nach
ihrem Namen weiter:

1. **Beitritt und Wiederaufnahme** (`accept_remote`, `resume_from_stale`):
   Cursor = Schnappschusszeit, keine bekannten Längen. Eine fremde Datei mit
   Namen vor dem Schnappschuss, die danach wuchs, wird nie geholt.
2. **Gewöhnliche Runde**: `DeviceCursor::wants_sized` holt eine Datei nur,
   wenn ihr Name nach dem Cursor liegt oder sie schon angewendet wurde und
   gewachsen ist. Eine Datei, die ein Gerät **verspätet** hochlädt (offline
   geschrieben) und deren Name vor dem Cursor liegt, holt kein anderes Gerät
   je („Unknown filename … never refetched“).

Alles erneut abzuspielen geht nicht: Der Anwender führt pro Zeile nach
`updated_at` zusammen. Ein altes, schon im Schnappschuss enthaltenes Ereignis
mit einem inzwischen geänderten Wert würde als Konflikt gemeldet.

## Entscheidungen (Toni, 2026-10-27)

- Der Schnappschuss nennt die **Kennungen der Ereignisse**, die er schon
  enthält, soweit sie noch in Protokolldateien liegen.
- Eigener PR, vor PR 7 Runde 15.
- Der Laptop wird nicht von Hand neu aufgesetzt, die Korrektur heilt ihn.

## Entwurf

### A. Der Compactor schreibt die Abdeckung

Vor dem Bau des Schnappschusses holt der Compactor alle Protokolldateien
(`DeviceCursor::epoch()`; das tut er heute erst danach, fürs Aufräumen, und
nutzt denselben Abruf dann für beides). Abgedeckt ist ein Ereignis in einer
dieser Dateien, wenn es

- von diesem Gerät stammt (seine Datenbank enthält jede eigene Änderung), oder
- in `sync_applied_events` steht (angewendet).

Diese Kennungen kommen als `covered_event_ids` in den Rumpf des Schnappschusses
(`AperioSnapshotBody`, `serde(default)`). Ein Ereignis, das in einer Datei
steht, aber noch nicht angewendet ist (gewachsen seit dem letzten Abruf), ist
nicht abgedeckt: Ein neues Gerät wendet es an, der Compactor selbst holt es mit
der nächsten Runde nach.

### B. Beitreten, Wiederaufnehmen und Nachholen: ein Durchgang

`accept_remote`, `resume_from_stale` und das einmalige Nachholen in der Runde
enden im selben Durchgang (`OnboardingService::apply_every_log`):

1. Beitritt und Wiederaufnahme wenden zuerst den Schnappschuss an (Upsert wie
   bisher). Nennt er Kennungen, werden sie im selben Zug als angewendet
   eingetragen.
2. **Alle** vorhandenen Protokolldateien holen, eigene eingeschlossen, dazu
   die eigenen noch nicht hochgeladenen (`pending/`). Jede Änderung zählt
   einmal (nach Kennung).
3. Alle Änderungen **gemeinsam** nach Zeit anwenden, nicht Datei für Datei:
   Sitzungen verschiedener Geräte überlappen sich.
4. Eine Änderung gilt nur, wenn sie neuer ist als ihre Zeile (siehe E), ohne
   Konfliktmeldung. Schon Angewendetes wird erneut gewogen. So bekommt eine
   Zeile, die ein Schnappschuss zurückgesetzt hat, ihre neuere Änderung zurück.
5. Längen der fremden Dateien merken. Eine abgerissen gelesene (die letzte
   Zeile ohne Zeilenende, alles davor lesbar) nicht: Dann ist der Durchgang
   unvollständig, und das nächste Nachholen nimmt sie wieder, nach seinen
   Regeln statt denen einer Runde. Ist sie beim nächsten Mal in derselben
   Größe wieder so, schreibt niemand mehr daran: Ihre Länge wird gemerkt.
   Das gilt auch für eigene Dateien: Der Merker hält sie, genannt werden nur
   fremde (F); eine eigene, oben für immer abgerissene, hielt ihr Gerät sonst
   für immer vom Genau-Sein ab (Prüfrunde 7). Auch eine gewöhnliche Runde
   merkt eine abgerissen gelesene Datei mit ihrer Größe: Findet die nächste
   sie gleich, holt sie sie nicht mehr in jeder Runde. Sie merkt sie in einem
   eigenen Merker (`sync.cursor.tornSeen`), den das Nachholen nicht liest:
   Eine Sichtung Minuten zuvor sagt nicht, dass niemand mehr daran schreibt
   (Prüfrunde 8).
   Eine Datei mit einer Zeile, die dieser Build nicht kennt (ein neuerer
   schrieb sie), ist kein abgerissenes Lesen, in keiner Größe: Ihre Länge wird
   gemerkt, sie hält kein Gerät vom Genau-Sein ab, auch in gewöhnlichen
   Runden wird sie nicht jedes Mal neu geholt, und das Gerät nennt sie in
   seinem Eintrag in `meta.json` (`unreadable`); kein Compactor löscht sie,
   solange ein aktives Gerät sie nennt (F). Der Merker
   (`sync.cursor.unreadableLogs`) trägt den Parser, der es versucht hat
   (`sync_core::PARSER_FINGERPRINT`, ein Hash von Ereigniskatalog und Leser):
   Die App-Version bleibt über Builds gleich und taugt dafür nicht. Hat ein
   Update den Parser geändert, vergisst die Runde die gemerkte Länge der
   Datei, und der nächste Abruf liest sie wieder; erst dann trägt der Merker
   den neuen Parser (Prüfrunde 7). Bis dahin ergänzt eine Runde ihn unter dem
   alten, statt ihn zu ersetzen (Prüfrunde 8). Eine unlesbare Datei, deren Name
   sich nicht merken ließ, bekommt auch keine Länge, und der Durchgang ist
   nicht vollständig: Die nächste Runde holt und nennt sie wieder (Prüfrunden 9
   und 10). Ein Merker, der sich nicht lesen
   lässt, lässt das Gerät einmal nachholen (Prüfrunde 5). Cursor = Maximum
   aus bisherigem Cursor (beim Beitritt und bei der Wiederaufnahme:
   Schnappschuss) und neuester fremder Datei, nie zurück.
6. Danach ist das Gerät **genau** (`sync.cursor.coverageExactSince`), wenn jede
   Datei lesbar war.

Erst holen, dann ändern: Ein abgebrochener Abruf lässt den Bestand unberührt,
auch Uhren und das „genau“-Zeichen. Die Wiederaufnahme nimmt ein früheres
„genau“ zurück und setzt es nur neu, wenn jede Datei lesbar war.
Das löst auch, was „Ursache“ gegen das Wiederabspielen einwandte: Es geschieht
still, und nur Neueres gewinnt.

### C. Gewöhnliche Runden

- **Einmal nachholen.** Ein Gerät, das nicht genau ist, holt in der Runde
  einmal alles nach (B). Das betrifft jedes Gerät, das vor dieser Korrektur
  beigetreten oder wiederaufgenommen ist, und eines nach „Neu beginnen“.
  Schlägt es fehl oder bleibt es unvollständig, versucht es eine Runde
  frühestens eine Stunde später wieder (`sync.cursor.catchUpTriedAt`): Es holt
  jede Datei. Die Runde selbst läuft weiter.
- **Unbekanntes holen.** Ein genaues Gerät holt auch eine fremde Datei, deren
  Name nicht nach dem Cursor liegt und deren Länge es nicht kennt
  (`DeviceCursor::fetch_unknown`), ohne Untergrenze. Das ist eine verspätet
  hochgeladene Sitzung, oder eine nach dem Aufräumen erneut hochgeladene mit
  Namen unter `gc_horizon`. Ein nicht genaues Gerät behält die alte Regel, bis
  es nachgeholt hat.
- **Was ein Gerät noch hochladen kann, löscht niemand** (Prüfrunden 5 und
  6). Jedes Gerät nennt in seinem Eintrag in `meta.json` die Dateien in
  seinem `pending/` (`pending`), bevor eine davon hochgeht; kein Compactor
  löscht eine Datei, die ein Gerät so nennt. Der Push lädt dann jede
  ausstehende Datei einmal je App-Start ganz hoch: Eine gelöschte kommt so
  nie zurück, und eine entfernte Kopie, die ein abgebrochener Push halb
  geschrieben hat, ist wieder ganz. Die Runde nennt schon zu Beginn, vor
  Wiederaufnahme und Nachholen, die Minuten dauern können (Prüfrunde 7); eine
  leere Datei wird nicht genannt, sie geht nie hoch. Fertig ist eine Datei, die vor der Datei
  benannt ist, an die der Writer anhängt; eine fertige wird entfernt, sobald
  sie ganz oben ist. Ein anderer Sync-Speicher (`configure`, ein Beitritt, das
  Umschalten der Verschlüsselung) bekommt jede ausstehende Datei noch einmal.
  Runde 5 hatte statt dessen geraten, ob die entfernte Kopie noch da ist
  (ein Protokoll des Hochgeladenen, nur der Rest unter `gc_horizon`, Rotation
  des Writers); Runde 6 fand, dass das halb geschriebene Kopien nie heilte.
- **Was 0.1.0 hinterließ, einmal.** Diese Builds entfernten nichts aus
  `pending/`, nannten nichts, und ihre Compactors löschten nach Namen. Bevor
  dieser Build zum ersten Mal pusht, nachholt oder wiederaufnimmt, gilt von
  jeder fertigen Datei unter `gc_horizon`, die der Sync-Speicher nicht mehr
  hat, als oben, was vor der letzten erfolgreichen Runde dieses Geräts lag
  (`sync.lastSuccessfulRound`, wie der erste Versuch sie las:
  `sync.push.leftoversCutoff`; abzüglich zehn Minuten: eine Runde pusht vor
  ihrem Ende, und eine Änderung während der Runde kann ihren Push verpasst
  haben); der Rest geht als eigene Datei, kurz vor der laufenden benannt, und
  die alte Datei geht (`sync.push.leftoversSettled`, danach nie wieder). Ist
  keine solche Runde bekannt, gilt nichts als oben. Hat der Sync-Speicher die
  Datei noch, geht sie ganz hoch: Gelöscht hat sie niemand, und ein späterer
  Push von 0.1.0 kann sie halb geschrieben haben (Prüfrunde 7). Die Grenze
  (`LeftoversCutoff`: die letzte erfolgreiche Runde des alten Builds und der
  Start der App-Sitzung, die sie einfror) wird zu Beginn jeder Runde und jedes
  Pushs eingefroren, bevor etwas sie verschieben kann, auch ohne `meta.json`
  (Prüfrunde 9); lässt sie sich nicht halten, ordnet der Versuch nichts, und
  die Runde zählt nicht als erfolgreich; eine, die sich nicht lesen lässt,
  wird neu eingefroren, ohne bekannte Runde: Diesen Wert haben die Runden
  dieses Builds inzwischen überschrieben (Prüfrunden 10 und 11). Ein Rest ist eine Datei, die vor dem
  ersten Lauf dieses Builds benannt ist (auf die Sekunde, wie Dateien heißen)
  und unter `gc_horizon` liegt; ein Rest, den das Ordnen schrieb, ist keiner
  (`sync.push.leftoverTails`). Bis alle
  geordnet sind, geht keiner hoch, auch keiner, der sich nicht lesen ließ
  (Prüfrunde 8); die späteren Dateien dieses Builds warten nicht (Prüfrunde 9).
  Das Auflisten holt bei den meisten Speichern einmal jede Datei; ein Versuch,
  der nicht fertig wird, wiederholt sich erst eine Stunde später
  (`sync.push.leftoversTriedAt`). Lässt sich ein geordneter Rest nicht
  löschen, wird er geleert, damit kein späterer Versuch ihn noch einmal teilt;
  einen Rest, den diese App-Sitzung schon schrieb, schreibt sie nicht noch
  einmal; ein Rest nimmt keinen Namen, den der Sync-Speicher schon hat.
  Auch `push_now`, der Push nach jeder Änderung und auf dem Handy der übliche,
  holt `meta.json` und prüft Mindestversion und Verschlüsselung wie die Runde.
- **Gemerkte Längen.** Die 256 neuesten (nach Namen), nicht nach `gc_horizon`
  beschnitten: Eine noch laufende Sitzung mit altem Namen muss ihr Wachstum
  weiter zeigen. Grenze: Liegen mehr als 256 Dateien vor, ist die älteste jede
  Runde „unbekannt“ und wird erneut geholt. Das ist richtig, kostet aber
  Bandbreite.

### D. Kennungen im Schnappschuss

Der Compactor nennt die gehaltenen Kennungen nur, wenn das Gerät **genau** ist
und jedes Ereignis in jeder vorhandenen Datei hält (eigenes oder angewendet).
Sonst nennt er nichts. Kennungen sparen einem beitretenden Gerät Arbeit und
schützen Zeilen ohne bekannte Zeit (E). Nötig sind sie nicht mehr: Das
Nachholen heilt auch ohne sie.

### E. Zeilen-Uhr: Neuer gewinnt

`sync_row_clock` (Migration 49) hält je Zeile zwei Zeiten:

- `changed`: die neueste Änderung jeder Art.
- `written`: die neueste, die die ganze Zeile schrieb (Anlegen, ganze Zeile,
  Löschen). Leer, solange nur Feld-Zusammenführungen die Zeile berührten.
- `deleted`: Diese neueste Schreibung der ganzen Zeile war ein Löschen.

Der Writer stempelt eigene Änderungen beim Schreiben, der Anwender jede
übernommene. Eine eigene Änderung ist immer neuer als alles, was ihre Zeile
gesehen hat: Geht die Uhr dieses Geräts nach, bekommt sie einen Stempel knapp
über der Uhr der Zeile, sonst übersprängen die anderen sie dauerhaft. Die Zeit einer Zeile ist `changed`. Bei Terminen und Aufgaben ist
sie mindestens ihr eigenes `updated_at`.

Ein Schnappschuss trägt die Uhren seines Autors (`row_clocks`). Wer ihn
übernimmt, stempelt sie, jede nie zurück, aber nur, wenn alle seine Zeilen
geschrieben wurden; sonst trägt er auch keine Kennungen ein. Ein Schlüssel,
den dieser Build nicht synchronisiert (etwa der eines neueren), wird mit
Absicht weggelassen und zählt nicht als Fehler. Ein Beitritt
beginnt mit leeren Uhren: Die eines früheren Datensatzes hielten die
Änderungen des neuen als älter draußen.

- **Ganze Zeile und Anlegen:** Älter als die Zeile wird übersprungen
  (`skipped_older`) und als angewendet vermerkt.
- **Feld-Zusammenführung** (Termin, Aufgabe, Liste, Kalender, Farbe): wägt die
  Zeit der Zeile wie bisher `updated_at`. Ältere Felder ergeben einen
  Konflikt; beim Nachholen bleibt die neuere Zeile still. Das `updated_at`
  einer älteren Zusammenführung bleibt draußen, sonst verlöre die Zeile ihre
  Zeit. Fehlt die Zeile und war ihre letzte Schreibung der ganzen Zeile ein
  Löschen, bleibt die Zusammenführung draußen und legt sie nicht neu an. Hat
  dagegen ihre Liste oder ihr Kalender sie beim Löschen mitgenommen, ohne
  eigenes Löschen, gilt die Zusammenführung: Eine Aufgabe, die ein anderes
  Gerät inzwischen in eine andere Liste verschoben hat, kommt dort an.
- **Löschen:** verliert nur gegen eine neuere Schreibung der ganzen Zeile,
  nicht gegen eine neuere Feldänderung. Eine Feldänderung kann eine gelöschte
  Zeile auf dem löschenden Gerät nicht zurückholen. Verlöre das Löschen,
  liefen die beiden Geräte dauerhaft auseinander. Ein älteres Anlegen nach dem
  Löschen bleibt gelöscht.

**Übergang.** Die Uhr beginnt leer. Eine Zeile ohne bekannte Zeit (keine Uhr,
kein `updated_at`) kann einen neueren Stand aus einer schon gelöschten Datei
halten. Beim Nachholen wird darum für so eine Zeile eine schon angewendete
oder eigene Änderung, ein Löschen eingeschlossen, **nicht** erneut
angewendet; die Uhr der Zeile geht knapp hinter ihre Zeit, damit dieselbe
Änderung später nicht doch gewinnt (ein Schnappschuss trägt die Uhr weiter).
Fehlt die Zeile, wurde sie danach gelöscht: Die Uhr merkt ein Löschen —
außer bei einem Termin oder einer Aufgabe, deren Kalender, Liste oder
Eltern-Aufgabe ebenfalls fehlt. Die kann sie beim Löschen mitgenommen haben:
Die Zeile gilt als nicht gelöscht, und eine verpasste Verschiebung gilt. Die
neueste gehaltene Änderung der Zeile entscheidet (Prüfrunde 5). Ist sie nach
einem angewendeten Löschen da, wurde sie neu angelegt. Die Uhr merkt sich,
dass sie nur gesetzt ist (`seeded`): Auch ein späteres Nachholen hält dort
eine angewendete Änderung. Das Merkmal folgt der neuesten Änderung: Erst eine
neuere übernommene macht die Zeit bekannt; bei gleicher Zeit übernimmt ein
Setzen weder ein echtes Schreiben noch die Lösch-Entscheidung eines früheren
Setzens. Ein Schnappschuss trägt es nicht weiter: Beim Übernehmenden halten
die Zeilen dessen Werte, seine Uhren sind dort bekannte Zeiten (Prüfrunde 6).
Eigene Änderungen gelten als
angewendet, denn die Zeilen des Geräts halten sie. Nur wo derselbe Durchgang
einen Schnappschuss angewendet hat (Beitritt, Wiederaufnahme), wird eine
eigene gewogen, die in keiner eigenen Datei oben steht: Der
Schnappschuss kann einen älteren Wert zurückgesetzt haben, und die anderen
Geräte übernehmen sie, sobald sie oben ist. Was ein älterer Build bis zu
seiner letzten erfolgreichen Runde (C, ohne die zehn Minuten) hielt, gilt
dabei als oben: Es ging damals hoch, und die anderen Geräte übernahmen es, ob
ein Aufräumen die Datei seither löschte oder ein späterer Push sie halb
schrieb (Prüfrunde 8) — außer ein Schnappschuss, der nichts nennt, ist älter
als die Änderung: Der kann sie nicht halten (Prüfrunde 9). Wo die eigene
Fernkopie einer Datei lesbar ist, zeigt sie, was hochging — doch ein späterer
Push des alten Builds kann sie gekürzt haben: Bis zehn Minuten vor dessen
letzter Runde gilt eine fehlende Zeile darum trotzdem als oben (Prüfrunden 10
und 11). Die ausstehenden Dateien liest der Durchgang vorher und nach dem
Schnappschuss noch einmal, damit eine Änderung währenddessen gewogen wird und
eine inzwischen hochgeladene Datei ihre Zeilen behält. Eine Änderung eines
Zugangs geht bei ausgeschaltetem E2E nie hoch und wird darum nicht gewogen. Ist die Grenze
noch nicht eingefroren, gilt `sync.lastSuccessfulRound`, wie der alte Build
ihn hinterließ. Lässt sich die Grenze oder eine ausstehende Datei nicht
lesen, scheitern Beitritt und Wiederaufnahme, bevor sie etwas ändern (was in
`pending/` keine Datei ist, zählt nicht). Tritt das Gerät einem Datensatz bei,
in dem es nie war, gilt nichts davon als oben, und seine Reste werden gegen
diesen neu geordnet, sobald der Beitritt steht. Ebenso jede eigene oder schon
angewendete Änderung, die der Schnappschuss nicht hält: nicht unter seinen
Kennungen, oder, nennt er keine, jünger als er (Prüfrunde 6). Das gewöhnliche Nachholen hat
keinen Schnappschuss und hält jede eigene (Prüfrunde 5). Ein Löschen eines
Termins oder einer Aufgabe wiegt das `updated_at` der Zeile mit, wenn ein
Schnappschuss sie zurückgebracht hat.

Eine Regel „älter als `gc_horizon` gilt als gehalten“ gibt es nicht
(Prüfrunde 4): Die Grenze folgt Dateinamen, nicht gelöschten Dateien, und
verwürfe späte und gewöhnliche Änderungen. Dateien, die 0.1.0-Builds nach dem
Aufräumen erneut hochgeladen haben, können darum beim ersten Nachholen eines
Geräts eine Zeile ohne bekannte Zeit auf einen älteren Wert setzen. Neue
Builds laden nur hoch, was niemand gelöscht hat (C); 0.1.0-Builds sind ab dem
ersten Eintrag eines neuen Builds in `meta.json` ausgesperrt, laden aber bis
zu ihrem Update weiter hoch. Erst wenn das Nachholen die Zeile selbst ändert, mit einer nie
angewendeten Änderung, werden spätere angewendete wieder gewogen. Grenzen:
Eine nie angewendete ältere Änderung auf so einer Zeile gewinnt noch, wie in
jeder Runde zuvor. Fehlt die Datei der letzten Verschiebung, entscheidet der
Behälter einer älteren gehaltenen Änderung. Eine Aufgabe, selbst gelöscht,
deren Liste danach gelöscht wurde, gilt als mitgenommen; eine nie angewendete
Verschiebung legt sie dann wieder an.

### F. Aufräumen (nach Prüfrunde 2)

- **Warten.** Jedes Gerät meldet in seinem Eintrag in `meta.json`, ob es
  genau ist (`exact`, Herzschlag, Beitritt, Wiederaufnahme, Compactor).
  Der Compactor löscht nichts, solange ein aktives Gerät es nicht ist,
  sich selbst eingeschlossen; aktiv heißt: nicht als veraltet markiert und
  sein Horizont liegt im Aufbewahrungsfenster. Ein älterer Build kennt das
  Feld nicht und gilt als nicht genau. Solange gewartet wird, hebt er
  `gc_horizon` nicht an und markiert niemanden als veraltet. Die Meldung
  nach „Jetzt kompaktieren“ nennt die Geräte, auf die gewartet wird.
- **14 Tage.** Eine Datei geht erst, wenn ihr jüngster Eintrag älter ist als
  das Aufbewahrungsfenster, der Compactor sie so lange unverändert gesehen hat
  (`sync.compaction.logsSeen`: Größe und seit wann; eine neue oder gewachsene
  Datei beginnt von vorn), und jedes Ereignis darin eigenes ist oder beim Bau
  des Schnappschusses angewendet war. So zählt das Fenster ab dem Eintreffen:
  Eine spät hochgeladene Sitzung mit alten Einträgen bleibt, bis jedes Gerät
  sie holen konnte.
- **Unlesbares.** Eine Datei, die ein aktives Gerät als unlesbar nennt
  (B.5), bleibt: Ein Update dort liest sie wieder (Prüfrunde 5). Genannt
  bleibt sie, bis eine Runde sie liest, welcher Parser sie auch aufschrieb;
  nur fremde Dateien werden genannt (Prüfrunde 6), eigene stehen nur im Merker
  des Geräts (B.5).
- **Ausstehendes.** Eine Datei, die irgendein Gerät als ausstehend nennt (C),
  bleibt: Es lädt sie vielleicht noch einmal hoch (Prüfrunde 6).
- **Veraltet.** Die Markierung, die ein Compactor setzt (`stale`), nimmt nur
  die Wiederaufnahme zurück, nachdem sie den Schnappschuss geholt hat;
  Herzschlag und Compactor übernehmen sie (Prüfrunde 7). Ein Push zwischen den
  Runden nennt seine Dateien über den Herzschlag und nahm sie sonst zurück:
  Das Gerät holte den Schnappschuss nie. Ein markiertes Gerät verdichtet
  nicht, auch nicht von Hand: Sein Schnappschuss ersetzte den, den seine
  Wiederaufnahme holen muss (Prüfrunde 8).
- **Ausstehende Dateien.** Der Compactor lädt nichts hoch. Er räumt eine
  ausstehende Datei nur weg, wenn ihre entfernte Kopie mindestens so lang
  ist; sonst bleibt sie für den Push der Runde, und ihre entfernte Kopie
  bleibt stehen. Scheitert die Rotation, räumt er nichts weg, und keine
  entfernte Kopie einer ausstehenden Datei geht.

## Was nicht Teil davon ist

- Lokale Termine und Kalender werden beim Start nicht nachgetragen (TODO B3).
  Was nie im Protokoll stand, bringt das Nachholen nicht, löscht es aber auch
  nicht.
- Echte Gleichzeitigkeit (Vektoruhren) bleibt wie in §19.3: Bei gleich alten
  Änderungen verschiedener Geräte entscheidet die Reihenfolge.
- Zwischen dem neuen Auflisten und dem Löschen bleibt ein kurzes Fenster:
  Hängt ein Gerät genau dann an, geht das Angehängte verloren. Ohne Sperre im
  Sync-Speicher lässt sich das nicht schließen.
- Auf dem Handy E2E auf einem Ziel ohne `meta.json` einzuschalten, ist
  „Neu beginnen“: Dort liegende Protokolldateien werden gelöscht (U10).
- Die Anhebung des Stempels einer eigenen Änderung (Uhrversatz) misst gegen
  die Uhr der Zeile, nicht gegen ihr `updated_at` (U12).
- Varianten, die diese Version nicht anwendet (Tastenkürzel), gelten als
  nicht gehalten: Eine Datei mit ihnen bliebe stehen, und das Gerät nennte
  keine Kennungen. Noch schreibt kein Gerät solche Ereignisse.

## Schritte

- C1 bis C5, die erste Fassung mit Neuaufbau: nach Prüfrunde 1 ersetzt.
- R1 Längen: die 256 neuesten, kein Beschnitt nach Horizont. Erledigt.
- R7 Zeilen-Uhr, mit zwei Zeiten, Löschen und Übergang (E). Erledigt.
- R3 Kennungen nur, wenn genau und vollständig. Erledigt.
- R4 und R5 Nachholen statt Neuaufbau, ein Durchgang für Beitritt,
  Wiederaufnahme und Runde. Erledigt.
- R6 Unbekanntes ohne Untergrenze. Erledigt.
- R2 Aufräumen: nach dem Schnappschuss neu auflisten, nur ganz Gehaltenes
  löschen, Leeres und Unlesbares nie, das Ende ausstehender Sitzungen vor dem
  Wegräumen hochladen (scheitert es, bleibt die Datei). Erledigt.
- Doku, dann Prüfrunde 2.

Je Schritt Tests und Gegenproben; `cargo test -p cal-ffi` bei jeder Änderung an
Compactor oder Runde (Lehre aus dem Compactor-Bogen).

## Prüfrunde 1 (PR #130, 2026-10-30)

4 Prüfer + Gegenprüfer + Kritiker (40 Agenten): 35 bestätigt, 0 widerlegt,
dazu 7 vom Kritiker. Einzelheiten: scratchpad `coverage/review_r1.json`.

**Kritisch**
- Gemerkte Längen nach `gc_horizon` beschnitten: Eine noch laufende Sitzung mit
  altem Namen fällt heraus, ihr Wachstum wird nie mehr geholt — der
  Laptop-Fehler kehrt zurück, auch beim Beitritt (dreimal gefunden).
- „Vollständig“ ist nicht beweisbar: Der Compactor sieht nur vorhandene
  Dateien. Fehlen einem Gerät Ereignisse aus schon gelöschten Dateien, oder hat
  ein alter Schnappschuss (Upsert bei der Wiederaufnahme) Zeilen zurückgesetzt,
  deren Ereignisse als angewendet vermerkt bleiben, gilt es trotzdem als
  vollständig — sein Schnappschuss würde Grundlage aller Neuaufbauten.
- Der Neuaufbau setzt zurück: Was ein Gerät mehr wusste als der Autor des
  Schnappschusses, ist verloren, wenn die Dateien schon gelöscht sind.
- Verspätete Dateien unter `gc_horizon` holt niemand; erneut hochgeladene alte
  Sitzungen ebenso.

**Schwer:** Beitritt wendet ganze Dateien nach Namen an statt Ereignisse nach
Zeit; Aufräumen auf einer Liste von vor dem Schnappschuss (Wachstum dazwischen
gelöscht), leere/abgerissene Lesungen; das Ende der rotierten Sitzung wird
weggeräumt, ohne hochgeladen zu sein; alte Builds löschen das Meta-Feld.

## Umbau nach Prüfrunde 1 (Toni, 2026-10-30)

- **Nachholen statt ersetzen.** Kein Neuaufbau aus dem Schnappschuss. Jedes
  Gerät, das nicht genau ist, holt **einmal** alle vorhandenen Dateien nach,
  eigene eingeschlossen, in Zeitreihenfolge; eine Änderung gilt nur, wenn sie
  neuer ist als die Zeile, ohne Konfliktmeldung, Löschungen ebenso. Danach ist
  es genau. So wird nichts zurückgesetzt; es repariert auch Zeilen, die ein
  alter Schnappschuss zurückgesetzt hat, solange die Ereignisse noch in Dateien
  liegen. Neuaufbau, Auto-Verdichtung, `meta.snapshot_coverage` und
  `deletes_to_replay` entfallen.
- **Neuer gewinnt auch ohne Zeilenzeit** (Toni: gleich mitbeheben). Wo eine
  Tabelle `updated_at` hat, setzt das Anwenden sie auf die Zeit des Ereignisses
  und vergleicht damit; ein älteres Ereignis gewinnt nicht mehr — auch nicht,
  wenn es spät ankommt (offline geschrieben, gewachsene Datei).
- **Längen:** die 256 neuesten behalten, nicht nach Horizont beschneiden.
- **Aufräumen:** nach dem Schnappschuss neu auflisten, nur löschen, was mit
  aktuellem Inhalt ganz gehalten wird; leere oder unlesbare Dateien nie; das
  Ende der rotierten Sitzung vor dem Wegräumen hochladen.
- **Kennungen** schreibt nur ein Gerät, das genau ist UND alles hält.
- **Beitritt und Nachholen** wenden die Ereignisse aller Dateien gemeinsam
  nach Zeit an.
- **Unbekannte Dateien** holt ein genaues Gerät ohne Untergrenze.

## Prüfrunde 2 (PR #130, 2026-10-30)

Gezielt: 4 Prüfer, die 10 schwersten Befunde gegengeprüft (alle bestätigt),
18 weitere selbst nachgeprüft, ein Kritiker. Einzelheiten: scratchpad
`coverage/review_r2.json`.

**Die Zeit der Zeile hält nicht**
- Ein Schnappschuss schreibt Zeilen, aber keine Uhren. Nach Beitritt oder
  Wiederaufnahme gewinnt dann eine ältere Änderung gegen seinen neueren
  Inhalt.
- Eine ältere Feld-Zusammenführung übernimmt ihr `updated_at` als Metadatum.
  Die Zeile verliert ihre Zeit, und die nächste ältere Änderung gewinnt.
- Eigene Ereignisse stehen nie in `sync_applied_events`. Die Übergangsregel
  schützt darum nicht vor den eigenen älteren Änderungen.
- Ein Gerät mit nachgehender Uhr schreibt eine spätere Änderung mit älterem
  Stempel; die anderen überspringen sie dauerhaft.

**Löschen gegen Bearbeiten:** Eine Feld-Zusammenführung auf eine fehlende
Zeile legt sie neu an. Das löschende Gerät bekommt die Zeile zurück, das
bearbeitende löscht sie: Sie laufen auseinander, entgegen Abschnitt E.

**Aufräumen:** Der Compactor löscht eine Datei, deren Ende ein anderes Gerät
noch nicht geholt hat, auch die eigene gleich nach dem Hochladen. Er löscht,
was ein Gerät vor seinem Nachholen braucht (der Laptop). Er vertraut seinem
eigenen Protokoll, auch wenn es nicht genau ist. Das Hochladen vor dem
Wegräumen umgeht den Push und lädt nach dem Abschalten von E2E Zugangsdaten
im Klartext hoch.

**Weiteres:** Der Bericht des Nachholens geht verloren (Ansichten laden nicht
neu). Ein Schnappschuss mit fehlgeschlagenen Zeilen trägt trotzdem seine
Kennungen ein. Uhren eines früheren Datensatzes bleiben beim Beitritt.
„Neu beginnen“ lässt die alten Dateien liegen, und das Nachholen holt sie
herein.

## Entscheidungen nach Prüfrunde 2 (Toni, 2026-10-30)

- **Aufräumen: warten und 14 Tage.** Der Compactor löscht nichts, solange ein
  aktives Gerät (nicht als veraltet markiert) noch nicht nachgeholt hat; jedes
  Gerät meldet das in `meta.json`. Und er löscht eine Datei erst, wenn ihr
  jüngster Eintrag älter ist als das Aufbewahrungsfenster (14 Tage). Im
  Sync-Ordner liegen damit immer mindestens 14 Tage Protokoll.
- **„Neu beginnen“ löscht die Protokolle und den Schnappschuss** des alten
  Datensatzes.
- **Das Klartext-Leck wird hier behoben:** Der Push entfernt
  Zugangsdaten-Ereignisse, solange E2E aus ist, an einer Stelle für alle
  Wege. Der Compactor lädt nichts mehr selbst hoch; eine ausstehende Datei,
  deren Ende noch fehlt, bleibt für den nächsten Push liegen.
- Ohne Rückfrage, weil es Abschnitt E erst wahr macht: Der Schnappschuss trägt
  die Uhren seines Autors (und nur, wenn alle Zeilen geschrieben wurden);
  Metadaten einer älteren Zusammenführung bleiben draußen; eigene Ereignisse
  gelten beim Nachholen als gehalten; eine Zusammenführung auf eine gelöschte
  Zeile bleibt draußen; eine eigene Änderung ist immer neuer als ihre Zeile;
  der Beitritt beginnt mit leeren Uhren; der Bericht des Nachholens geht in
  die Runde ein.

## Prüfrunde 3 (PR #130, 2026-10-31)

Klein: 3 Prüfer auf die zehn Commits nach Runde 2, die 6 schwersten Befunde
gegengeprüft, ein Kritiker. Einzelheiten: scratchpad `coverage/review_r3.json`.

- **Eigene, noch nicht hochgeladene Änderungen sind nicht gehalten.** Nach dem
  Schnappschuss eines Beitritts oder einer Wiederaufnahme halten die Zeilen sie
  nicht mehr; das Gerät behielt den Wert des Schnappschusses, schob aber seine
  Änderung, und die anderen übernahmen sie.
- **Was das Nachholen hält, wird nicht vermerkt** (älter als die GC-Grenze),
  und eine gewöhnliche Runde wendet es später doch an. Die Regel gilt nur im
  Nachholen.
- **Kaskade ist kein Löschen:** Eine Aufgabe, die aus einer gleichzeitig
  gelöschten Liste verschoben wurde, ging verloren; die Regel „eine
  Zusammenführung auf eine gelöschte Zeile bleibt draußen“ unterscheidet nicht
  zwischen gelöscht und mit dem Behälter verschwunden.
- **14 Tage ab dem Schreiben, nicht ab dem Eintreffen:** Eine spät
  hochgeladene Sitzung mit alten Einträgen ging gleich wieder.
- **Alte Builds** verdichten nach Namen und löschen die `exact`-Meldungen.
- Kleiner: eine dauerhaft unlesbare Datei sperrt das Aufräumen für immer; eine
  unvollständige Wiederaufnahme behält ein altes „genau“; ein gelöschter Termin,
  den ein Schnappschuss zurückbrachte, galt als Zeile ohne Zeit; der Beitritt
  ändert Uhren und Merker, bevor er etwas geholt hat; ein aus gutem Grund
  verworfener Schlüssel ließ den Schnappschuss als fehlerhaft gelten;
  „Neu beginnen“ ohne Passwort könnte bei übrig gebliebenem E2E-Merker
  Zugangsdaten in den Schnappschuss schreiben.

## Entscheidungen nach Prüfrunde 3 (Toni, 2026-10-31)

- **Alte Builds aussperren:** Die Sync-Formatversion steigt auf 2, die
  App-Version auf 0.2.0 (Toni: 0.2.0; Desktop und Handy melden für den Sync
  die Workspace-Version, und die alten Builds melden 0.1.0). Die erste
  Verdichtung eines neuen Builds hebt `min_app_version` auf 0.2.0 (seit
  Prüfrunde 6 schon sein erster Eintrag in `meta.json`); alte Builds
  synchronisieren dann nicht mehr und zeigen „Update nötig“. Bis dahin können
  sie noch nach Namen löschen: nach dem Update einmal „Jetzt kompaktieren“.
- Danach eine Prüfrunde wie Runde 3.

## Prüfrunde 4 (PR #130, 2026-10-31)

Wie Runde 3: 3 Prüfer auf die Commits nach Runde 3, 6 gegengeprüft (alle
bestätigt), ein Kritiker. Einzelheiten: scratchpad `coverage/review_r4.json`.

- **Die Regel „älter als `gc_horizon` gilt als gehalten“ ist falsch.**
  `gc_horizon` folgt Dateinamen, nicht gelöschten Dateien: Eine gewöhnliche
  Änderung, in einer lang laufenden Sitzung vor einer fremden Verdichtung
  geschrieben, oder die Offline-Änderungen eines lange abwesenden Geräts,
  wurden auf jedem Gerät verworfen, beim Urheber aber behalten. Die Regel
  entfällt ganz (Runden und Nachholen). Was sie abwehren sollte — Dateien,
  die 0.1.0-Builds nach dem Aufräumen erneut hochgeladen haben — bleibt eine
  Grenze des Übergangs: Neue Builds laden fertige Sitzungen nicht erneut hoch,
  und 0.1.0-Builds sind ab der ersten Verdichtung auf Format 2 ausgesperrt.
- **Die gesetzte Uhr einer gehaltenen Änderung** stand genau auf ihrer Zeit;
  ein Schnappschuss trug sie weiter, und ein Beitretender wandte die Änderung
  dann doch an. Sie steht jetzt knapp danach.
- **Eine gehaltene Anlage auf einer fehlenden Zeile** setzte „geschrieben,
  nicht gelöscht“, und eine spätere Bearbeitung legte die gelöschte Zeile neu
  an. Fehlt die Zeile, gilt sie als gelöscht.
- Kleiner: eine dauerhaft unlesbare Datei wurde nach einem Update nie wieder
  gelesen; „Jetzt kompaktieren“ prüft die Mindestversion nicht; eine
  gescheiterte zweite Auflistung löschte die Sichtungsliste; der Beitritt
  schob die `meta.json` zurück, die er vor seinem langen Durchgang gelesen
  hatte (und hob damit eine gleichzeitige Aussperrung auf). Alle vier behoben:
  Der Merker unlesbarer Dateien trägt die App-Version; „Jetzt kompaktieren“
  bricht bei zu alter Version ab, bevor es etwas hochlädt; die Sichtungsliste
  bleibt bei gescheiterter Auflistung, wie sie ist; der Beitritt trägt sich in
  eine frisch geholte `meta.json` ein.
- Alte Builds: Ein ausgesperrter 0.1.0-Build kann „Jetzt kompaktieren“ noch
  ausführen und nach Namen löschen. Das lässt sich nur durch das Update
  verhindern: alle Geräte aktualisieren, dort nicht von Hand kompaktieren.

## Prüfrunde 5 (PR #130, 2026-10-31)

Wie Runde 4: 3 Prüfer auf die Commits nach Runde 4, 6 gegengeprüft (alle
bestätigt), ein Kritiker. Vor dem Umbau prüften 3 Kritiker den Plan.
Einzelheiten: scratchpad `coverage/review_r5.json`, `coverage/r5_plan.md`.

- **Mitgenommen statt gelöscht.** Eine gehaltene Änderung auf einer fehlenden
  Zeile setzte „gelöscht“, auch wenn ihre Liste, ihr Kalender oder ihre
  Eltern-Aufgabe sie mitgenommen hatte; eine verpasste Verschiebung blieb
  draußen (Runde 3, Befund 4, kam im Nachholen zurück). Siehe E.
- **Gesetzt bleibt unbekannt.** Eine im ersten Nachholen gesetzte Uhr galt im
  nächsten als bekannte Zeit: Eine spätere angewendete Änderung, die der erste
  Durchgang nicht erreicht hatte, gewann. Siehe E.
- **Eigene Änderungen.** Alte eigene Sitzungen, längst hochgeladen und vom GC
  gelöscht, galten beim Nachholen als „nicht hochgeladen“ und gewannen auf
  Zeilen ohne bekannte Zeit. Das Nachholen hält jetzt jede eigene; Beitritt
  und Wiederaufnahme fragen das Push-Protokoll. Siehe E.
- **Doppelt hochgeladen.** Neue Builds luden alte und vom GC gelöschte eigene
  Sitzungen doch wieder hoch: beim ersten Start, bei jedem Neustart, und eine
  laufende Sitzung, die ein Compactor gelöscht hatte, beim nächsten Anhängen
  ganz. Siehe C.
- **Unlesbare Dateien.** Der Merker hing an der App-Version, die über Builds
  gleich bleibt; eine wachsende Datei eines neueren Builds hielt ein Gerät nie
  genau; in gewöhnlichen Runden wurde sie jede Runde neu geholt. Siehe B.5.
- **Verschlüsselung.** „Jetzt kompaktieren“ und `push_now` prüften nicht, ob
  der Datensatz verschlüsselt ist: Ein unverschlüsseltes Gerät schrieb seinen
  Schnappschuss oder seine Protokolle lesbar hinein.
- Von mir entschieden (Toni sagen): Die Gegenrichtung (Gerät verschlüsselt,
  Datensatz nicht mehr) und das Fenster während des Einschaltens bleiben, wie
  sie vor diesem PR waren, und stehen in TODO B3: Sie brauchen eine eigene
  Meldung in beiden Oberflächen. Ebenso das „Neu beginnen“ eines anderen
  Geräts: Wer weiterläuft, lädt seine laufende Datei mit den alten
  Ereignissen in den neuen Datensatz.

## Prüfrunde 6 (PR #130, 2026-10-31)

Vier Prüfer auf die Commits nach Runde 5, 8 gegengeprüft (alle bestätigt),
ein Kritiker; vor dem Umbau prüften 3 Kritiker den Plan. Einzelheiten:
scratchpad `coverage/review_r6.json`, `coverage/r6_plan.md`.

- **Kursänderung beim Push.** Das Protokoll aus Runde 5 riet, ob eine
  entfernte Kopie noch da ist; ein abgebrochener Push blieb so für immer halb
  geschrieben, Einträge gingen verloren, und es hing nicht am Sync-Speicher.
  Jetzt nennt jedes Gerät seine ausstehenden Dateien in `meta.json`, und kein
  Compactor löscht sie (C, F).
- **0.1.0-Reste** gehen nach der letzten erfolgreichen Runde dieses Geräts,
  nicht nach `last_seen` in `meta.json`: Den schrieb 0.1.0 auch nach einem
  gescheiterten Push, nur alle zwölf Stunden, und ein Verdichten vorher
  überschrieb ihn.
- **Aussperren beim ersten Schreiben.** Schon der erste Eintrag dieses Builds
  in `meta.json` (Herzschlag, Beitritt) hebt Format und `min_app_version`:
  Danach löscht kein Build mehr nach Namen und übersieht, was Geräte als
  ausstehend nennen.
- **Schnappschüsse** tragen das Merkmal „nur gesetzt“ nicht weiter, und nach
  einem Schnappschuss wird gewogen, was er nicht hält (E).
- **Unlesbares** bleibt genannt, bis es gelesen ist; eigene Dateien werden
  nicht genannt; eine nach einem Parser-Wechsel neu zu lesende Datei verliert
  ihre gemerkte Länge, auch bei einem Speicher ohne Größenangaben.
- Von mir entschieden (Toni sagen): das Aussperren schon beim ersten
  Schreiben statt bei der ersten Verdichtung; ein vergessenes Gerät, das
  zurückkommt, und ein Gerät, das als erste 0.2.0-Tat neu beitritt, laden ihre
  alten Dateien noch ganz hoch (TODO B3).

## Prüfrunde 7 (PR #130, 2026-10-31)

Vier Prüfer auf die Commits nach Runde 6, 8 gegengeprüft (7 bestätigt, einer
widerlegt), ein Kritiker. Einzelheiten: scratchpad `coverage/review_r7.json`.

- **Veraltet bleibt veraltet.** Der Push zwischen den Runden (auf dem Handy
  nach jeder Änderung, am Desktop beim Beenden) nannte seine Dateien über den
  Herzschlag, und der nahm die Markierung des Compactors zurück: Das Gerät
  holte den Schnappschuss nie und blieb für immer abweichend. Jetzt nimmt nur
  die Wiederaufnahme sie zurück (F).
- **Eigene abgerissene Dateien** stehen wieder im Merker, genannt werden sie
  nicht (B.5): Sonst wurde ihr Gerät nie genau, holte stündlich alles, und
  kein Compactor räumte je auf.
- **0.1.0-Reste** gehen ganz hoch, wenn der Sync-Speicher sie noch hat; nur
  von einer fehlenden geht der Rest (C). Unentschiedene bleiben liegen; die
  Grenze gilt, wie der erste Versuch sie las.
- **Nennen und Aussperren** zu Beginn jeder Runde, vor Wiederaufnahme und
  Nachholen; die Wiederaufnahme sperrt ebenfalls aus; leere Dateien werden
  nicht genannt.
- Kleiner: Eine gewöhnliche Runde merkt eine abgerissene Datei (ein genaues
  Gerät holte eine für immer abgerissene sonst jede Runde); nach einem
  Parser-Wechsel trägt der Merker den neuen Parser erst, wenn die Längen
  vergessen sind.
- Von mir entschieden (Toni sagen): Ein veraltetes Gerät pusht weiter wie
  bisher, nur die Markierung bleibt; die Reste-Prüfung listet den
  Sync-Speicher, was bei den meisten Speichern einmal jede Datei holt; ein
  0.1.0-Verdichten, das beim Aussperren schon läuft, kann noch nach Namen
  löschen (TODO B3).

## Prüfrunde 8 (PR #130, 2026-10-31)

Vier Prüfer auf den Commit nach Runde 7, 8 gegengeprüft (alle bestätigt), ein
Kritiker. Einzelheiten: scratchpad `coverage/review_r8.json`.

- **Ein markiertes Gerät verdichtet nicht** (F): Von Hand ersetzte es den
  Schnappschuss, den seine Wiederaufnahme holen muss, durch seinen eigenen,
  dem fehlte, was das Aufräumen gelöscht hatte.
- **0.1.0-Reste** (C): Zurückgehalten wird nach Zeit, nicht nach Namen, auch
  eine, die sich nicht lesen ließ; die Grenze zuerst eingefroren und, wo das
  scheitert, die Runde nicht als erfolgreich vermerkt; das Auflisten nach
  einem Fehlschlag erst eine Stunde später; ein Rest, der sich nicht löschen
  lässt, geleert; kein Restname, den der Sync-Speicher hat. Was ein Rest bis
  zur Grenze hielt, gilt beim Beitritt und bei der Wiederaufnahme als oben
  (E): Sonst gewann es, ganz gewogen, über den Schnappschuss, und kein anderes
  Gerät bekam es je.
- **Unlesbares** (B.5): Eine Runde ergänzt einen Merker, den ein
  Parser-Wechsel noch nicht neu gestempelt hat, statt ihn zu ersetzen; ihre
  eigenen Sichtungen abgerissener Dateien stehen in einem eigenen Merker, den
  das Nachholen nicht liest.
- Von mir entschieden (Toni sagen): Scheitert beim Ordnen eines Rests das
  Löschen und das Leeren, kann ein späterer Versuch seinen Rest ein zweites
  Mal hochladen (doppelte Ereignisse, harmlos; TODO B3).

## Prüfrunde 9 (PR #130, 2026-10-31)

Vier Prüfer auf den Commit nach Runde 8, 8 gegengeprüft (7 bestätigt, einer
widerlegt), ein Kritiker. Einzelheiten: scratchpad `coverage/review_r9.json`.

- **Die Grenze der Reste** (C) wird zu Beginn jeder Runde und jedes Pushs
  eingefroren, auch ohne `meta.json`, und hält neben der letzten Runde des
  alten Builds fest, ab wann die Dateien von diesem stammen: Nur die älteren
  warten; vorher hielt ein Ordnen, das nie fertig wurde, auch die eigenen
  späteren Sitzungen zurück.
- **Beitritt und Wiederaufnahme** (E) halten die Zeilen eines Rests bis zur
  letzten Runde selbst, nicht zehn Minuten davor: Was die Runde hochschob,
  gewann sonst über den Schnappschuss. Ein Schnappschuss, der nichts nennt und
  älter ist, hält sie nicht; ohne eingefrorene Grenze gilt die Runde, wie der
  alte Build sie hinterließ; eine Grenze oder ausstehende Datei, die sich
  nicht lesen lässt, lässt den Durchgang scheitern, bevor er etwas ändert; ein
  fremder Datensatz setzt die Grenze zurück.
- Kleiner: ein Versuch, der nicht fertig wird, erst nach einer Stunde
  wieder, und ein schon geschriebener Rest nicht noch einmal; eine unlesbare
  Datei, deren Name nicht hielt, ohne Länge; die Übernahme einer Markierung
  mitten im Verdichten ist durch einen Test gehalten.
- Von mir entschieden (Toni sagen): Was 0.1.0 nach seiner letzten Runde noch
  per Push hochschob (beim Beenden, auf dem Handy nach jeder Änderung), wiegt
  ein Beitritt oder eine Wiederaufnahme weiterhin über den Schnappschuss
  (TODO B3).

## Prüfrunde 10 (PR #130, 2026-10-31)

Vier Prüfer auf den Commit nach Runde 9, 8 gegengeprüft (5 bestätigt, 3
widerlegt), ein Kritiker. Einzelheiten: scratchpad `coverage/review_r10.json`.

- **Beitritt und Wiederaufnahme** (E): Eine lesbare eigene Fernkopie zeigt,
  was von einer ausstehenden Datei hochging; die Grenze gilt nur, wo sie
  fehlt oder abgerissen ist. Die Dateien selbst liest der Durchgang nach dem
  Schnappschuss. Ein Beitritt setzt die Grenze erst zurück, wenn er steht.
  Was in `pending/` keine Datei ist, hält nichts auf.
- **Reste** (C): Die Grenze auf die Sekunde, wie Dateien heißen; geschriebene
  Reste werden gemerkt und nicht als Dateien des alten Builds gehalten; eine
  unlesbare Grenze wird neu eingefroren.
- **Unlesbares** (B.5): Auch der Durchgang über alle Dateien merkt keine
  Länge, wo der Name nicht hielt, und ist dann nicht vollständig; eine Runde
  merkt dort gar keine Länge statt 0, die eine Auflistung ohne Größen als
  bekannt läse.
- Von mir entschieden (Toni sagen): Eine Änderung, die 0.1.0 in seiner
  letzten Runde nach deren Push schrieb, gilt bei einer Datei, die der
  Sync-Speicher nicht mehr hat, als oben (TODO B3). Eine ausstehende Datei,
  die sich dauerhaft nicht lesen lässt, hält Beitritt und Wiederaufnahme auf,
  bis sie lesbar ist oder entfernt wird (TODO B3).

## Prüfrunde 11 (PR #130, 2026-10-31)

Vier Prüfer auf den Commit nach Runde 10, 8 gegengeprüft (7 bestätigt, einer
widerlegt), ein Kritiker; alle Befunde klein. Einzelheiten: scratchpad
`coverage/review_r11.json`.

- **Lesbare Fernkopie** (E): Sie zeigt, was hochging, außer für Zeilen bis
  zehn Minuten vor der letzten Runde des alten Builds; eine leere oder
  gekürzte Kopie brachte sonst dessen alte Zeilen über den Schnappschuss
  zurück. Zugangsdaten bei ausgeschaltetem E2E werden nicht gewogen.
- **Ausstehende Dateien** (E): Der Durchgang nimmt beide Lesungen zusammen.
- **Grenze** (C): neu eingefroren ohne bekannte Runde; ein geordnetes Gerät
  braucht sie nicht mehr und tritt trotzdem bei.
- Von mir entschieden (Toni sagen): Ist die eingefrorene Grenze beschädigt,
  gilt keine Runde des alten Builds als bekannt: Seine Reste gehen ganz hoch
  (TODO B3).
