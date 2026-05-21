import { useId } from 'react';
import { useTranslation } from 'react-i18next';

import {
  useTaskCascadeEnabled,
  type CarryOverDefault,
} from '../state/TaskCascadeProvider';

/**
 * Tasks settings panel.
 *
 * Hosts the three task-behaviour switches. The cascade implemented in
 * `taskCascade.ts` is on by default — users who prefer fully
 * independent tasks can opt out and the planners degrade to single-
 * row writes / no-ops. The auto-date logic (#104) is gated by its own
 * toggle. The carry-over default (#107) controls whether the
 * day-start dialog opens at all or runs as a silent batch action.
 *
 * The panel is rendered inside the Settings dialog's `role="tabpanel"`
 * (see `SettingsDialog`). Each row is a self-contained section with
 * heading, hint, and one control. `aria-describedby` hangs the long
 * explanations off the controls themselves so NVDA reads the
 * focused control name first and the explanation second — same a11y
 * fix we landed in #102 for the cascade-coupling row.
 */
const CARRY_OVER_OPTIONS: readonly CarryOverDefault[] = [
  'ask',
  'today',
  'backlog',
];

export function TasksPanel() {
  const { t } = useTranslation();
  const {
    enabled,
    setEnabled,
    autoDate,
    setAutoDate,
    carryOverDefault,
    setCarryOverDefault,
  } = useTaskCascadeEnabled();

  const couplingHeadingId = useId();
  const couplingHintId = useId();
  const autoDateHeadingId = useId();
  const autoDateHintId = useId();
  const carryOverHeadingId = useId();
  const carryOverHintId = useId();
  const carryOverGroupId = useId();

  return (
    <div className="form">
      <section
        aria-labelledby={couplingHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={couplingHeadingId} className="color-labels__heading">
          {t('dialogs.tasks.statusCoupling.heading')}
        </h3>
        <p id={couplingHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.statusCoupling.hint')}
        </p>
        <label className="tasks-settings__toggle">
          <input
            type="checkbox"
            checked={enabled}
            aria-describedby={couplingHintId}
            onChange={(e) => setEnabled(e.target.checked)}
          />
          <span>{t('dialogs.tasks.statusCoupling.label')}</span>
        </label>
      </section>

      <section
        aria-labelledby={autoDateHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={autoDateHeadingId} className="color-labels__heading">
          {t('dialogs.tasks.autoDate.heading')}
        </h3>
        <p id={autoDateHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.autoDate.hint')}
        </p>
        <label className="tasks-settings__toggle">
          <input
            type="checkbox"
            checked={autoDate}
            aria-describedby={autoDateHintId}
            onChange={(e) => setAutoDate(e.target.checked)}
          />
          <span>{t('dialogs.tasks.autoDate.label')}</span>
        </label>
      </section>

      <section
        aria-labelledby={carryOverHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={carryOverHeadingId} className="color-labels__heading">
          {t('dialogs.tasks.carryOverDefault.heading')}
        </h3>
        <p id={carryOverHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.carryOverDefault.hint')}
        </p>
        {/* Radios over a <select> because the three options have
            meaningfully different consequences and benefit from
            being visible side-by-side; arrow-key navigation between
            them is also the gold-standard tablist-adjacent pattern
            for keyboard / SR users. `aria-describedby` on the
            radiogroup attaches the hint to every focus stop. */}
        <div
          role="radiogroup"
          id={carryOverGroupId}
          aria-labelledby={carryOverHeadingId}
          aria-describedby={carryOverHintId}
          className="tasks-settings__radiogroup"
        >
          {CARRY_OVER_OPTIONS.map((option) => (
            <label key={option} className="tasks-settings__radio">
              <input
                type="radio"
                name={carryOverGroupId}
                value={option}
                checked={carryOverDefault === option}
                onChange={() => setCarryOverDefault(option)}
              />
              <span>
                {t(`dialogs.tasks.carryOverDefault.options.${option}`)}
              </span>
            </label>
          ))}
        </div>
      </section>
    </div>
  );
}
