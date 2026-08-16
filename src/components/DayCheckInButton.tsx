import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useDateFormat } from '../intl/dateFormat';
import { DayLogDialog } from './DayLogDialog';

/**
 * "Check-in" for one day, and the dialog it opens.
 *
 * One button per VIEW rather than one per day. The per-day buttons were taken
 * out of the week and month grids on purpose — seven or thirty-one extra tab
 * stops to reach one you wanted — so this acts on the day the view is standing
 * on, which for a grid is the focused cell and for the day view is the day
 * itself.
 *
 * That makes the label ambiguous on its own ("which day?"), so the accessible
 * name always carries the date while the visible text stays short. The visible
 * text is for somebody who can see which cell is focused; the name is for
 * somebody who cannot.
 *
 * The year view has no day to hand — it focuses a MONTH — so it does not get
 * one, and the agenda view aims at the day its range starts on.
 */
export function DayCheckInButton({
  day,
  className = 'form__action',
}: {
  /** Local day key, `YYYY-MM-DD`. */
  day: string;
  className?: string;
}) {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const [open, setOpen] = useState(false);

  const label = (() => {
    try {
      return fmt.format(new Date(`${day}T00:00:00`), 'PPPP');
    } catch {
      return day;
    }
  })();

  return (
    <>
      <button
        type="button"
        className={className}
        aria-label={t('dialogs.dayLog.openButtonOnDay', { day: label })}
        onClick={() => setOpen(true)}
      >
        {t('dialogs.dayLog.openButton')}
      </button>
      <DayLogDialog
        isOpen={open}
        onClose={() => setOpen(false)}
        day={day}
        dayLabel={label}
      />
    </>
  );
}
