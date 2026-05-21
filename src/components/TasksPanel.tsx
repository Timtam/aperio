import { useId } from 'react';
import { useTranslation } from 'react-i18next';

import { useTaskCascadeEnabled } from '../state/TaskCascadeProvider';

/**
 * Tasks settings panel.
 *
 * Currently hosts one switch: parent / subtask status coupling. The
 * cascade implemented in `taskCascade.ts` is on by default — users
 * who prefer fully independent tasks can opt out here, and the
 * planners then degrade to single-row writes / no-ops.
 *
 * The panel is rendered inside the Settings dialog's `role="tabpanel"`
 * (see `SettingsDialog`). It has no listbox or other landmark of its
 * own, so there is no NVDA "phantom focus" pitfall to dance around
 * — a plain form + heading does the job.
 */
export function TasksPanel() {
  const { t } = useTranslation();
  const { enabled, setEnabled } = useTaskCascadeEnabled();

  const headingId = useId();
  const hintId = useId();

  return (
    <div className="form">
      <section
        aria-labelledby={headingId}
        className="tasks-settings__section"
      >
        <h3 id={headingId} className="color-labels__heading">
          {t('dialogs.tasks.statusCoupling.heading')}
        </h3>
        {/* `aria-describedby` sits on the checkbox itself, not on the
            section. With it on the section, NVDA reads the long hint
            the moment focus enters the region — *before* the checkbox
            label gets its turn. Attaching the hint to the input makes
            it the input's description, so the read order becomes
            name → role → state → description (the natural one).
            The visual order keeps the hint above the checkbox so
            sighted users see the explanation in context. */}
        <p id={hintId} className="tasks-settings__hint">
          {t('dialogs.tasks.statusCoupling.hint')}
        </p>
        <label className="tasks-settings__toggle">
          <input
            type="checkbox"
            checked={enabled}
            aria-describedby={hintId}
            onChange={(e) => setEnabled(e.target.checked)}
          />
          <span>{t('dialogs.tasks.statusCoupling.label')}</span>
        </label>
      </section>
    </div>
  );
}
