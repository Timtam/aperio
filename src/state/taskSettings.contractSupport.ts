// Test support for the task-settings contract: how each surface reads the
// sixteen stored task and calendar preferences, resolves a list's effective
// settings, and normalises what it writes back — measured on the REAL code of
// both surfaces. The desktop is `TaskCascadeProvider`, rendered with its
// preference reads answered from a table; mobile is `taskBehaviour.ts`, with
// its preference module answered the same way. The test files that import
// this mock `../api/client`, `../../mobile/src/api/prefs` and
// `@tauri-apps/api/event`. Nothing in the app imports this.
import { act, render } from '@testing-library/react';
import { createElement } from 'react';
import { vi } from 'vitest';

import {
  getUserPref as mobileGetUserPref,
  setUserPref as mobileSetUserPref,
} from '../../mobile/src/api/prefs';
import {
  effectiveForList as mobileEffectiveForList,
  readTaskBehaviour,
  withListOverride,
  writeDayWindow,
  writeDeadlineCountdownDays,
  type ListOverrides,
  type TaskBehaviour,
} from '../../mobile/src/state/taskBehaviour';
import { getUserPref as desktopGetUserPref } from '../api/client';
import { TaskCascadeProvider, type TaskCascadeContextValue } from './TaskCascadeProvider';
import { useTaskCascadeEnabled } from './taskCascadeContext';

/** The stored keys both surfaces read, by the names the cases use. */
export const KEYS = {
  cascade: 'tasks.cascadeStatusCoupling',
  autoDate: 'tasks.autoDateOnStart',
  autoSelfAssign: 'tasks.autoSelfAssign',
  visualEffortSizing: 'tasks.visualEffortSizing',
  twoLevelPriority: 'tasks.twoLevelPriority',
  remindUntimedToday: 'tasks.remindUntimedToday',
  remindDeadlineArrived: 'tasks.remindDeadlineArrived',
  remindDeadlineCountdown: 'tasks.remindDeadlineCountdown',
  deadlineCountdownDays: 'tasks.deadlineCountdownDays',
  dayViewMode: 'calendar.dayViewMode',
  dayStartMin: 'calendar.dayStartMin',
  dayEndMin: 'calendar.dayEndMin',
  checkoffMode: 'tasks.checkoffMode',
  carryOverDefault: 'tasks.carryOverDefault',
  dayStartTrigger: 'tasks.dayStartTrigger',
  listOverrides: 'tasks.listOverrides',
} as const;

/** Stored values by key; a key that is absent is not stored. */
export type RawPrefs = Record<string, string | null>;

/** What a surface holds after reading, by one set of names. */
export interface Settings {
  cascadeEnabled: boolean;
  autoDate: boolean;
  autoSelfAssign: boolean;
  visualEffortSizing: boolean;
  twoLevelPriority: boolean;
  remindUntimedToday: boolean;
  remindDeadlineArrived: boolean;
  remindDeadlineCountdown: boolean;
  deadlineCountdownDays: number;
  dayViewMode: string;
  dayStartMin: number;
  dayEndMin: number;
  checkoffMode: string;
  carryOverDefault: string;
  dayStartTrigger: string;
  listOverrides: Record<string, ListOverrides>;
}

export interface BySurface<T> {
  mobile: T;
  desktop: T;
}

/** A number the fixture could not spell in JSON. */
export type Num = number | 'NaN' | 'Infinity' | '-Infinity';
const num = (v: Num): number => (typeof v === 'number' ? v : Number(v));

/** Only own properties survive, as they would in storage. */
function plain<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function answering(prefs: RawPrefs, failing?: string) {
  return (key: string): Promise<string | null> => {
    if (key === failing) return Promise.reject(new Error(`reading ${key} failed`));
    return Promise.resolve(Object.hasOwn(prefs, key) ? prefs[key] : null);
  };
}

// ── Mobile ─────────────────────────────────────────────────────────────────

async function mobileBehaviour(prefs: RawPrefs, failing?: string): Promise<TaskBehaviour> {
  vi.mocked(mobileGetUserPref).mockImplementation(answering(prefs, failing));
  return readTaskBehaviour();
}

function fromMobile(b: TaskBehaviour): Settings {
  return {
    cascadeEnabled: b.cascadeEnabled,
    autoDate: b.autoDate,
    autoSelfAssign: b.autoSelfAssign,
    visualEffortSizing: b.visualEffortSizing,
    twoLevelPriority: b.twoLevelPriority,
    remindUntimedToday: b.remindUntimedToday,
    remindDeadlineArrived: b.remindDeadlineArrived,
    remindDeadlineCountdown: b.remindDeadlineCountdown,
    deadlineCountdownDays: b.deadlineCountdownDays,
    dayViewMode: b.dayViewMode,
    dayStartMin: b.dayStartMin,
    dayEndMin: b.dayEndMin,
    checkoffMode: b.checkoffMode,
    carryOverDefault: b.carryOverDefault,
    dayStartTrigger: b.dayStartTrigger,
    listOverrides: plain(b.listOverrides),
  };
}

/** The last value mobile wrote for `key`. */
function mobileWrote(key: string): string | undefined {
  const calls = vi.mocked(mobileSetUserPref).mock.calls.filter(([k]) => k === key);
  return calls.at(-1)?.[1];
}

// ── Desktop ────────────────────────────────────────────────────────────────

function Probe({ onValue }: { onValue: (value: TaskCascadeContextValue) => void }) {
  onValue(useTaskCascadeEnabled());
  return null;
}

async function mountDesktop(prefs: RawPrefs, failing?: string) {
  vi.mocked(desktopGetUserPref).mockImplementation(answering(prefs, failing));
  const holder: { value: TaskCascadeContextValue | null } = { value: null };
  let view: ReturnType<typeof render> | null = null;
  await act(async () => {
    view = render(
      createElement(TaskCascadeProvider, {
        children: createElement(Probe, {
          onValue: (value: TaskCascadeContextValue) => {
            holder.value = value;
          },
        }),
      }),
    );
  });
  for (let i = 0; i < 20 && (holder.value === null || holder.value.hydrating); i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
  const current = (): TaskCascadeContextValue => {
    if (holder.value === null || holder.value.hydrating) {
      throw new Error('the desktop provider never finished reading');
    }
    return holder.value;
  };
  current();
  return {
    current,
    unmount: () => view?.unmount(),
  };
}

function fromDesktop(v: TaskCascadeContextValue): Settings {
  return {
    cascadeEnabled: v.enabled,
    autoDate: v.autoDate,
    autoSelfAssign: v.autoSelfAssign,
    visualEffortSizing: v.visualEffortSizing,
    twoLevelPriority: v.twoLevelPriority,
    remindUntimedToday: v.remindUntimedToday,
    remindDeadlineArrived: v.remindDeadlineArrived,
    remindDeadlineCountdown: v.remindDeadlineCountdown,
    deadlineCountdownDays: v.deadlineCountdownDays,
    dayViewMode: v.dayViewMode,
    dayStartMin: v.dayStartMin,
    dayEndMin: v.dayEndMin,
    checkoffMode: v.checkoffMode,
    carryOverDefault: v.carryOverDefault,
    dayStartTrigger: v.dayStartTrigger,
    listOverrides: plain(v.listOverrides),
  };
}

// ── The answers ────────────────────────────────────────────────────────────

export interface ReadInput {
  prefs: RawPrefs;
  /** A key whose read fails. */
  failing?: string;
}

export async function answerRead(i: ReadInput): Promise<BySurface<Settings>> {
  const mobile = fromMobile(await mobileBehaviour(i.prefs, i.failing));
  const desktop = await mountDesktop(i.prefs, i.failing);
  try {
    return { mobile, desktop: fromDesktop(desktop.current()) };
  } finally {
    desktop.unmount();
  }
}

export interface EffectiveInput {
  prefs: RawPrefs;
  listId: string;
}

export interface Effective {
  cascade: boolean;
  autoDate: boolean;
  carryOverDefault: string;
}

export async function answerEffective(i: EffectiveInput): Promise<BySurface<Effective>> {
  const m = mobileEffectiveForList(await mobileBehaviour(i.prefs), i.listId);
  const desktop = await mountDesktop(i.prefs);
  try {
    const d = desktop.current().effectiveForList(i.listId);
    return {
      mobile: { cascade: m.cascade, autoDate: m.autoDate, carryOverDefault: m.carryOverDefault },
      desktop: { cascade: d.cascade, autoDate: d.autoDate, carryOverDefault: d.carryOverDefault },
    };
  } finally {
    desktop.unmount();
  }
}

export async function answerCountdownWrite(value: Num): Promise<BySurface<string>> {
  vi.mocked(mobileSetUserPref).mockClear();
  await writeDeadlineCountdownDays(num(value));
  const mobile = mobileWrote(KEYS.deadlineCountdownDays) ?? '(nothing written)';
  const desktop = await mountDesktop({});
  try {
    act(() => desktop.current().setDeadlineCountdownDays(num(value)));
    // The provider persists `String(deadlineCountdownDays)`.
    return { mobile, desktop: String(desktop.current().deadlineCountdownDays) };
  } finally {
    desktop.unmount();
  }
}

export interface WindowWrite {
  start: string;
  end: string;
}

export async function answerDayWindowWrite(start: Num, end: Num): Promise<BySurface<WindowWrite>> {
  vi.mocked(mobileSetUserPref).mockClear();
  await writeDayWindow(num(start), num(end));
  const mobile = {
    start: mobileWrote(KEYS.dayStartMin) ?? '(nothing written)',
    end: mobileWrote(KEYS.dayEndMin) ?? '(nothing written)',
  };
  const desktop = await mountDesktop({});
  try {
    act(() => desktop.current().setDayWindow(num(start), num(end)));
    // The provider persists `String(dayStartMin)` and `String(dayEndMin)`.
    const v = desktop.current();
    return { mobile, desktop: { start: String(v.dayStartMin), end: String(v.dayEndMin) } };
  } finally {
    desktop.unmount();
  }
}

export interface OverrideUpdateInput {
  /** The stored override map before the change. */
  stored: string | null;
  listId: string;
  override: ListOverrides;
}

/** The override map each surface would store after the change, as the JSON
 *  it stores: its key order is what the sync comparison sees. */
export async function answerOverrideUpdate(i: OverrideUpdateInput): Promise<BySurface<string>> {
  const prefs: RawPrefs = { [KEYS.listOverrides]: i.stored };
  const b = await mobileBehaviour(prefs);
  const mobile = JSON.stringify(withListOverride(b.listOverrides, i.listId, i.override));
  const desktop = await mountDesktop(prefs);
  try {
    act(() => desktop.current().setListOverride(i.listId, i.override));
    return { mobile, desktop: JSON.stringify(desktop.current().listOverrides) };
  } finally {
    desktop.unmount();
  }
}
