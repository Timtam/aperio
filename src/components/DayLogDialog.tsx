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
  const { markers, loading: loadingMarkers, error: markersFailed } = useDayMarkers();
  const [log, setLog] = useState<DayLog>(() => emptyDayLog(day));
  const [error, setError] = useState<string | null>(null);
  // Writes this dialog started and has not seen land yet. A tick's own write
  // raises the same signal a foreign one does, and re-reading in the middle of
  // our own burst would paint the state we just left behind.
  const writesInFlight = useRef(0);
  const writeSeq = useRef(0);
  // The DAY's own read, tracked apart from the vocabulary's. A tick before it
  // lands would write a log built from nothing, erasing whatever the day
  // already held — so the checkboxes wait for it, exactly as on the phone.
  const [loadingLog, setLoadingLog] = useState(true);
  const [logFailed, setLogFailed] = useState(false);
  // Which checkbox carries the tab stop. Real focus moves with it, so the
  // announcement is the checkbox's own rather than a listbox's "selected".
  const [activeIndex, setActiveIndex] = useState(0);
  const boxes = useRef<(HTMLInputElement | null)[]>([]);
  const introId = useId();
  const introRef = useRef<HTMLParagraphElement | null>(null);

  // Load the day each time the dialog opens, not once per mount: the same
  // dialog serves whichever day the user is standing on.
  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    setLog(emptyDayLog(day));
    setError(null);
    setLoadingLog(true);
    setLogFailed(false);
    void (async () => {
      try {
        const loaded = await getDayLog(day);
        if (!cancelled) setLog(loaded);
      } catch (err) {
        if (!cancelled) {
          setError(isCommandError(err) ? `${err.code}: ${err.message}` : String(err));
          setLogFailed(true);
        }
      } finally {
        if (!cancelled) setLoadingLog(false);
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
      // Two quick ticks are two independent commands racing for the DB lock.
      // Without this, the FIRST one's response — which knows nothing about the
      // second tick — could land last and visibly un-tick what the user just
      // ticked, while the disk held the right thing.
      const mine = (writeSeq.current += 1);
      try {
        const saved = await setDayLog(next);
        if (writeSeq.current === mine) setLog(saved);
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

  // Clamp when the vocabulary shrinks under an open dialog — a sync round can
  // delete a marker another device removed.
  useEffect(() => {
    if (activeIndex >= markers.length && markers.length > 0) {
      setActiveIndex(markers.length - 1);
    }
  }, [markers.length, activeIndex]);

  const onListKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (markers.length === 0) return;
      let next: number | null = null;
      if (e.key === 'ArrowDown') next = Math.min(activeIndex + 1, markers.length - 1);
      else if (e.key === 'ArrowUp') next = Math.max(activeIndex - 1, 0);
      else if (e.key === 'Home') next = 0;
      else if (e.key === 'End') next = markers.length - 1;
      if (next === null) return;
      e.preventDefault();
      setActiveIndex(next);
      // Move the REAL focus, not just the tab stop: the whole point is that
      // the next arrow press announces the marker it landed on.
      boxes.current[next]?.focus();
    },
    [activeIndex, markers.length],
  );

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

      {loadingMarkers || loadingLog ? (
        <p className="form__hint">{t('dialogs.dayLog.loading')}</p>
      ) : logFailed || markersFailed ? (
        // Say nothing about the vocabulary here. "You have no markers yet" is
        // an invitation to create one, and showing it to somebody who has ten
        // and hit a read error would be a lie — the error above is the truth.
        <p className="form__hint">{t('dialogs.dayLog.readFailed')}</p>
      ) : markers.length === 0 ? (
        <p className="form__hint">{t('dialogs.dayLog.noMarkers')}</p>
      ) : (
        <ul className="form__check-list" onKeyDown={onListKeyDown}>
          {markers.map((m, i) => {
            const ticked = log.markers.includes(m.id);
            return (
              <li key={m.id}>
                {/* `--inline` rather than the column default: a checkbox, an
                    emoji and a word on three lines each turned a short list
                    into a scrolling one. */}
                <label className="form__field form__field--inline">
                  <input
                    ref={(el) => {
                      boxes.current[i] = el;
                    }}
                    type="checkbox"
                    checked={ticked}
                    // Roving tabindex: the list is ONE tab stop, and the
                    // arrow keys move within it. A stop per marker meant ten
                    // markers cost ten presses to walk past — for a dialog
                    // whose entire promise is that recording a day is cheap.
                    // Real checkboxes rather than a listbox of options, so
                    // the announcement stays "checkbox, checked" and Space
                    // keeps toggling natively.
                    tabIndex={i === activeIndex ? 0 : -1}
                    onFocus={() => setActiveIndex(i)}
                    onChange={() => void onToggle(m.id, m.name)}
                  />
                  {/* The symbol is decoration beside the name — a screen
                      reader announcing an emoji's own name in place of what
                      the user called this would be worse than silence. */}
                  {m.symbol && <span aria-hidden="true">{m.symbol}</span>}
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
