import { PixelRatio } from 'react-native';

// Chrome dimensions (paddings, fixed button sizes, tile min-heights) don't
// follow the OS font scale the way Text does — so scaling text down left
// oversized buttons around shrunken labels. `chrome(px)` scales a dimension
// by the font scale, clamped so extreme scales can't collapse touch targets
// or blow up layouts. Read once at import: styles are built once per theme.
// (Android relaunches the JS on a font-scale change; iOS Dynamic Type updates
// Text live, so an in-session change shows mixed sizing until the next launch
// — the same deliberate trade-off as CalendarDayList's grid FONT_SCALE.)
const FONT_SCALE = Math.min(Math.max(PixelRatio.getFontScale(), 0.75), 1.4);

/** Scale a chrome dimension with the OS font scale (clamped 0.75–1.4). */
export function chrome(px: number): number {
  return Math.round(px * FONT_SCALE);
}

/** Like {@link chrome}, but floored at the 44pt platform minimum touch-target
 *  size — for TAP TARGETS with fixed width/height (nav buttons), which must
 *  not shrink below the minimum at small font scales. */
export function chromeTouch(px: number): number {
  return Math.max(44, chrome(px));
}
