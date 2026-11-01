import {
  calendarWeekLabel,
  calendarWeekName,
  calendarWeekOf,
  type BacklogWeeks,
} from '@aperio/shared';

type Translate = (key: string, vars?: Record<string, unknown>) => string;

/** A backlog section's heading as it is seen, and its name as it is heard. */
export interface BacklogSectionNames {
  heading: string;
  spoken: string;
}

/**
 * The headings of the backlog's three sections (TODO B15 step 5). Seen:
 * "Diese Woche (3) · KW 45". Heard: "Diese Woche, Kalenderwoche 45, 3
 * Aufgaben" — the week in words, and the count with its noun, not a number
 * in brackets. "Weiteres" spans no one week and names none. With weeks
 * switched off (`view.weekNumbers`), seen and heard leave the week out.
 */
export function backlogSectionNames(
  t: Translate,
  weeks: BacklogWeeks,
  weekStartsOn: number,
  weekNumbers: boolean,
  counts: { thisWeek: number; nextWeek: number; rest: number },
): { thisWeek: BacklogSectionNames; nextWeek: BacklogSectionNames; rest: BacklogSectionNames } {
  const section = (
    name: string,
    plainKey: string,
    weekStart: string | null,
    count: number,
  ): BacklogSectionNames => {
    const week = weekStart != null && weekNumbers ? calendarWeekOf(weekStart, weekStartsOn) : null;
    if (week == null) {
      return {
        heading: t(plainKey, { count }),
        spoken: t('views.backlog.sectionSpoken', { name, count }),
      };
    }
    return {
      heading: t('views.backlog.headingWithWeek', {
        name,
        week: calendarWeekLabel(t, week),
        count,
      }),
      spoken: t('views.backlog.sectionSpokenWeek', {
        name,
        week: calendarWeekName(t, week),
        count,
      }),
    };
  };
  return {
    thisWeek: section(
      t('views.backlog.thisWeekName'),
      'views.backlog.thisWeekHeading',
      weeks.thisWeekStart,
      counts.thisWeek,
    ),
    nextWeek: section(
      t('views.backlog.nextWeekName'),
      'views.backlog.nextWeekHeading',
      weeks.nextWeekStart,
      counts.nextWeek,
    ),
    rest: section(t('views.backlog.restName'), 'views.backlog.restHeading', null, counts.rest),
  };
}
