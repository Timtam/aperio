import { describe, expect, it } from 'vitest';

import { detectLinks } from './links';

describe('detectLinks', () => {
  it('returns nothing for empty / nullish input', () => {
    expect(detectLinks('')).toEqual([]);
    expect(detectLinks(null)).toEqual([]);
    expect(detectLinks(undefined)).toEqual([]);
    expect(detectLinks('no links here, just words')).toEqual([]);
  });

  it('detects http and https URLs', () => {
    const links = detectLinks('see http://a.com and https://b.com/x?y=1#z');
    expect(links.map((l) => l.url)).toEqual([
      'http://a.com',
      'https://b.com/x?y=1#z',
    ]);
  });

  it('normalises bare www. to http://', () => {
    const links = detectLinks('visit www.example.com today');
    expect(links).toHaveLength(1);
    expect(links[0].url).toBe('http://www.example.com');
  });

  it('detects bare email addresses as mailto:', () => {
    const links = detectLinks('mail me at user@example.com please');
    expect(links).toHaveLength(1);
    expect(links[0].url).toBe('mailto:user@example.com');
  });

  it('strips trailing sentence punctuation', () => {
    expect(detectLinks('go to https://example.com.')[0].url).toBe(
      'https://example.com',
    );
  });

  it('handles a URL wrapped in parentheses', () => {
    expect(detectLinks('(see https://example.com)')[0].url).toBe(
      'https://example.com',
    );
  });

  it('preserves order across multiple links', () => {
    const links = detectLinks('first https://one.com then https://two.com');
    expect(links.map((l) => l.url)).toEqual([
      'https://one.com',
      'https://two.com',
    ]);
  });

  it('collapses duplicate URLs to the first occurrence', () => {
    const links = detectLinks('https://dup.com and again https://dup.com');
    expect(links.map((l) => l.url)).toEqual(['https://dup.com']);
  });

  it('drops disallowed schemes (ftp, file, javascript, custom)', () => {
    // linkify-it doesn't match most of these by default, and the
    // allowlist filter is the backstop either way — the net result
    // must be no openable links.
    const links = detectLinks(
      'ftp://f.com file:///etc/passwd javascript:alert(1) app://x',
    );
    expect(links).toEqual([]);
  });

  it('keeps the http(s) link even when sat next to a disallowed one', () => {
    const links = detectLinks('bad file:///x but good https://ok.com');
    expect(links.map((l) => l.url)).toEqual(['https://ok.com']);
  });
});
