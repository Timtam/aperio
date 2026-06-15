import { describe, expect, it } from 'vitest';

import { suppressGroupHeaderKey } from './taskTreeKeys';

const key = (over: {
  key: string;
  altKey?: boolean;
  ctrlKey?: boolean;
  metaKey?: boolean;
}) => ({ altKey: false, ctrlKey: false, metaKey: false, ...over });

describe('suppressGroupHeaderKey', () => {
  it('suppresses a plain key so the inert row stays quiet', () => {
    expect(suppressGroupHeaderKey(key({ key: 'd' }))).toBe(true);
    expect(suppressGroupHeaderKey(key({ key: 'Delete' }))).toBe(true);
  });

  it('lets Tab through so focus can move', () => {
    expect(suppressGroupHeaderKey(key({ key: 'Tab' }))).toBe(false);
  });

  it('lets OS / global modifier combos through to the window', () => {
    // The regression this guards: Alt+F4 must close the window even while a
    // group header is focused.
    expect(suppressGroupHeaderKey(key({ key: 'F4', altKey: true }))).toBe(false);
    expect(suppressGroupHeaderKey(key({ key: 'r', ctrlKey: true }))).toBe(false);
    expect(suppressGroupHeaderKey(key({ key: 'w', metaKey: true }))).toBe(false);
  });
});
