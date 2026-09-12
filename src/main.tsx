import React from 'react';
import ReactDOM from 'react-dom/client';

import {
  installConferenceDetector,
  installGroupSuggestionRules,
  installMeetingDuplicateFilter,
  installMeetingLinkRules,
  installEventGroupFold,
  installGroupCarryRules,
  installTaskGroupingRules,
  installTaskDayRules,
  installTaskPriorityRules,
  installTextCollation,
} from '@aperio/shared';

import { App } from './App';
import { installConsoleBridge } from './dev/consoleBridge';
import { applyThemeMode, readThemeMode } from './state/themeMode';
import { applyUiScale, readUiScale } from './state/uiScale';
import {
  compareNames,
  compareTitles,
  detectConferenceJson,
  findGroupSuggestionsJson,
  suggestGroupMateJson,
  withoutDuplicateMeetingsJson,
  findMeetingLinkPairsJson,
  normalizeJoinUrlThroughCore,
  collapseEventGroupsJson,
  planCarryJson,
  occurrenceCarryFieldsJson,
  futureCarryFieldsJson,
  carryOntoFieldsJson,
  groupTasksJson,
  tasksOnDaysJson,
  backlogWeeksJson,
  splitDeadlinesByWeekJson,
  initCoreRules,
  isImportantPriority,
  normalPriority,
  priorityRank,
} from './wasm/coreRules';
import i18n from './i18n';
import './styles.css';

// Mirror webview console output into the Rust log stream in EVERY build.
// In dev it surfaces in the terminal; in release it flows into the
// persistent log file so a user's exported log (Settings → Protokolle)
// captures frontend errors too.
installConsoleBridge();

// Apply the device-local UI scale (root font-size) BEFORE the first paint so
// the interface never renders at 100% and then jumps. Every rem token follows.
applyUiScale(readUiScale());

// Same for the device-local theme mode: resolve light/dark/system onto
// <html data-theme> before anything renders, so the palette never flashes.
applyThemeMode(readThemeMode());

// The core rules, compiled into this webview as WebAssembly, are the desktop's
// only SYNCHRONOUS road into `cal-core` — Tauri's `invoke` is IPC and a React
// render cannot await. Compiling the module is the one asynchronous step, and
// it happens here, once, before the first render: afterwards every call is an
// ordinary function call, including the ones inside `Array.sort` comparators.
//
// A failure is fatal and SAID, not swallowed. Rules that silently fall back to
// a second implementation are the thing this exists to remove, and a blank
// window would tell a screen-reader user nothing at all.
initCoreRules()
  .then(() => {
    // The core's ordering rule, bound to the language the USER chose — not the
    // one the operating system reports, which is what `localeCompare` followed
    // before. Re-bound on every language change, so a switch reorders the
    // lists it should.
    const installCollation = () =>
      installTextCollation({
        compareNames: (a, b) => compareNames(a, b, i18n.language),
        compareTitles: (a, b) => compareTitles(a, b, i18n.language),
      });
    installCollation();
    i18n.on('languageChanged', installCollation);

    // The priority ranking, from `cal_core::task_priority`. No language in it,
    // so it is installed once and never re-bound — unlike the collation above.
    installTaskPriorityRules({
      priorityRank,
      isImportantPriority,
      normalPriority,
    });

    // Finding the online meeting in an event. No language in it either, so it
    // is installed once — see `shared/conferencing.ts` for what it replaces.
    installConferenceDetector({ detectConferenceJson });

    // Recognising a copy. Also installed once, and also synchronous — both
    // callers ask inside a `useMemo`, where nothing can await.
    installGroupSuggestionRules({ findGroupSuggestionsJson, suggestGroupMateJson });

    // Hiding a provider-side meeting that already has a calendar entry.
    installMeetingDuplicateFilter({ withoutDuplicateMeetingsJson });

    // Pairing a meeting with the appointment it belongs to, and the URL fold
    // the identity rests on.
    installMeetingLinkRules({
      findMeetingLinkPairsJson,
      normalizeJoinUrl: normalizeJoinUrlThroughCore,
    });

    // Folding a group into one row — what a day actually looks like.
    installEventGroupFold({ collapseEventGroupsJson });

    // Carrying a change to the other copies.
    installGroupCarryRules({
      planCarryJson,
      occurrenceCarryFieldsJson,
      futureCarryFieldsJson,
      carryOntoFieldsJson,
    });

    // Grouping the task view.
    installTaskGroupingRules({ groupTasksJson, languageTag: () => i18n.language });

    // The calendar-day task rules.
    installTaskDayRules({
      tasksOnDaysJson,
      backlogWeeksJson,
      splitDeadlinesByWeekJson,
      languageTag: () => i18n.language,
    });

    ReactDOM.createRoot(document.getElementById('root')!).render(
      <React.StrictMode>
        <App />
      </React.StrictMode>,
    );
  })
  .catch((err: unknown) => {
    // eslint-disable-next-line no-console
    console.error('the core rules failed to load', err);
    const root = document.getElementById('root');
    if (root) {
      root.setAttribute('role', 'alert');
      root.textContent = `Aperio konnte seine Kernregeln nicht laden: ${
        err instanceof Error ? err.message : String(err)
      }`;
    }
  });
