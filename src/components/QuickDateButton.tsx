import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { quickDates } from '@aperio/shared';

import { useDateFormat } from '../intl/dateFormat';
import { useViewState } from '../state/viewStateContext';
import { Modal } from './Modal';

/**
 * The four dates people actually pick, one press away.
 *
 * Typing "tomorrow" into a date field means first working out what tomorrow
 * IS, then walking a spinner to it — which is a lot of work for the most
 * ordinary answer there is, and more still when every step is spoken.
 *
 * Deliberately a fixed, tiny set with no way to edit it. Four learnable
 * buttons beat a configurable list nobody would maintain, and the date field
 * beside them still takes anything at all.
 *
 * Every button says the DATE it will set as well as its name — "Tomorrow,
 * Thursday, 20 August" — so nobody has to accept an offer to find out what it
 * was.
 */
export function QuickDateButton({
  onPick,
  className = 'form__action',
}: {
  /** Receives a local `YYYY-MM-DD`. */
  onPick: (dayKey: string) => void;
  className?: string;
}) {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { weekStartsOn } = useViewState();
  const [open, setOpen] = useState(false);

  // Computed on OPEN, not on mount: a dialog left open across midnight would
  // otherwise still be offering yesterday's idea of "today".
  const offers = open ? quickDates(new Date(), weekStartsOn) : [];

  return (
    <>
      <button
        type="button"
        className={className}
        onClick={() => setOpen(true)}
      >
        {t('dialogs.quickDate.open')}
      </button>
      <Modal
        isOpen={open}
        onClose={() => setOpen(false)}
        title={t('dialogs.quickDate.title')}
        className="modal--form modal--narrow"
      >
        <div className="quick-date__choices" role="group">
          {offers.map((offer) => (
            <button
              key={offer.id}
              type="button"
              className="form__action"
              onClick={() => {
                onPick(offer.dayKey);
                setOpen(false);
              }}
            >
              {t(`dialogs.quickDate.${offer.id}`, {
                date: fmt.format(new Date(`${offer.dayKey}T00:00:00`), 'PPPP'),
              })}
            </button>
          ))}
        </div>
        <div className="modal__actions">
          <button
            type="button"
            className="form__action"
            onClick={() => setOpen(false)}
          >
            {t('dialogs.cancel')}
          </button>
        </div>
      </Modal>
    </>
  );
}
