import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { AnnouncerProvider, useAnnouncer } from './a11y/Announcer';
import { TitleBar } from './components/TitleBar';
import { useSuppressBrowserDefaults } from './hooks/useSuppressBrowserDefaults';
import { useDateFormat } from './intl/dateFormat';
import {
  createCalendar,
  createEvent,
  deleteCalendar,
  getEvents,
  isCommandError,
  listCalendars,
} from './api/client';
import type { Calendar, CalendarEvent } from './api/types';

type AppInfo = {
  name: string;
  version: string;
};

export function App() {
  useSuppressBrowserDefaults();
  return (
    <AnnouncerProvider>
      <Shell />
    </AnnouncerProvider>
  );
}

/**
 * Phase 1+2 app shell.
 *
 * Architectural markers that must survive future refactors:
 *  - `role="application"` on the root element (DESIGN.md section 3.2.1)
 *    so screen readers stay in focus mode.
 *  - `aria-label` carrying the product name.
 *  - Global polite + assertive `aria-live` regions, supplied by
 *    [`AnnouncerProvider`].
 *  - Custom title bar (no system decorations), platform-aware button
 *    placement.
 *
 * The Phase 1 smoke screen is still the body. Phase 3 replaces it with
 * the real calendar views.
 */
function Shell() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const fmt = useDateFormat();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [selectedCalendar, setSelectedCalendar] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleError = useCallback(
    (err: unknown) => {
      if (isCommandError(err)) {
        setError(`${err.code}: ${err.message}`);
        announce(`${err.code}: ${err.message}`, 'assertive');
      } else {
        setError(String(err));
        announce(String(err), 'assertive');
      }
    },
    [announce],
  );

  useEffect(() => {
    invoke<AppInfo>('app_info').then(setInfo).catch(handleError);
    listCalendars()
      .then((list) => {
        setCalendars(list);
        if (list.length > 0) {
          setSelectedCalendar(list[0].id);
        }
      })
      .catch(handleError);
  }, [handleError]);

  useEffect(() => {
    if (!selectedCalendar) {
      setEvents([]);
      return;
    }
    const start = new Date();
    start.setHours(0, 0, 0, 0);
    const end = new Date(start.getTime() + 30 * 24 * 60 * 60 * 1000);
    getEvents({
      calendar_id: selectedCalendar,
      start: start.toISOString(),
      end: end.toISOString(),
    })
      .then(setEvents)
      .catch(handleError);
  }, [selectedCalendar, handleError]);

  const handleCreateCalendar = useCallback(async () => {
    setError(null);
    try {
      const cal = await createCalendar({
        name: `Calendar ${calendars.length + 1}`,
        color_hex: '#1e88e5',
      });
      setCalendars((prev) => [...prev, cal]);
      setSelectedCalendar(cal.id);
      announce(t('app.phase1.calendarCreated', { name: cal.name }));
    } catch (err) {
      handleError(err);
    }
  }, [calendars.length, announce, handleError, t]);

  const handleCreateEvent = useCallback(async () => {
    if (!selectedCalendar) return;
    setError(null);
    try {
      const start = new Date();
      const end = new Date(start.getTime() + 60 * 60 * 1000);
      const ev = await createEvent({
        calendar_id: selectedCalendar,
        title: t('app.phase1.smokeEventTitle'),
        description: null,
        location: null,
        start: start.toISOString(),
        end: end.toISOString(),
        all_day: false,
        recurrence: null,
        color_label: null,
        reminders: [],
        sound: null,
        attendees: [],
      });
      setEvents((prev) => [...prev, ev]);
      announce(t('app.phase1.eventCreated', { title: ev.title }));
    } catch (err) {
      handleError(err);
    }
  }, [selectedCalendar, announce, handleError, t]);

  const handleDeleteCalendar = useCallback(
    async (id: string) => {
      setError(null);
      try {
        await deleteCalendar(id);
        setCalendars((prev) => prev.filter((c) => c.id !== id));
        if (selectedCalendar === id) {
          setSelectedCalendar(null);
          setEvents([]);
        }
        announce(t('app.phase1.calendarDeleted'));
      } catch (err) {
        handleError(err);
      }
    },
    [selectedCalendar, announce, handleError, t],
  );

  return (
    <div
      id="app-root"
      role="application"
      aria-label="Aperio"
      className="app-root"
    >
      <TitleBar />
      <header className="app-header">
        <h1>{t('app.title')}</h1>
        {info && <span className="app-version">v{info.version}</span>}
      </header>

      <main className="app-main">
        <p className="app-intro">{t('app.phase1.intro')}</p>

        <section aria-labelledby="cal-heading" className="panel">
          <h2 id="cal-heading">{t('app.phase1.calendars')}</h2>
          {calendars.length === 0 ? (
            <p>{t('app.phase1.noCalendars')}</p>
          ) : (
            <ul role="list" className="cal-list">
              {calendars.map((cal) => (
                <li key={cal.id} role="listitem">
                  <button
                    type="button"
                    aria-pressed={cal.id === selectedCalendar}
                    onClick={() => setSelectedCalendar(cal.id)}
                  >
                    {cal.name}
                  </button>
                  <button
                    type="button"
                    aria-label={t('app.phase1.deleteCalendarA11y', {
                      name: cal.name,
                    })}
                    onClick={() => handleDeleteCalendar(cal.id)}
                  >
                    ✕
                  </button>
                </li>
              ))}
            </ul>
          )}
          <button type="button" onClick={handleCreateCalendar}>
            {t('app.phase1.newCalendar')}
          </button>
        </section>

        <section aria-labelledby="evt-heading" className="panel">
          <h2 id="evt-heading">{t('app.phase1.events')}</h2>
          {!selectedCalendar ? (
            <p>{t('app.phase1.selectCalendarFirst')}</p>
          ) : events.length === 0 ? (
            <p>{t('app.phase1.noEvents')}</p>
          ) : (
            <ul role="list">
              {events.map((ev) => (
                <li key={ev.id} role="listitem">
                  <strong>{ev.title}</strong>
                  <span> — {fmt.format(new Date(ev.start), 'PPpp')}</span>
                </li>
              ))}
            </ul>
          )}
          <button
            type="button"
            disabled={!selectedCalendar}
            onClick={handleCreateEvent}
          >
            {t('app.phase1.newEvent')}
          </button>
        </section>

        {error && (
          <p role="alert" className="error">
            {error}
          </p>
        )}
      </main>
    </div>
  );
}
