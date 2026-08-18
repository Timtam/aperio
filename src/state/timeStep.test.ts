import { describe, expect, it } from 'vitest';

import { timeInputStep } from './timeStep';

describe('timeInputStep', () => {
  it('steps by the chosen amount when the value is on the grid', () => {
    expect(timeInputStep('09:30', 15)).toBe(900);
    expect(timeInputStep('09:00', 30)).toBe(1800);
    expect(timeInputStep('09:05', 5)).toBe(300);
  });

  it('falls back to whole minutes for an OFF-grid value', () => {
    // A time input whose value does not match its `step` is `:invalid`, and
    // with `required` that blocks the form. An event stored at 09:07 must not
    // become unsavable because the user picked a 15-minute step.
    expect(timeInputStep('09:07', 15)).toBe(60);
    expect(timeInputStep('09:31', 30)).toBe(60);
  });

  it('never constrains anything at a step of one minute', () => {
    expect(timeInputStep('09:07', 1)).toBe(60);
  });

  it('survives an empty or malformed value', () => {
    // Time fields are emptiable in the task editor, and a half-typed value
    // exists for as long as somebody is typing it.
    expect(timeInputStep('', 15)).toBe(60);
    expect(timeInputStep('nonsense', 15)).toBe(60);
    expect(timeInputStep('09:', 15)).toBe(60);
  });
});
