import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  emptyDayLog,
  toggleDayMarker,
  type DayLog,
} from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { getDayLog, isCommandError, setDayLog } from '../api/client';
import { useDayMarkersChanged } from '../state/dayMarkersChanged';
import { useDayMarkers } from '../state/useDayMarkers';
import { Modal } from './Modal';

/**
 * Tick a day with the user's own markers.
 *
 * One checkbox per marker, in the order the user arranged them, all of them
 * live: a tick writes straight through rather than waiting for a Save. The
 * whole point of the feature is that recording a day costs almost nothing, and
 * a confirm step at the end would be most of its cost.
 *
 * That makes the dialog's "Close" genuinely a close, not a cancel — which is
 * why there is no Cancel button offering an undo that does not exist. Unticking
 * is the undo.
 */
export function DayLogDialog({
  isOpen,
  onClose,
  day,
  dayLabel,
}: {
  isOpen: boolean;
  onClose: () => void;
  /** Local day key, `YYYY-MM-DD`. */
  day: string;
  /** The day as the user reads it, for the dialog title. */
  dayLabel: string;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { markers, loading: loadingMarkers } = useDayMarkers();
  const [log, setLog] = useState<DayLog>(() => emptyDayLog(day));
  const [error, setError] = useState<string | null>(null);
  // Writes this dialog started and has not seen land yet. A tick's own write
  // raises the same signal a foreign one does, and re-reading in the middle of
  // our own burst would paint the state we just left behind.
  const writesInFlight = useRef(0);
  const introId = useId();
  const introRef = useRef<HTMLParagraphElement | null>(null);

  // Load the day each time the dialog opens, not once per mount: the same
  // dialog serves whichever day the user is standing on.
  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    setLog(emptyDayLog(day));
    setError(null);
    void (async () => {
      try {
        const loaded = await getDayLog(day);
        if (!cancelled) setLog(loaded);
      } catch (err) {
        if (!cancelled) {
          setError(isCommandError(err) ? `${err.code}: ${err.message}` : String(err));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [isOpen, day]);

  const onToggle = useCallback(
    async (id: string, name: string) => {
      const next = toggleDayMarker(log, id);
      // Optimistic: the checkbox must answer the keystroke, not the disk. A
      // failed write puts the old state back and says so.
      setLog(next);
      writesInFlight.current += 1;
      try {
        const saved = await setDayLog(next);
        setLog(saved);
      } catch (err) {
        setLog(log);
        setError(isCommandError(err) ? `${err.code}: ${err.message}` : String(err));
        announce(t('dialogs.dayLog.writeFailed', { name }));
      } finally {
        writesInFlight.current -= 1;
      }
    },
    [log, announce, t],
  );

  // A day ticked on another device while this dialog stands open. Adopting it
  // is not just cosmetic: every tick writes the log WHOLE, so a dialog holding
  // a stale copy would erase the other device's marker with the next one.
  useDayMarkersChanged(() => {
    if (!isOpen || writesInFlight.current > 0) return;
    void (async () => {
      try {
        setLog(await getDayLog(day));
      } catch {
        // Keep what is on screen — a failed re-read is not a reason to drop
        // ticks the user can see.
      }
    })();
  });

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.dayLog.title', { day: dayLabel })}
      className="modal--form modal--narrow"
      initialFocusRef={introRef}
      describedById={introId}
    >
      <p id={introId} ref={introRef} tabIndex={-1} className="form__hint">
        {t('dialogs.dayLog.intro')}
      </p>

      {error && (
        <p className="form__error" role="alert">
          {error}
        </p>
      )}

      {loadingMarkers ? (
        <p className="form__hint">{t('dialogs.dayLog.loading')}</p>
      ) : markers.length === 0 ? (
        <p className="form__hint">{t('dialogs.dayLog.noMarkers')}</p>
      ) : (
        <ul className="form__check-list">
          {markers.map((m) => {
            const ticked = log.markers.includes(m.id);
            return (
              <li key={m.id}>
                <label className="form__field form__field--check">
                  <input
                    type="checkbox"
                    checked={ticked}
                    onChange={() => void onToggle(m.id, m.name)}
                  />
                  {/* The symbol is decoration beside the name — a screen
                      reader announcing an emoji's own name in place of what
                      the user called this would be worse than silence. */}
                  {m.symbol && <span aria-hidden="true">{m.symbol} </span>}
                  <span>{m.name}</span>
                </label>
              </li>
            );
          })}
        </ul>
      )}

      <div className="modal__actions">
        <button type="button" className="form__action" onClick={onClose}>
          {t('dialogs.dayLog.close')}
        </button>
      </div>
    </Modal>
  );
}
