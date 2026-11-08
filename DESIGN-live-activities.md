# Live-Aktivitäten für Termine — Entwurf

Status: **entschieden; alle fünf Stufen gebaut (Kern, Tür, Antippen, iOS, Android).** Toni hat die Form am 6. November 2026
festgelegt, in zwei Runden: vor der Recherche (Start, Inhalt, Überblenden) und
danach (iOS-26-Einplanung, ein Signal, iOS-Text, Anschluss, ohne Erinnerung,
ganztägig, Standardwert). Die Recherche lief als Workflow mit drei Code-Lesern
(Erinnerungs-Pfad, iOS-Erweiterung, Android) und zwei Plattform-Recherchen
(ActivityKit, Android 16 Live Updates). Danach prüften drei Prüfer diesen
Entwurf gegen den Code, die Logik und die Plattformen (eine Runde); ihre
Befunde sind eingearbeitet. Die Stufenliste am Ende sagt, was in welcher
Reihenfolge gebaut wird. Nichts davon lief bisher auf einem Gerät.

Nur das Handy ist betroffen. Der Desktop hat nichts Vergleichbares; die Regel
liegt trotzdem im Kern, damit sie an einer Stelle steht und getestet ist.

## Der Anlass

Eine Erinnerung meldet sich einmal und ist dann weg. Wer kurz vor einem Termin
wissen will, wie viel Zeit bleibt, wo er stattfindet und was danach kommt, muss
Aperio öffnen und suchen. Eine Live-Aktivität hält diese drei Dinge sichtbar,
vom Moment der Erinnerung bis zum Ende des Termins: auf dem Sperrbildschirm, in
der Dynamic Island und auf Android in der laufenden Benachrichtigung samt
Statusleiste.

## Die Entscheidungen

1. **Eine Aktivität je Termin**, vom Auslösen seiner **letzten** Erinnerung bis
   zu seinem Ende.
2. **Überblenden:** Ist die letzte Erinnerung des nächsten Termins fällig,
   während die Aktivität noch läuft, zeigt sie ab da den nächsten Termin. Sonst
   endet sie mit dem Termin.
3. **Inhalt:** Titel und Zeit, Ort oder Raum, der Anschlusstermin. **Kein**
   Beitreten-Knopf.
4. **Anschluss:** der nächste Termin mit Uhrzeit am selben Tag, egal wie groß
   die Lücke ist, auch ohne eigene Erinnerung. Ganztägige zählen nicht.
5. **Ohne Erinnerung keine Aktivität.** Ohne Erinnerung gibt es keinen
   Startpunkt; wer eine Aktivität will, gibt dem Termin eine Erinnerung.
6. **Ganztägige Termine** (auch Geburtstage) bekommen **keine** Aktivität.
7. **Schalter** in den Einstellungen, standardmäßig **an**, je Gerät.
8. **iOS-Start:** Ab iOS 26 plant Aperio die Aktivität beim Öffnen für die Zeit
   der letzten Erinnerung ein, und iOS startet sie dann von selbst. Darunter
   startet sie, wenn Aperio geöffnet wird (Tipp auf die Erinnerung oder Öffnen
   der App) und ein Fenster gerade läuft. Kein Push-Server.
9. **Ein Signal:** Die letzte Erinnerung **wird** zur Aktivität, statt neben ihr
   zu erscheinen.
   - Android: Die Erinnerung verwandelt sich in die laufende Benachrichtigung;
     TalkBack sagt sie einmal an.
   - iOS 26: Der Start-Alarm der eingeplanten Aktivität ersetzt die
     Benachrichtigung. Klappt das Einplanen nicht, bleibt die Benachrichtigung.
   - Unter iOS 26 bleibt die Benachrichtigung immer.
10. **iOS-Text: beide Zeiten mitlaufend**, zum Beispiel „Beginn in 10 Minuten,
    Ende in 1 Stunde“, später „Beginn vor 5 Minuten, Ende in 55 Minuten“. Das
    bleibt ohne Aperio wahr. Der eine Inhaltswechsel, den iOS ohne App erlaubt,
    geht ans Überblenden. (Die mitlaufende Angabe kennt nur **eine** Einheit;
    die genauen Uhrzeiten stehen deshalb daneben, siehe Darstellung.)
11. **Höchstens 2 Stunden Vorlauf** (nachträglich, beim Bau von Stufe 1): Liegt
    die letzte Erinnerung mehr als 2 Stunden vor Beginn, gibt es keine
    Aktivität, und die Erinnerung bleibt eine gewöhnliche Benachrichtigung. Eine
    Erinnerung „1 Tag vorher“ ist eine zum Planen, nicht eine, nach der man die
    Uhr beobachtet.

## Was die Plattformen erlauben

### iOS (ActivityKit)

- **Starten** (`Activity.request`) geht nur, solange Aperio im Vordergrund ist,
  oder in einem `LiveActivityIntent`, den der Nutzer selbst auslöst (Widget-Knopf,
  Kurzbefehl, Siri). Aus dem Hintergrund wirft der Aufruf `.visibility`.
- **Einplanen** (iOS 26): `request(attributes:content:pushType:style:alertConfiguration:start:)`.
  iOS startet die Aktivität zur angegebenen Zeit, auch wenn Aperio im
  Hintergrund ist. Der Aufruf selbst muss aus dem Vordergrund kommen. Eine
  `AlertConfiguration` ist Pflicht, die Aktivität steht bis zum Start in
  `.pending`, und eingeplante Aktivitäten zählen zum Limit. Den Startzeitpunkt
  einer eingeplanten Aktivität kann man nicht ändern, nur die Aktivität beenden.
- **Der Alarm** hat als Ton nur `.default` oder eine mitgelieferte Tondatei
  (`.named`), keinen stummen. Titel und Text des Alarms zeigt nur die Apple
  Watch; auf dem iPhone ist die Aktivität selbst alles.
- **Ändern und Beenden** (`update`, `end`) gehen auch aus dem Hintergrund, aber
  nichts weckt Aperio pünktlich: Hintergrund-Abgleich kommt, wann iOS will, und
  eine lokale Benachrichtigung weckt die App nicht.
- **Ohne App** ändert sich der Inhalt nur über Zeitangaben, die iOS selbst
  mitlaufen lässt, und **genau einmal** über `staleDate`: Ab dann ist `isStale`
  wahr, und iOS zeichnet neu. Die mitlaufenden Angaben:
  - `SystemFormatStyle.DateReference` (ab iOS 18): „in 10 Minuten“ / „vor 5
    Minuten“, in beide Richtungen wahr, aber nur **eine** Einheit („in 1
    Stunde“, nicht „in 1 Stunde 10 Minuten“);
  - `Text(timerInterval:)`: ein Countdown wie „1:09:59“.
- **Kein Selbst-Ende.** Ohne App bleibt eine Aktivität bis zur Grenze von 8
  Stunden aktiv; dann beendet iOS sie, lässt sie aber bis zu 4 Stunden auf dem
  Sperrbildschirm. Die Grenze gilt je Aktivität, also je Kette.
- **Reihenfolge:** Laufen mehrere Aktivitäten, entscheidet `relevanceScore`,
  welche die Dynamic Island bekommt und oben steht; ohne Wert die zuerst
  gestartete.
- **Grenzen:** Statische und veränderliche Daten zusammen höchstens 4 KB. Apple
  nennt keine Zahl für gleichzeitige Aktivitäten je App; berichtet werden 5.
- **Nutzer-Schalter:** Jede App hat in den iOS-Einstellungen einen eigenen
  Schalter (`areActivitiesEnabled`). Ist er aus, wirft `request` `.denied`.
- **`.dismissed`** heißt „vom Nutzer **oder vom System** entfernt“, auch nach
  Aperios eigenem `end`. Zustandswechsel erfährt Aperio nur, solange es läuft.
- **Darstellungen:** Sperrbildschirm, Dynamic Island (kompakt, minimal,
  erweitert), StandBy; Apple Watch und CarPlay übernehmen sie von selbst.

### Android

- **Live Updates** (Android 16 QPR1 und neuer): eine laufende Benachrichtigung,
  die um Hervorhebung bittet (`setRequestPromotedOngoing`), mit Titel,
  Standard- oder BigText-Stil, ohne eigene Ansichten, **nicht** eingefärbt, auf
  einem Kanal über MIN. Dazu die Berechtigung `POST_PROMOTED_NOTIFICATIONS`
  (bei der Installation erteilt). Der Nutzer kann die Hervorhebung je App
  abschalten (`canPostPromotedNotifications`).
- **Googles Leitlinie:** Live Updates sollen vom Nutzer ausgelöst sein, nicht
  von anderen; „kommende Kalendertermine“ sind ausdrücklich **keine** passende
  Verwendung, ein Termin, der „gleich beginnt“, schon. Termine aus Einladungen
  anderer passen nicht sauber zu „vom Nutzer ausgelöst“ (siehe Risiken).
- **Ältere Android-Versionen:** dieselbe Benachrichtigung, nur ohne
  Hervorhebung: laufend, mit Countdown (`setChronometerCountDown`). Ab Android
  14 lässt sie sich trotzdem wegwischen.
- **Zeitgenau** über `AlarmManager.setExactAndAllowWhileIdle`. Aperio hat
  `USE_EXACT_ALARM` (Google Play erlaubt es „Kalender-Apps, die
  Terminbenachrichtigungen zeigen“), das aber erst ab Android 13 gilt. Auf
  Android 12 fehlt das Recht; Expo prüft `canScheduleExactAlarms()` und fällt auf
  ungenaue Alarme zurück. Alarme verschwinden bei Neustart und Zwangsbeenden.
- **Expo kann es nicht.** `expo-notifications` kennt nur `sticky` (laufend),
  aber keinen Countdown, kein `onlyAlertOnce`, keine Hervorhebung, und eine
  geplante Benachrichtigung lässt sich nicht an Ort und Stelle ändern. Es braucht
  ein kleines Kotlin-Stück im Modul `cal-ffi`.
- **Ansagen:** Eine neue Benachrichtigung über MIN sagt das System einmal an.
  Ein Update sagt es nur an, wenn es Ton macht **und** der Kanal mindestens
  DEFAULT ist; eine stumme Erinnerung (Kanal LOW) würde als Update also nie
  angesagt.
- **TalkBack** (nach dem offenen Quellcode, nicht am Gerät geprüft) liest
  „Termin, Titel, Text“. Den Countdown liest es in der Ansage nicht, beim
  Erkunden als Dauer ohne Richtung („24 Minuten, 13 Sekunden“). Die
  Statusleisten-Anzeige heißt vermutlich nur „Benachrichtigung von Aperio:
  Aktiv“.

## Die Regel im Kern

Eine reine Funktion in `cal-core` (neues Modul `live_activity`) bekommt alles
als Eingabe und liefert einen **Plan**. Sie liest nichts selbst.

### Eingaben

`cal-core` darf `host-core` nicht kennen, deshalb eigene Eingabetypen, auf die
`host-core` seine Daten abbildet:

- die Erinnerungs-Auslöser der Termine (Termin, Kalender, Vorkommen, Zeitpunkt,
  Ton, Titel und Text der Benachrichtigung), **auch die bereits ausgelösten**,
  solange das Vorkommen nicht vorbei ist;
- alle Termine mit Uhrzeit der betroffenen Tage, aufgeklappt zu Vorkommen, mit
  Ort, für Kopien und Anschluss;
- die Termingruppen;
- die verborgenen Kalender des Geräts (sie sind gerätelokal, wie beim Widget);
- „jetzt“ und der Horizont;
- je Vorkommen sein **Ortstag** auf dem Gerät (für „am selben Tag“ und für das
  Falten je Tag). Der Kern bekommt Tage statt einer Zone, wie überall sonst:
  Die Zone kennt der Host.

Der Horizont zählt **Vorkommen**, nicht Auslöser: Hat ein Vorkommen einen
Auslöser im Horizont, kommen alle seine Auslöser bis zu seinem Ende mit. Sonst
begänne das Fenster am Horizont-Rand an einer früheren Erinnerung.

Auslöser und Termine kommen aus **einem** Lesevorgang. Zwei getrennte Aufrufe
lesen jedes Konto live und können verschiedene Stände sehen; dann wäre eine
Erinnerung doppelt oder verschwunden.

### Ein Termin und seine Kopien

- **Schlüssel eines Vorkommens:** Termin-Id plus ursprünglicher Beginn des
  Vorkommens (bei Einzelterminen die Id allein). Ein verschobenes Vorkommen
  behält ihn.
- **Kopien** sind, je Tag in Ortszeit des Geräts: Mitglieder einer Gruppe, die
  sich nicht auseinanderentwickelt haben (gleicher Beginn), und Zeilen eines
  Meeting-Kalenders, die `without_duplicate_meetings` verwirft, gepaart mit dem
  echten Termin über dieselbe Beitreten-Adresse (`join_url_of`). Auseinander-
  gelaufene Gruppen sind getrennte Termine, wie in den Ansichten. Zwei echte
  Termine mit derselben Adresse bleiben zwei.
- Verborgene Kalender fallen **vor** dem Zusammenlegen weg.
- Angezeigt wird die Kopie, die die Faltung wählt (`collapse_event_groups`:
  die erste, die man bearbeiten kann). Die Ansichten nehmen die Zeilen in der
  Reihenfolge, in der die Konten antworten; der Kern sortiert sie vorher fest
  (Beginn, Kalender, Id), damit dieselben Daten immer gleich falten. Der
  Schlüssel einer gefalteten Gruppe ist der kleinste Mitglieds-Schlüssel,
  damit er nicht davon abhängt, welche Kopie gerade vorn steht.

### Das Fenster

- Nur Termine (keine Aufgaben), nicht ganztägig, nicht abgesagt (unabhängig von
  der Anzeige-Einstellung für abgesagte), nicht verborgen.
- **Beginn** = der späteste Auslöser des Vorkommens und seiner Kopien, der
  **vor dem Ende** liegt. Auslöser am Ende oder danach zählen nicht. Gibt es
  keinen, gibt es kein Fenster. Erinnerungen nach dem Beginn sind erlaubt.
- **Ende** = das späteste Ende der Kopien.
- **Vorlauf:** Liegt der Beginn des Fensters mehr als 2 Stunden vor dem
  Terminbeginn, gibt es kein Fenster (Entscheidung 11).
- **Feste Erinnerungszeit an einer Serie** (absolute Erinnerung) gehört nur zum
  ersten Vorkommen, das nach ihr endet, wie heute beim Planen. Sonst begänne an
  diesem einen Zeitpunkt ein Fenster für jedes Vorkommen.

Die letzte Erinnerung lässt sich heute nicht ablesen: Der Auslöser-Liste fürs
Handy fehlen alle bereits ausgelösten (`upcoming_reminders_json` behält nur
Zukünftige). Die neue Tür schaut deshalb zurück.

### Ketten und Überblenden

Zu jedem Zeitpunkt zeigt die Aktivität das **laufende Fenster mit dem spätesten
Beginn**. Gleichstand: früherer Terminbeginn, dann Schlüssel. (Kein Titel: Die
Textordnung ist im Kern ein abschaltbares Feature, und für einen Gleichstand
zählt nur, dass dieselben Daten immer dasselbe zeigen.) Daraus folgt:

- Beginnt das Fenster von B, während A läuft, zeigt die Aktivität ab da B.
- Endet B vor A (B liegt mitten in A), zeigt sie danach wieder A.
- Läuft kein Fenster mehr, endet die Aktivität.

Eine **Kette** ist eine lückenlose Folge solcher Abschnitte, und eine Kette ist
**eine** Aktivität. Eine Kette trägt die Schlüssel aller ihrer Fenster. Eine
schon laufende Aktivität gehört zu der Kette, die **einen ihrer** Schlüssel
enthält; so bleibt die Zuordnung, auch wenn der erste Termin der Kette vorbei
ist und aus der Eingabe fällt.

### Der Anschluss

Je Abschnitt: der erste Termin mit Uhrzeit, der nach dem Beginn des Abschnitts
**und** nach dem Beginn des angezeigten Termins beginnt, am selben Tag wie der
angezeigte. Nicht abgesagt, nicht verborgen, nicht ganztägig, keine Kopie des
angezeigten. Ob er eine Erinnerung hat, spielt keine Rolle. Beginnt der
Anschluss, endet der Abschnitt, und der nächste zeigt den folgenden Anschluss.

### Signal: welche Erinnerungen in der Aktivität aufgehen

Ein Abschnitt, der mit dem Beginn eines Fensters beginnt, ist ein
**Signal-Abschnitt**: Er ist die letzte Erinnerung dieses Termins. Er nennt die
Auslöser, die in ihm **aufgehen**: alle Auslöser des angezeigten Vorkommens und
seiner Kopien zu genau diesem Zeitpunkt. Nur sie. Alles andere bleibt eine
gewöhnliche Benachrichtigung, auch:

- frühere Erinnerungen des Termins;
- die Erinnerung eines Termins, dessen Fenster zur selben Zeit beginnt, aber
  den Gleichstand verliert und daher nicht angezeigt wird;
- Erinnerungen von Kopien zu anderen Zeitpunkten.

So ist die Aufteilung eine Mengen-Differenz über Auslöser-Schlüssel (Termin,
Kalender, Vorkommen, Zeitpunkt), die der Kern testet. Ob eine Plattform die
aufgegangenen Auslöser tatsächlich weglässt, entscheidet sie danach (siehe
Android und iOS); im Zweifel bleibt die Benachrichtigung.

### Der Plan

Die Tür (`live_activity_plan_json` in `crates/cal-ffi/src/host.rs`, am Handy
`liveActivityPlan` in `mobile/src/api/liveActivities.ts`) liefert **beides**
aus einem Lesevorgang: die vollständige Liste der Benachrichtigungen (wie
`upcoming_reminders_json`) und den Plan. Jeder Eintrag der Liste sagt dabei
selbst, ob er in einem Abschnitt aufgeht (`absorbed`). Der Vergleich geschieht im
Kern auf Zeitpunkten: Plan und Liste schreiben denselben Zeitpunkt verschieden
(`Z` gegen `+00:00`), und ein Textvergleich in einer Oberfläche träfe nichts.

Der Lesevorgang schaut **einen Tag** zurück, um die letzte Erinnerung eines
laufenden Termins zu finden, und sammelt die Vorkommen bis einen Tag nach dem
Horizont, für den Anschluss. Termine aus dem Gerätekalender liest er dabei mit:
Sie bekommen keine Auslöser (die Erinnerungen stellt das Betriebssystem), sind
aber echte Termine, die als Anschluss oder als Kopie zählen. Lokale Termine sind
nie abgesagt. Der Plan hat je Kette:

- die Schlüssel ihrer Fenster, Beginn und Ende;
- eine Liste von **Abschnitten**. Ein Abschnitt endet, wo sich etwas Sichtbares
  ändert: Ein Fenster beginnt oder endet, der angezeigte Termin beginnt, der
  Anschluss beginnt, oder Android darf ab hier hervorheben.

Jeder Abschnitt trägt: von, bis, den angezeigten Termin (Schlüssel, Id,
Kalender, ursprünglicher Beginn des Vorkommens, Titel, Ort, Beginn, Ende),
seinen Anschluss (Titel, Beginn), ob der Termin schon begonnen hat, ob er
hervorgehoben werden darf, und als Signal-Abschnitt die Schlüssel der
aufgegangenen Auslöser. Ton, Titel und Text der Erinnerung stehen in der
Benachrichtigungs-Liste unter demselben Schlüssel.

**Ort:** Ist der Ort nichts als eine Beitreten-Adresse (bei Meeting-Zeilen
üblich), zeigt die Aktivität keinen. Vorgelesen ist eine URL kein Raum.

Die Texte baut der Kern nicht. Er liefert Zeitpunkte; Wörter und Sprache kommen
wie beim Widget aus den Übersetzungen der App (`locales/`).

## Android

### Ablauf

1. JavaScript holt Benachrichtigungen und Plan im selben Durchlauf, der heute die
   Erinnerungen plant (`rescheduleOnce` in `mobile/src/reminders/scheduler.ts`):
   beim Start, beim Zurückkehren in den Vordergrund, nach jeder Änderung und im
   Hintergrund-Abgleich.
2. JavaScript gibt den Plan mit übersetzten Textvorlagen und der Sprache an
   Kotlin (`scheduleLiveActivities`).
3. **Erst wenn Kotlin ihn angenommen hat**, lässt JavaScript die aufgegangenen
   Auslöser bei Expo weg. Scheitert die Übergabe oder ist der Schalter aus,
   plant Expo alle, wie heute.
4. Kotlin legt den Plan in `filesDir` ab (atomar, wie `WidgetStore.kt`) und
   stellt **einen** genauen Alarm: für die nächste Abschnittsgrenze. Der
   Empfänger stellt danach den folgenden. So bleiben es wenige Alarme, auch bei
   vollem Kalender.
5. Der Alarm-Empfänger liest **nur** die abgelegten Dateien und zeigt, ändert
   oder entfernt die Benachrichtigung. Er öffnet den Kern nicht: Das wäre teuer
   und ist für solche Einstiege verworfen (TODO A7).
6. Uhrzeiten formatiert Kotlin beim Zeigen aus den Zeitpunkten, mit Sprache aus
   Aperio und Region vom Gerät, wie das Widget. Nach einem Zonenwechsel stimmen
   sie so sofort; nur der „selbe Tag“ des Anschlusses folgt erst beim nächsten
   Durchlauf.
7. Ein Empfänger für Neustart (`BOOT_COMPLETED`), App-Update
   (`MY_PACKAGE_REPLACED`) und Zeit- oder Zonenwechsel stellt den Alarm neu und
   zeigt den laufenden Abschnitt neu.

### Ein Signal

- Ein **Signal-Abschnitt** ist eine **neue** Benachrichtigung (neue Id), auf dem
  Kanal der Erinnerung und mit ihrem Ton. Eine neue Benachrichtigung sagt das
  System an, auch auf dem stummen Kanal. Beim Überblenden verschwindet die alte
  und die neue erscheint; genau das täte die Erinnerung des nächsten Termins
  heute auch.
- Jeder andere Abschnitt ändert die bestehende Benachrichtigung **still**
  (`setOnlyAlertOnce`, `setSilent`), auch wenn sie inzwischen fehlt: Er meldet
  sich nie ein zweites Mal.

### Nachholen

Kotlin führt eine eigene Datei mit den Signalen, die es **wirklich gezeigt** hat.
Liegt ein Signal-Abschnitt beim Stellen schon in der Vergangenheit:

- noch nicht gezeigt und höchstens 5 Minuten alt (wie `CATCH_UP_GRACE` beim
  Nachholen von Erinnerungen): mit Ton;
- sonst still.

Wird der Schalter gerade eingeschaltet, hatte Expo die vergangenen
Erinnerungen noch; die erste Übergabe danach markiert vergangene Signale als
gezeigt.

### Texte und TalkBack

- Titel: der Termin.
- Text, vor Beginn: „Beginnt um 10:00 · Raum 2.14 · Danach: Review um 11:00“;
  ab Beginn: „Läuft bis 11:00 · …“.
- **Uhrzeiten statt „in 10 Minuten“**, denn TalkBack liest den Countdown in der
  Ansage nicht und der Text ändert sich nur an Abschnittsgrenzen. Ein „in 10
  Minuten“ wäre eine Minute später falsch.
- Der Countdown (`setUsesChronometer`, `setChronometerCountDown`, `setWhen`)
  läuft sichtbar mit: bis zum Beginn, dann bis zum Ende.
- `CATEGORY_EVENT`, damit TalkBack „Termin“ voranstellt; `tickerText` gleich dem
  Text.

### Hervorhebung (Live Update)

Wegen Googles Leitlinie bittet die Benachrichtigung erst **15 Minuten vor
Beginn** um Hervorhebung, und bis zum Ende. Liegt die letzte Erinnerung früher,
ist sie bis dahin eine gewöhnliche laufende Benachrichtigung. Den Zeitpunkt
rechnet der Kern (eine Konstante dort), Kotlin setzt nur das Kennzeichen. Nie
`setColorized(true)`: Das war das Signal der ersten Android-16-Fassung und
verhindert die Hervorhebung ab QPR1.

### Wegwischen

Ab Android 14 lässt sich die Benachrichtigung wegwischen. Ein `deleteIntent`
schreibt den angezeigten Vorkommens-Schlüssel in eine **eigene** Datei, die nur
Kotlin schreibt; eine neue Übergabe des Plans ersetzt sie nicht. Einträge fallen
weg, wenn das Vorkommen vorbei ist.

- Dieses Vorkommen kommt nicht wieder, auch nicht beim nächsten Öffnen.
- Der nächste Termin mit eigenem Signal-Abschnitt erscheint trotzdem, denn er
  ist dessen Erinnerung. Wie auf iOS.
- Ein `deleteIntent` zur geplanten Endzeit oder danach zählt nicht als
  Wegwischen. Ein Zeitlimit (`setTimeoutAfter`) gibt es höchstens bis zum Ende
  der Kette, als Netz; entfernt wird über den Alarm.

### Genaue Alarme

Wie Expo: `canScheduleExactAlarms()` prüfen und sonst ungenau stellen. Alternativ
`SCHEDULE_EXACT_ALARM` mit `maxSdkVersion=32` für Android 12 erklären (braucht
ein Konfigurations-Plugin). Ohne eins von beiden ginge auf Android 12 die letzte
Erinnerung verloren, denn sie hängt dann nur am Kotlin-Alarm.

### Antippen

Ein `PendingIntent` direkt auf die Aktivität der App (Android 12 verbietet den
Umweg über einen Empfänger), mit Termin, Kalender und Beginn des Vorkommens.
Die App liest ihn selbst aus, denn an Expos Benachrichtigungs-Weiterleitung geht
er vorbei.

### Gebaut (Stufe 5)

- **Übergabe:** `shared/liveActivityAndroid.ts` (`androidHandover`, mit vitest
  geprüft) macht aus dem Plan eine flache, zeitlich geordnete Liste von
  Abschnitten, Zeitpunkte als Millisekunden, je Abschnitt der Kanal seiner
  Erinnerung und, wenn er eine ist, eine feste Signal-Id. JavaScript
  (`mobile/src/liveActivities/androidActivities.ts`) übergibt sie im
  Erinnerungs-Durchlauf; erst danach lässt Expo die aufgegangenen Erinnerungen
  weg. Scheitert die Übergabe, bleiben alle bei Expo.
- **Benachrichtiger:** `LiveActivityNotifier.kt` legt den Plan ab und stellt
  genau einen Alarm, für die nächste Grenze. Eigene Dateien für die Signale, die
  geklingelt haben, für weggewischte Termine und für die angezeigte
  Benachrichtigung; eine neue Übergabe vergisst davon nichts.
- **Ein Signal:** Klingelt jede fällige, noch nicht erklungene Erinnerung der
  laufenden Kette, auch wenn ein Alarm zu spät kam und ihr Abschnitt schon
  vorbei ist: verspätet, aber nie verloren (keine 5-Minuten-Gnade mehr, denn
  die hätte eine späte Erinnerung stumm gemacht, wo Expo sie hörbar gebracht
  hätte). Eine neue Benachrichtigung auf dem Kanal der Erinnerung; jeder andere
  Abschnitt ändert die angezeigte still, ein unveränderter wird nicht neu
  gepostet. Gemerkt wird je Erinnerung, als Paar aus Termin-Id und Zeitpunkt
  (dem Schlüssel, nach dem die Liste ihre Doppel zusammenlegt), nicht je
  Terminschlüssel, der sich mit den Kopien ändern kann. JavaScript sagt bei jeder
  Übergabe, welche vergangenen Erinnerungen Expo noch hielt und also brachte
  (`rangByExpo`); die zählen als erklungen. Die Markierung `absorbed` gilt für
  die ganze Doppel-Gruppe eines Listeneintrags.
- **Scheitern:** Scheitert die Übergabe, hält der Benachrichtiger nichts mehr
  (Plan, Alarm und Anzeige weg), und Expo bekommt alle Erinnerungen zurück; die
  nächste Übergabe zählt dann die ganze Vergangenheit als Expos.
- **Text:** Uhrzeiten in Aperios Sprache mit Region und 12/24-Stunden-Wahl des
  Geräts, Wörter aus Aperio. Countdown sichtbar daneben, `CATEGORY_EVENT`,
  kein `tickerText` (er ginge auch gesperrt in die Ansage), `setTimeoutAfter`
  bis zum Ende der Kette als Netz; ein Wegwischen zur oder nach der geplanten
  Endzeit zählt nicht. Vor Android 8 setzt die Benachrichtigung Ton und
  Priorität selbst, weil es keine Kanäle gibt.
- **Wegwischen** gilt für den Termin und den Rest seiner Kette, bis eine eigene
  Erinnerung kommt; eine Erinnerung, die der Nutzer noch nicht gesehen hat,
  kommt trotzdem.
- **Hervorhebung:** die Bitte (`android.requestPromotedOngoing`) für Abschnitte ab
  15 Minuten vor Beginn, nie eingefärbt; `POST_PROMOTED_NOTIFICATIONS` im
  Manifest des Moduls. Die Bitte wirkt erst ab Android 16 QPR1 (36.1). Darf
  Aperio dort nicht hervorheben, sagt das eine Zeile unter dem Schalter, mit
  einem Knopf zur Systemseite.
- **Alarme:** genau, wo erlaubt (`USE_EXACT_ALARM` ab Android 13,
  `SCHEDULE_EXACT_ALARM` bis 12), sonst ungenau. Der nächste Alarm wird vor dem
  Anzeigen gestellt, damit ein Fehler die Kette nicht reißt; alle Einstiege
  sind synchronisiert. Ein Empfänger für Neustart, App-Update und Zeit- oder
  Zonenwechsel zeigt den laufenden Abschnitt neu und stellt den Alarm.
- **Antippen:** dieselbe Adresse wie auf dem iPhone, `aperio://event?…`, jeder
  Wert mit `Uri.encode`; gelesen vom `ReminderTapRouter` über Linking.

## iOS

### Ab welcher Version

- **iOS 16:** keine Aktivität. Die Widget-Erweiterung, die sie zeichnet, braucht
  iOS 17. Alle ActivityKit-Aufrufe hinter `#available(iOS 17, *)`.
- **iOS 17:** Start beim Öffnen; Zeiten als Uhrzeiten (ein Countdown stünde nach dem Beginn bei null).
- **iOS 18:** dazu die mitlaufenden Angaben („in 10 Minuten“).
- **iOS 26:** dazu das Einplanen.

### Der Abgleich

Aperio merkt sich (gerätelokal) je Aktivität ihre Id, die Schlüssel ihrer
Kette, den geplanten Start und ob Aperio sie selbst beendet hat. Bei jedem
Durchlauf vergleicht es das mit `Activity.activities` und dem Plan:

- **Laufend und passt zu einer Kette** (über einen gemeinsamen Schlüssel): bei
  geändertem Inhalt ändern.
- **Laufend, aber keine Kette mehr** (Termin gelöscht, verschoben, verborgen):
  beenden, `.immediate`.
- **Eingeplant** (iOS 26): Stimmen erster Schlüssel und Start mit der Kette
  überein, bleibt sie. Sonst beenden; im Vordergrund neu einplanen.
- **Von iOS beendet** (8-Stunden-Grenze): zählt als keine Aktivität.
- **Fehlt, obwohl Aperio sie nicht beendet hat**, vor der 8-Stunden-Grenze: hat
  der Nutzer sie weggewischt (siehe unten).
- **Läuft eine Kette und hat keine Aktivität:** im Vordergrund sofort starten,
  ohne Alarm. Nicht für ein weggewischtes Vorkommen.
- **iOS 26, im Vordergrund:** die nächsten **zwei** Ketten einplanen, mehr nicht,
  weil eingeplante zum Limit zählen.

Im **Hintergrund-Abgleich** darf Aperio nur ändern und beenden. Er kommt, wann
iOS will; er räumt auf, taktet aber nichts.

**Kaltstart nach Tipp auf die Erinnerung:** Der volle Durchlauf liest jedes Konto
live und kann Sekunden dauern; wer kurz schaut und sperrt, wäre schneller weg.
Deshalb bleibt der letzte Plan auf dem Gerät, und sobald die App „aktiv“ ist,
startet Aperio die laufende Kette **aus ihm**, vor dem vollen Durchlauf. Der
gleicht danach ab.

### Ein Signal auf iOS 26

- Welche Erinnerungen der Durchlauf **nicht** als Benachrichtigung plant,
  ergibt sich in **jedem** Durchlauf, auch im Hintergrund, aus den eingeplanten
  Aktivitäten, die ActivityKit wirklich meldet und die zum Plan passen (erster
  Schlüssel und Start). Nicht daraus, dass das Einplanen früher einmal klappte.
- Fehlt eine eingeplante Aktivität vor ihrem Start, oder sind Live-Aktivitäten
  in den iOS-Einstellungen aus, plant der Durchlauf die Benachrichtigung wieder;
  das darf auch der Hintergrund.
- **Stumme Erinnerungen** plant Aperio nicht als Aktivität ein (der Alarm kann
  nicht stumm sein). Sie bleiben Benachrichtigungen; die Aktivität startet dann
  beim Öffnen, wie unter iOS 26.
- Die Erinnerung des **zweiten** Termins einer Kette bleibt eine
  Benachrichtigung: Ein `staleDate`-Wechsel kann keinen Alarm auslösen.

### Der Inhalt und der eine Wechsel

Der Inhalt enthält den **aktuellen** Abschnitt mit seinem Anschluss und, falls
die Kette überblendet, den **nächsten** Termin mit **dessen** Anschluss.
`staleDate` ist der Beginn des nächsten Abschnitts mit anderem Termin, und ohne
einen solchen das Ende der Kette:

- vor `staleDate`: der aktuelle Termin;
- ab `staleDate`: der nächste Termin, oder, wenn die Kette keinen hat, derselbe
  Termin als „vorbei“.

Weitere Wechsel (eine dritte Station, ein neuer Anschluss, das Ende des zweiten
Termins) gibt es erst, wenn Aperio wieder läuft. Die mitlaufenden Angaben
bleiben dabei wahr („Ende vor 10 Minuten“); ein überholter Anschluss steht bis
dahin weiter da.

`relevanceScore` steigt mit dem Beginn der Kette: Eine spätere Kette bekommt
immer die Dynamic Island, nicht eine ältere, die schon „vorbei“ zeigt. Beendet
Aperio eine fertige Kette, dann `.immediate`.

### Darstellung

- **Sperrbildschirm:** Titel; „10:00 bis 11:10“; „Beginn in 10 Minuten · Ende
  in 1 Stunde“ (ab iOS 18); Ort; „Danach: Review um 11:00“.
- **Dynamic Island, kompakt:** links ein Symbol, rechts die Beginnzeit („10:00“).
  Eine feste Uhrzeit bleibt ohne Wechsel wahr; ein Countdown stünde nach dem
  Beginn bei 0:00.
- **Minimal:** die Beginnzeit.
- **Erweitert:** Titel in der Mitte, Zeiten, Ort und Anschluss unten (nicht in
  den Seitenbereichen, dort verrutscht der VoiceOver-Fokus, ein von Apple
  bestätigter Fehler).
- **Sprache aus Aperio, Region vom Gerät** (`localeFor` in `Formatting.swift`),
  damit 24-Stunden-Uhr und Datumsreihenfolge dem Gerät folgen. Wörter übersetzt
  aus der App, wie beim Widget.
- **Ausnahme: die mitlaufenden Angaben** („in 59 Minuten“). Sie zeichnet iOS
  selbst, in der Sprache der Erweiterung; die Sprache, die die Ansicht ihnen
  mitgibt, erreicht sie nicht, und `SystemFormatStyle.DateReference` nimmt
  keine. Eine Erweiterung ohne erklärte Sprachen spricht dabei die
  Entwicklersprache: Am 2026-11-08 stand auf Tonis deutschem iPhone „Beginn in
  59 minutes“. Darum erklärt `mobile/targets/widget/Info.plist` dieselben
  Sprachen wie die App (`CFBundleLocalizations` en, de); die mitlaufenden
  Angaben folgen damit der Sprache des Geräts. Läuft Aperio in einer anderen
  Sprache als das Gerät, sprechen nur sie die des Geräts.
- Antippen öffnet den Termin (`widgetURL`).

### VoiceOver

Je Darstellung **ein** zusammengefasstes Element, das sich wie ein Satz liest:
„Standup, 10:00 bis 11:10, Beginn in 10 Minuten, Ende in 1 Stunde, Raum 2.14,
danach Review um 11:00“. Keine festen `accessibilityLabel`-Texte mit relativen
Zeiten: Die Ansicht wird archiviert, ein fester Text wäre sofort veraltet. Wie
VoiceOver die mitlaufenden Angaben liest, ist nirgends beschrieben und muss auf
Tonis iPhone geprüft werden.

### Wegwischen

ActivityKit sagt nicht, ob der Nutzer oder das System entfernt hat, und sagt es
nur, solange Aperio läuft. Deshalb gilt als weggewischt: eine Aktivität, die
Aperio gestartet und nicht selbst beendet hat und die vor der 8-Stunden-Grenze
fehlt. Gemerkt wird der damals angezeigte **Vorkommens-Schlüssel**, wie auf
Android: Dieses Vorkommen kommt nicht wieder; der nächste Termin der Kette kommt
beim nächsten Öffnen.

### Technik

- `NSSupportsLiveActivities` in `mobile/app.json` (`ios.infoPlist`).
- Eine `ActivityConfiguration` in der vorhandenen Widget-Erweiterung
  (`AperioWidgetBundle` in `mobile/targets/widget/index.swift`). Keine neue
  App-Id, keine neuen Zugangsdaten bei EAS.
- Die `ActivityAttributes`-Struktur muss in App und Erweiterung **gleich**
  sein. Es gibt keinen Weg, eine Swift-Datei zwischen dem `CalFfi`-Pod und der
  Erweiterung zu teilen. Deshalb zwei Kopien mit Textwächter, wie bei den
  Widget-Strukturen (`src/intl/widgetSnapshotWire.test.ts`).
- Start, Einplanen, Ändern, Beenden und Auflisten als Funktionen in
  `CalFfiModule.swift`; Android bekommt die Gegenstücke für seinen Plan, das
  Brücken-Skript (`mobile/scripts/check-ffi-bridges.mjs`) wacht darüber.

**Gebaut (Stufe 4):**

- **Die Regeln des Abgleichs** liegen als reine Funktion in
  `shared/liveActivityIos.ts` (`reconcileIosActivities`, mit vitest geprüft):
  Was ActivityKit meldet und was Aperio sich merkt, geht hinein; Aktionen
  (starten, einplanen, ändern, beenden) und das neue Gedächtnis kommen heraus.
  Swift führt nur aus (`LiveActivityBridge.swift`; JS-Seite
  `mobile/src/liveActivities/iosActivities.ts`, eingehängt in den
  Erinnerungs-Durchlauf).
- **Ein Signal:** Weggelassen werden nur die Erinnerungen einer eingeplanten
  Aktivität, die ActivityKit gerade meldet und die noch passt, oder einer, die
  in diesem Durchlauf erfolgreich eingeplant wurde. Scheitert irgendetwas,
  kommt die Liste aus dem gewöhnlichen Lesevorgang, und alles klingelt.
- **Gedächtnis** (gerätelokal): je Aktivität ihre Schlüssel, der zuletzt
  gezeigte Termin, der Start und der zuletzt gesendete Inhalt. Unveränderter
  Inhalt wird nicht noch einmal gesendet; jedes Update zählt bei der Apple
  Watch gegen ein Budget.
- **Kaltstart:** Der letzte Plan bleibt gespeichert; bei jeder Rückkehr startet
  er die laufende Kette sofort, plant aber nichts ein. Der volle Durchlauf
  gleicht danach ab.
- **Attribute** zweimal byte-gleich (`LiveActivityAttributes.swift` im Pod und
  in der Erweiterung), bewacht von `src/intl/liveActivityWire.test.ts`, das
  auch die Felder gegen die geschriebenen JSON-Schlüssel prüft. Zeitpunkte
  reisen als Text.
- **Einplanen** mit `startDate:` statt `start:`: Apple hat das Etikett nach dem
  ersten iOS-26-SDK umbenannt und beide auf 26.0 zurückdatiert; das alte baut
  auf jedem iOS-26-SDK. Der Zustand wird über seinen Namen gelesen, denn
  `.pending` gibt es erst ab iOS 26.
- **Mitlaufende Zeit** ab iOS 18: `Text(.currentDate, format: .reference(to:
  allowedFields: [.day, .hour, .minute]))`, laut Dokumentation bis zu zwei
  Einheiten.
- **Antippen:** `aperio://event?eventId=…&calendarId=…&start=…&title=…`, jeder
  Wert bis auf Buchstaben und Ziffern kodiert (`URLQueryItem` ließe `&` und `=`
  stehen); gelesen von `reminderTapOfUrl`, geöffnet vom `ReminderTapRouter`.
- **Schalter** vorerst nur auf iOS; Android bekommt ihn mit Stufe 5. Nur auf
  einem iPhone ab iOS 17: Das iPad hat keine Live-Aktivitäten. Nach dem Weg
  über den Knopf zu den iPhone-Einstellungen sagt Aperio, wie es steht.
- **Aus der Prüfrunde:** Eine wartende Aktivität bleibt nur, solange sie zu den
  frühesten zwei Ketten gehört und ihre Erinnerung nicht stumm ist; hat sich ihr
  Inhalt geändert, wird sie im Vordergrund neu eingeplant. Gemerkt werden die
  Schlüssel der Kette, wie sie jetzt ist (nicht alle je gesehenen), und eine
  laufende Aktivität springt nie zu einer Kette, die noch nicht begonnen hat.
  Sind Live-Aktivitäten aus, gilt nichts als weggewischt. Der Abgleich läuft
  auch ohne Benachrichtigungs-Berechtigung. Beide Zeitangaben stehen auf einer
  Zeile, Ort und Anschluss sind einzeilig (Apple begrenzt auf 160 Punkte), und
  die erweiterte Insel ist ein einziges Element mit Titel.
- **Sperrbildschirm:** Aperio liest die Vorschau-Einstellung der
  Mitteilungen; steht sie nicht auf „Immer“, sind Titel, Ort und Anschluss
  `privacySensitive` (Feld `hideOnLockScreen`).

## Antippen öffnet den Termin

Eine Aktivität, die beim Antippen nur Aperio nach vorn holt, wäre halb. Und mit
„einem Signal“ wäre es unvorhersagbar: Die letzte Erinnerung (jetzt die
Aktivität) öffnete den Termin, die früheren nicht. Deshalb kommt die
Handy-Hälfte von TODO A5 („Tippen auf eine Erinnerung öffnet den Termin“) als
eigene Stufe vor den Plattform-Stufen: Benachrichtigungen tragen Kalender und
Beginn des Vorkommens, ein Antwort-Hörer (auch beim Kaltstart) öffnet den
Termin, sobald Navigation, Zustand und App-Sperre bereit sind. Dieselbe Route
bedient die Aktivität (iOS über `widgetURL`, Android über den eigenen
`PendingIntent`). Die Desktop-Hälfte von A5 (Klick auf den Windows-Toast) bleibt
in A5.

## Einstellungen

- Schalter „Live-Aktivitäten für Termine“ im Abschnitt Benachrichtigungen der
  allgemeinen Einstellungen, standardmäßig an, gerätelokal. Der Durchlauf liest
  ihn bei jedem Lauf neu (wie die verborgenen Kalender), auch im Hintergrund, wo
  keine Oberfläche einen Zwischenspeicher füllt. Umschalten plant sofort neu; aus
  heißt: alle Aktivitäten beenden, Plan leeren, alle Erinnerungen wieder über
  Expo.
- **iOS:** Ist der iOS-Schalter für Aperio aus (`areActivitiesEnabled`), steht
  darunter ein kurzer Hinweis mit Knopf zu Aperios iOS-Einstellungen, und Aperio
  versucht nichts.
- **Android:** Darf Aperio nicht hervorheben (`canPostPromotedNotifications`),
  steht darunter ein Hinweis mit Knopf zur Systemeinstellung dafür. Die laufende
  Benachrichtigung gibt es trotzdem.

## Sperrbildschirm

- **Android:** Die laufende Benachrichtigung hat die Sichtbarkeit des
  Erinnerungs-Kanals; was dort von Erinnerungen zu sehen ist, ist es auch hier.
- **iOS:** Eine Live-Aktivität folgt der Einstellung „Vorschauen zeigen“ nicht
  von selbst. Aperio liest sie (`showPreviewsSetting`); steht sie nicht auf
  „Immer“, markiert die Ansicht Titel und Ort als privat (`privacySensitive`).
  Ob iOS das in Live-Aktivitäten wirklich schwärzt, ist ungeprüft.

## Was es NICHT löst

- **Termine aus dem Gerätekalender** bekommen keine Aktivität: Ihre
  Erinnerungen stellt das Betriebssystem, Aperio zählt sie nicht auf. Folgepunkt.
- **Aufgaben** bekommen keine Aktivität.
- **iOS ohne Öffnen:** Wer Aperio tagelang nicht öffnet, bekommt ab iOS 26 nur
  die zwei eingeplanten Ketten, auf älteren iOS-Versionen gar keine.
- **Ketten über 8 Stunden** (ein Tag voller Besprechungen) beendet iOS mitten
  drin. Beim nächsten Öffnen startet die laufende Kette neu.
- **iOS 16** bekommt keine Aktivität.

## Stufenliste

Jede Stufe ist ein PR mit Tests und Gegenprobe. Die Geräte-Stufen kosten je
einen Build und laufen blind, deshalb steht vorn, was sich ohne Gerät prüfen
lässt.

1. **Die Regel im Kern.** `cal-core::live_activity` mit eigenen Eingabetypen:
   Vorkommens-Schlüssel, Kopien, Fenster, Ketten, Abschnitte, Anschluss,
   Hervorhebungs-Zeitpunkt, Signal-Abschnitte mit aufgegangenen Auslösern.
   Reine Funktion mit Tests, auch: Zeitumstellung im Fenster, Mitternacht beim
   Anschluss, Erinnerung nach dem Ende, ineinander liegende Termine, Gleichstand
   zweier Fenster, Kette A→B→C, Kopien mit gleichen und verschiedenen
   Erinnerungen, Meeting plus Einladung, absolute Erinnerung an einer Serie,
   Horizont-Rand, Schlüssel bleibt, wenn der erste Termin vorbei ist.
2. **Die Tür.** Eine öffentliche Aufklapp-Funktion in Rust (mit verschobenen und
   abgesagten Vorkommen) statt der privaten Kopie in `host-core/reminders.rs`;
   Auslöser mit Ort; Aufzählung schaut zurück; ein Lesevorgang liefert
   Benachrichtigungen und Plan (`liveActivityPlanJson` in
   `crates/cal-ffi/src/host.rs`); Brücken iOS und Android; TS-Typ und Hülle in
   `mobile/src/api/`.
3. **Antippen öffnet den Termin** (Handy-Hälfte von A5), für Erinnerungen;
   die Route nimmt später die Aktivitäten auf.
4. **iOS.** Erweiterung (Ansichten, `ActivityConfiguration`), Info.plist,
   Attribut-Struktur mit Wächter, Brücke, Abgleich im Erinnerungs-Durchlauf
   (Vordergrund, Hintergrund, Kaltstart aus dem gemerkten Plan),
   iOS-26-Einplanung mit einem Signal, Wegwischen, Schalter und iOS-Hinweis,
   Texte en/de, Hilfe-Seite. TestFlight-Build.
5. **Android.** Plan-Ablage, Signal- und Wegwisch-Datei, Alarm- und
   Neustart-Empfänger, Benachrichtigungs-Bau (Countdown, Hervorhebung, neu oder
   still), genaue Alarme mit Rückfall, Antippen, ein Signal im Durchlauf mit
   Rückfall auf Expo, Hervorhebungs-Hinweis, `POST_PROMOTED_NOTIFICATIONS`,
   Hilfe-Seite. Play-Build.

## Risiken

- **Ein Signal verlegt Erinnerungen.** Auf Android kommt die letzte Erinnerung
  eines Termins dann aus Aperios eigenem Code statt aus Expo. Ein Fehler dort
  verschluckt eine Erinnerung. Gegenmittel: Die Aufteilung ist eine getestete
  Mengen-Differenz im Kern; Expo lässt nur weg, was Kotlin angenommen hat;
  Nachholen mit Ton; Neustellen bei Neustart und Update; dieselben Kanäle.
- **iOS 26: eine eingeplante Aktivität kann verschwinden**, ohne dass Aperio es
  erfährt, solange es nicht läuft: bei einem Neustart (ungeprüft), wenn der
  Nutzer Live-Aktivitäten abschaltet, wenn iOS sie verwirft. Dann fehlt ihre
  Erinnerung bis zum nächsten Durchlauf. Zeigt Tonis Gerätetest, dass
  eingeplante einen Neustart nicht überleben, wird auf iOS nicht mehr
  weggelassen (zwei Signale) — das ist dann Tonis Entscheidung.
- **Veraltete Pläne.** Ändert sich ein Termin auf einem anderen Gerät, sieht die
  Aktivität es erst beim nächsten Durchlauf. Das gilt heute schon für
  Erinnerungen.
- **Googles Leitlinie** spricht von vom Nutzer ausgelösten Live Updates, nicht
  von Einladungen anderer. Die 15-Minuten-Regel deckt „gleich beginnend“ ab;
  Einladungen bleiben eine Lücke, die Google oder Gerätehersteller durchsetzen
  könnten.
- **Gerätehersteller** (Samsung, Xiaomi) können eigene Regeln für die
  Hervorhebung haben.

## Ungeprüft

Muss auf Tonis Geräten geprüft werden, weil es nirgends beschrieben ist:

- wie VoiceOver die mitlaufenden Angaben und die Dynamic Island liest, und ob es
  den Start-Alarm einer eingeplanten Aktivität ansagt;
- ob `Activity.request` beim Kaltstart nach einem Tipp auf die Erinnerung schon
  gelingt oder erst, wenn die App aktiv ist (geplant: erst bei „aktiv“);
- ob eingeplante und laufende Aktivitäten einen Neustart überleben, und wie weit
  im Voraus iOS einplanen lässt;
- ob `privacySensitive` eine Live-Aktivität auf dem gesperrten iPhone schwärzt;
- wie TalkBack die Benachrichtigung im Schirm, die Statusleisten-Anzeige und die
  hervorgehobene Benachrichtigung auf dem Sperrbildschirm liest;
- ob das Android-Gerät die Hervorhebung zeigt (Android 16 QPR1 oder neuer).
