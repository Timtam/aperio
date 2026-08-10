import { useEffect, useId, useMemo, useRef } from 'react';
import { FocusableNote } from '../a11y/FocusableNote';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';

import { useCalendarStore } from '../state/calendarStoreContext';
import { useTaskCascadeEnabled } from '../state/taskCascadeContext';
import { useViewState } from '../state/viewStateContext';
import type {
  CarryOverDefault,
  CheckoffMode,
  DayStartTrigger,
  ListOverrides,
} from '../state/TaskCascadeProvider';
import { SettingsSelectorDetail } from './SettingsSelectorDetail';
import { SoundPrefField } from './SoundPrefField';

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

/**
 * Predefined day-start trigger choices. Exposed as a `<select>` so
 * the five-element list stays compact in the panel. Custom HH:MM
 * values that future migrations might introduce still round-trip
 * through the provider's string storage; only the UI is restricted
 * to these presets for now.
 */
const DAY_START_TRIGGER_OPTIONS: readonly DayStartTrigger[] = [
  '00:00',
  '06:00',
  '08:00',
  '12:00',
  'app-start',
];

/** Check-off gesture behaviour choices, rendered as a radio group. */
const CHECKOFF_MODE_OPTIONS: readonly CheckoffMode[] = ['toggle', 'cycle'];

/** Stable id accessor for the per-list selector (module scope so its identity holds). */
const taskListItemId = (l: { id: string }) => l.id;

/**
 * Sensible "X days before a deadline" presets for the countdown lead
 * time. The pref clamps to 1..30, so these stay inside that range; a
 * `<select>` keeps the short list compact and one focus stop for SR
 * users (vs. a free-typed number spinner).
 */
const DEADLINE_COUNTDOWN_DAYS_OPTIONS: readonly number[] = [1, 2, 3, 5, 7, 14];

export function TasksPanel() {
  const { t } = useTranslation();
  const {
    enabled,
    setEnabled,
    autoDate,
    setAutoDate,
    autoSelfAssign,
    setAutoSelfAssign,
    visualEffortSizing,
    setVisualEffortSizing,
    twoLevelPriority,
    setTwoLevelPriority,
    remindUntimedToday,
    setRemindUntimedToday,
    remindDeadlineArrived,
    setRemindDeadlineArrived,
    remindDeadlineCountdown,
    setRemindDeadlineCountdown,
    deadlineCountdownDays,
    setDeadlineCountdownDays,
    carryOverDefault,
    setCarryOverDefault,
    dayStartTrigger,
    setDayStartTrigger,
    checkoffMode,
    setCheckoffMode,
    listOverrides,
    setListOverride,
    prefRevision,
  } = useTaskCascadeEnabled();
  const announce = useAnnouncer();
  // A synced change landing while this panel is open moves a control the user
  // may have their hands on. Saying so is the least we owe them: the value
  // really did change everywhere, so showing the old one would be a lie, and
  // changing it silently is how a screen-reader user ends up mistrusting the
  // control. The ref skips the value the panel mounted with.
  const seenPrefRevision = useRef(prefRevision);
  useEffect(() => {
    if (seenPrefRevision.current === prefRevision) return;
    seenPrefRevision.current = prefRevision;
    announce(t('dialogs.tasks.changedElsewhere'));
  }, [prefRevision, announce, t]);
  const { taskLists, accounts } = useCalendarStore();
  const { showHiddenTaskListTargets, setShowHiddenTaskListTargets } =
    useViewState();

  // Group lists by their owning account so the per-list editor reads
  // as "iCloud > Privat | Arbeit" rather than a flat alphabetical
  // list — mirrors the Calendars-panel layout.
  const accountNameById = useMemo(() => {
    const map = new Map<string, string>();
    accounts.forEach((a) => map.set(a.id, a.display_name));
    return map;
  }, [accounts]);
  const listSelectorGroups = useMemo(() => {
    const byAccount = new Map<string, typeof taskLists>();
    taskLists.forEach((l) => {
      const bucket = byAccount.get(l.account_id) ?? [];
      bucket.push(l);
      byAccount.set(l.account_id, bucket);
    });
    return [...byAccount.entries()].map(([accountId, lists]) => ({
      id: accountId,
      label:
        accountNameById.get(accountId) ??
        (accountId === 'local'
          ? t('dialogs.settings.calendars.localAccount')
          : accountId),
      items: lists,
    }));
  }, [taskLists, accountNameById, t]);

  // Make sure the current (possibly synced-from-another-device) value
  // is always selectable even if it's not one of the presets — splice
  // it in and keep the list sorted so the <select> never shows blank.
  const countdownDayChoices = useMemo(() => {
    const set = new Set<number>(DEADLINE_COUNTDOWN_DAYS_OPTIONS);
    set.add(deadlineCountdownDays);
    return [...set].sort((a, b) => a - b);
  }, [deadlineCountdownDays]);

  const checkoffHeadingId = useId();
  const checkoffHintId = useId();
  const checkoffGroupId = useId();
  const couplingHeadingId = useId();
  const couplingHintId = useId();
  const autoDateHeadingId = useId();
  const autoDateHintId = useId();
  const autoSelfAssignHeadingId = useId();
  const autoSelfAssignHintId = useId();
  const visualEffortSizingHeadingId = useId();
  const visualEffortSizingHintId = useId();
  const twoLevelPriorityHeadingId = useId();
  const twoLevelPriorityHintId = useId();
  const showHiddenTargetsHeadingId = useId();
  const showHiddenTargetsHintId = useId();
  const carryOverHeadingId = useId();
  const carryOverHintId = useId();
  const carryOverGroupId = useId();
  const triggerHeadingId = useId();
  const triggerHintId = useId();
  const triggerSelectId = useId();
  const remindersHeadingId = useId();
  const remindUntimedHintId = useId();
  const remindDeadlineArrivedHintId = useId();
  const remindCountdownHintId = useId();
  const countdownDaysHintId = useId();
  const countdownDaysSelectId = useId();
  const perListHeadingId = useId();
  const perListHintId = useId();

  // Build the override-update payload for one knob on one list. The
  // tri-state "inherit | true | false" select serialises through:
  //   "" → field absent (inherit global)
  //   non-empty → field present with that value
  function updateListOverride<K extends keyof ListOverrides>(
    listId: string,
    key: K,
    value: ListOverrides[K] | undefined,
  ) {
    const existing = listOverrides[listId] ?? {};
    const next: ListOverrides = { ...existing };
    if (value === undefined) {
      delete next[key];
    } else {
      next[key] = value;
    }
    setListOverride(listId, next);
  }

  // Spoken summary of how many behaviour knobs a list overrides — folded into
  // the option's accessible name so the screen reader announces the state
  // without opening the detail pane. (Sound is a separate pref, not counted,
  // mirroring the calendar summary which counts only reminders.)
  const listSummary = (listId: string): string => {
    const n = Object.keys(listOverrides[listId] ?? {}).length;
    if (n === 0) return t('dialogs.tasks.perList.summaryNone');
    if (n === 1) return t('dialogs.tasks.perList.summaryOne');
    return t('dialogs.tasks.perList.summaryOther', { count: n });
  };

  return (
    <div className="form">
      <section
        aria-labelledby={checkoffHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={checkoffHeadingId} className="color-labels__heading">
          {t('dialogs.tasks.checkoffMode.heading')}
        </h3>
        <p id={checkoffHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.checkoffMode.hint')}
        </p>
        <div
          role="radiogroup"
          id={checkoffGroupId}
          aria-labelledby={checkoffHeadingId}
          aria-describedby={checkoffHintId}
          className="tasks-settings__radiogroup"
        >
          {CHECKOFF_MODE_OPTIONS.map((option) => (
            <label key={option} className="tasks-settings__radio">
              <input
                type="radio"
                name={checkoffGroupId}
                value={option}
                checked={checkoffMode === option}
                onChange={() => setCheckoffMode(option)}
              />
              <span>
                {t(`dialogs.tasks.checkoffMode.options.${option}`)}
              </span>
            </label>
          ))}
        </div>
      </section>

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
        aria-labelledby={autoSelfAssignHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={autoSelfAssignHeadingId} className="color-labels__heading">
          {t('dialogs.tasks.autoSelfAssign.heading')}
        </h3>
        <p id={autoSelfAssignHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.autoSelfAssign.hint')}
        </p>
        <label className="tasks-settings__toggle">
          <input
            type="checkbox"
            checked={autoSelfAssign}
            aria-describedby={autoSelfAssignHintId}
            onChange={(e) => setAutoSelfAssign(e.target.checked)}
          />
          <span>{t('dialogs.tasks.autoSelfAssign.label')}</span>
        </label>
      </section>

      <section
        aria-labelledby={visualEffortSizingHeadingId}
        className="tasks-settings__section"
      >
        <h3
          id={visualEffortSizingHeadingId}
          className="color-labels__heading"
        >
          {t('dialogs.tasks.visualEffortSizing.heading')}
        </h3>
        <p id={visualEffortSizingHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.visualEffortSizing.hint')}
        </p>
        <label className="tasks-settings__toggle">
          <input
            type="checkbox"
            checked={visualEffortSizing}
            aria-describedby={visualEffortSizingHintId}
            onChange={(e) => setVisualEffortSizing(e.target.checked)}
          />
          <span>{t('dialogs.tasks.visualEffortSizing.label')}</span>
        </label>
      </section>

      <section
        aria-labelledby={twoLevelPriorityHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={twoLevelPriorityHeadingId} className="color-labels__heading">
          {t('dialogs.tasks.twoLevelPriority.heading')}
        </h3>
        <p id={twoLevelPriorityHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.twoLevelPriority.hint')}
        </p>
        <label className="tasks-settings__toggle">
          <input
            type="checkbox"
            checked={twoLevelPriority}
            aria-describedby={twoLevelPriorityHintId}
            onChange={(e) => setTwoLevelPriority(e.target.checked)}
          />
          <span>{t('dialogs.tasks.twoLevelPriority.label')}</span>
        </label>
      </section>

      <section
        aria-labelledby={showHiddenTargetsHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={showHiddenTargetsHeadingId} className="color-labels__heading">
          {t('dialogs.tasks.showHiddenTaskListTargets.heading')}
        </h3>
        <p id={showHiddenTargetsHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.showHiddenTaskListTargets.hint')}
        </p>
        <label className="tasks-settings__toggle">
          <input
            type="checkbox"
            checked={showHiddenTaskListTargets}
            aria-describedby={showHiddenTargetsHintId}
            onChange={(e) => setShowHiddenTaskListTargets(e.target.checked)}
          />
          <span>{t('dialogs.tasks.showHiddenTaskListTargets.label')}</span>
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

      <section
        aria-labelledby={triggerHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={triggerHeadingId} className="color-labels__heading">
          {t('dialogs.tasks.dayStartTrigger.heading')}
        </h3>
        <p id={triggerHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.dayStartTrigger.hint')}
        </p>
        <label
          htmlFor={triggerSelectId}
          className="tasks-settings__select-label"
        >
          <span className="form__label">
            {t('dialogs.tasks.dayStartTrigger.label')}
          </span>
          <select
            id={triggerSelectId}
            value={dayStartTrigger}
            aria-describedby={triggerHintId}
            onChange={(e) =>
              setDayStartTrigger(e.target.value as DayStartTrigger)
            }
          >
            {DAY_START_TRIGGER_OPTIONS.map((option) => {
              // The translation keys mirror the option values. The
              // colon in HH:MM forms is awkward in dot-paths so we
              // route every preset through an explicit map of named
              // keys (`midnight`, `morning06`, …).
              const labelKey =
                option === 'app-start'
                  ? 'appStart'
                  : option === '00:00'
                  ? 'midnight'
                  : `morning${option.replace(':', '')}`;
              return (
                <option key={option} value={option}>
                  {t(`dialogs.tasks.dayStartTrigger.options.${labelKey}`)}
                </option>
              );
            })}
          </select>
        </label>
      </section>

      {/* Day-start reminders. Three independent on/off toggles + the
          shared "X days before" lead time. These only configure the
          prefs — the reminder LOGIC that consumes them lands in a later
          step. Grouped here with the other day-start behaviours. */}
      <section
        aria-labelledby={remindersHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={remindersHeadingId} className="color-labels__heading">
          {t('dialogs.tasks.reminders.heading')}
        </h3>
        <p className="tasks-settings__hint">
          {t('dialogs.tasks.reminders.hint')}
        </p>

        <p id={remindUntimedHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.reminders.untimedToday.hint')}
        </p>
        <label className="tasks-settings__toggle">
          <input
            type="checkbox"
            checked={remindUntimedToday}
            aria-describedby={remindUntimedHintId}
            onChange={(e) => setRemindUntimedToday(e.target.checked)}
          />
          <span>{t('dialogs.tasks.reminders.untimedToday.label')}</span>
        </label>

        <p id={remindDeadlineArrivedHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.reminders.deadlineArrived.hint')}
        </p>
        <label className="tasks-settings__toggle">
          <input
            type="checkbox"
            checked={remindDeadlineArrived}
            aria-describedby={remindDeadlineArrivedHintId}
            onChange={(e) => setRemindDeadlineArrived(e.target.checked)}
          />
          <span>{t('dialogs.tasks.reminders.deadlineArrived.label')}</span>
        </label>

        <p id={remindCountdownHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.reminders.deadlineCountdown.hint')}
        </p>
        <label className="tasks-settings__toggle">
          <input
            type="checkbox"
            checked={remindDeadlineCountdown}
            aria-describedby={remindCountdownHintId}
            onChange={(e) => setRemindDeadlineCountdown(e.target.checked)}
          />
          <span>{t('dialogs.tasks.reminders.deadlineCountdown.label')}</span>
        </label>

        {/* The "X days before" lead time. Shown regardless so the value
            can be set ahead of (or independently from) the countdown
            toggle. A short <select> is one focus stop for SR users. */}
        <p id={countdownDaysHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.reminders.countdownDays.hint')}
        </p>
        <label
          htmlFor={countdownDaysSelectId}
          className="tasks-settings__select-label"
        >
          <span className="form__label">
            {t('dialogs.tasks.reminders.countdownDays.label')}
          </span>
          <select
            id={countdownDaysSelectId}
            value={String(deadlineCountdownDays)}
            aria-describedby={countdownDaysHintId}
            onChange={(e) =>
              setDeadlineCountdownDays(Number.parseInt(e.target.value, 10))
            }
          >
            {countdownDayChoices.map((days) => (
              <option key={days} value={days}>
                {t('dialogs.tasks.reminders.countdownDays.option', {
                  count: days,
                })}
              </option>
            ))}
          </select>
        </label>
      </section>

      {/* Per-list overrides (#124). The three knobs above are the
          global defaults; this section lets the user override any
          subset of them for a specific task list. Most users won't
          touch it — the "Globaler Standard" option is the default
          per field, identical to the historical behaviour. */}
      <section
        aria-labelledby={perListHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={perListHeadingId} className="color-labels__heading">
          {t('dialogs.tasks.perList.heading')}
        </h3>
        <p id={perListHintId} className="tasks-settings__hint">
          {t('dialogs.tasks.perList.hint')}
        </p>
        {taskLists.length === 0 ? (
          <FocusableNote className="form__hint">
            {t('dialogs.tasks.perList.empty')}
          </FocusableNote>
        ) : (
          // Master/detail (shared with the Calendars panel): one listbox of
          // task lists (a single tab stop) + a detail pane that edits ONLY the
          // selected list, instead of stacking every list's controls inline.
          <SettingsSelectorDetail
            groups={listSelectorGroups}
            getItemId={taskListItemId}
            getItemName={(l) => l.name}
            getItemSummary={(l) => listSummary(l.id)}
            getItemBadge={(l) =>
              Object.keys(listOverrides[l.id] ?? {}).length || ''
            }
            selectorLabel={t('dialogs.tasks.perList.selectorLabel')}
            optionLabel={({ account, name, summary }) =>
              t('dialogs.tasks.perList.optionLabel', {
                account,
                list: name,
                summary,
              })
            }
            detailHeading={({ account, name }) =>
              t('dialogs.tasks.perList.detailHeading', { account, list: name })
            }
            renderDetail={(list) => {
              // Tri-state selects: empty value = "inherit global". No
              // per-control `aria-describedby` — the detail region heading
              // ("{account} › {list}") already names the editing context, so
              // every control announces which list it belongs to.
              const override = listOverrides[list.id] ?? {};
              return (
                <div className="tasks-settings__list-controls">
                  <label className="tasks-settings__select-label">
                    <span className="form__label">
                      {t('dialogs.tasks.perList.cascade')}
                    </span>
                    <select
                      value={
                        override.cascade === undefined
                          ? ''
                          : override.cascade
                          ? 'true'
                          : 'false'
                      }
                      onChange={(e) => {
                        const v = e.target.value;
                        updateListOverride(
                          list.id,
                          'cascade',
                          v === '' ? undefined : v === 'true',
                        );
                      }}
                    >
                      <option value="">
                        {t('dialogs.tasks.perList.inherit')}
                      </option>
                      <option value="true">
                        {t('dialogs.tasks.perList.on')}
                      </option>
                      <option value="false">
                        {t('dialogs.tasks.perList.off')}
                      </option>
                    </select>
                  </label>
                  <label className="tasks-settings__select-label">
                    <span className="form__label">
                      {t('dialogs.tasks.perList.autoDate')}
                    </span>
                    <select
                      value={
                        override.autoDate === undefined
                          ? ''
                          : override.autoDate
                          ? 'true'
                          : 'false'
                      }
                      onChange={(e) => {
                        const v = e.target.value;
                        updateListOverride(
                          list.id,
                          'autoDate',
                          v === '' ? undefined : v === 'true',
                        );
                      }}
                    >
                      <option value="">
                        {t('dialogs.tasks.perList.inherit')}
                      </option>
                      <option value="true">
                        {t('dialogs.tasks.perList.on')}
                      </option>
                      <option value="false">
                        {t('dialogs.tasks.perList.off')}
                      </option>
                    </select>
                  </label>
                  <label className="tasks-settings__select-label">
                    <span className="form__label">
                      {t('dialogs.tasks.perList.carryOver')}
                    </span>
                    <select
                      value={override.carryOverDefault ?? ''}
                      onChange={(e) => {
                        const v = e.target.value;
                        updateListOverride(
                          list.id,
                          'carryOverDefault',
                          v === '' ? undefined : (v as CarryOverDefault),
                        );
                      }}
                    >
                      <option value="">
                        {t('dialogs.tasks.perList.inherit')}
                      </option>
                      {CARRY_OVER_OPTIONS.map((option) => (
                        <option key={option} value={option}>
                          {t(
                            `dialogs.tasks.carryOverDefault.options.${option}`,
                          )}
                        </option>
                      ))}
                    </select>
                  </label>
                  {/* §14.4 per-list default notification sound — inherits the
                      global default unless set. The selector re-keys this
                      detail per selection, so it reflects the current list. */}
                  <SoundPrefField prefKey={`sound.tasklist.${list.id}`} />
                </div>
              );
            }}
          />
        )}
      </section>
    </div>
  );
}
