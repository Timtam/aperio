import { describe, expect, it } from 'vitest';

import { nextCycleStatus } from './useTaskStatusToggle';
import type { TaskStatus } from '../api/types';

describe('nextCycleStatus (three-state check-off)', () => {
  it('steps open → in_progress → completed → open', () => {
    expect(nextCycleStatus('open')).toBe('in_progress');
    expect(nextCycleStatus('in_progress')).toBe('completed');
    expect(nextCycleStatus('completed')).toBe('open');
  });

  it('re-enters the cycle from cancelled (a check-off un-cancels)', () => {
    expect(nextCycleStatus('cancelled')).toBe('open');
  });

  it('completes a full loop back to the start', () => {
    let s: TaskStatus = 'open';
    s = nextCycleStatus(s); // in_progress
    s = nextCycleStatus(s); // completed
    s = nextCycleStatus(s); // open
    expect(s).toBe('open');
  });
});
