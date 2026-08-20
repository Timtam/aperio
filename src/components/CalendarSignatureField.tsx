import { useTranslation } from 'react-i18next';

import { useSignatures } from '../state/useSignatures';

/**
 * Which signature a calendar carries — edited where the calendar is.
 *
 * It sat in the Signatures panel as one row per calendar, which was the wrong
 * place twice: a matrix of every calendar is a poor way to answer a question
 * about ONE, and the setting only appeared once a signature existed, so it read
 * as missing. Per-calendar settings live in the calendar's own detail, beside
 * its default reminders — that is where somebody looking for them goes.
 *
 * The binding is not a preference about a button. A calendar with a signature
 * puts it on new appointments by itself; the button is for the exceptions.
 */
export function CalendarSignatureField({ calendarId }: { calendarId: string }) {
  const { t } = useTranslation();
  const { signatures, forCalendar, bind } = useSignatures([calendarId]);

  // Nothing to bind to yet. Silence beats a picker whose only entry is "none"
  // and a hint about a feature the user has not set up.
  if (signatures.length === 0) return null;

  return (
    <label className="form__field">
      <span className="form__label">{t('dialogs.signature.calendarLabel')}</span>
      <select
        value={forCalendar(calendarId)?.id ?? ''}
        onChange={(e) => void bind(calendarId, e.target.value || null)}
      >
        <option value="">{t('dialogs.settings.signatures.bindingNone')}</option>
        {signatures.map((s) => (
          <option key={s.id} value={s.id}>
            {s.name}
          </option>
        ))}
      </select>
      <span className="form__hint">{t('dialogs.signature.calendarHint')}</span>
    </label>
  );
}
