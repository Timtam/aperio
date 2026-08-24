// One-shot override for the CalendarPager's NEXT period announcement,
// registered by a programmatic jump ("Go to the current task") just before it
// moves the anchor. The pager's period announce is deliberately interrupting,
// so on a cross-period jump it would cut off the jump's own "Now showing
// <day>" utterance and speak only the bare week/month name — the one label
// that does NOT contain the target day. Registering the richer text here
// makes the interrupting utterance the informative one.
//
// Short-lived by design: the pager consumes it on the very next commit, and
// the expiry keeps a registration whose jump did NOT change the period
// (consumed nothing) from resurfacing on a later manual page flip. Lives in
// its own file (not CalendarPager.tsx) per the react-refresh
// only-export-components rule.

let override: { text: string; expires: number } | null = null;

export function overrideNextPeriodAnnounce(text: string): void {
  override = { text, expires: Date.now() + 2_000 };
}

export function takePeriodAnnounceOverride(): string | null {
  const held = override;
  override = null;
  if (held == null || Date.now() > held.expires) return null;
  return held.text;
}
