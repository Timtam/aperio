/**
 * `step` for a time input, in seconds — but only while the current value sits
 * ON the grid.
 *
 * A time input whose value does not match its `step` is `:invalid`, and with
 * `required` that blocks the whole form. An event stored at 09:07 would become
 * unsavable the moment the user picked a 15-minute step, which is a far worse
 * bug than the one the step is here to fix. So an off-grid value keeps
 * minute-by-minute stepping until it is moved onto the grid, and from then on
 * the step applies.
 */
export function timeInputStep(value: string, stepMinutes: number): number {
  if (stepMinutes <= 1) return 60;
  const parts = value.split(':');
  if (parts.length < 2) return 60;
  // `Number('')` is 0, not NaN — a half-typed "09:" would otherwise look like
  // a clean o'clock and snap the field to the step mid-keystroke.
  const [hh, mm] = parts;
  if (hh.trim() === '' || mm.trim() === '') return 60;
  const h = Number(hh);
  const m = Number(mm);
  if (!Number.isFinite(h) || !Number.isFinite(m)) return 60;
  return m % stepMinutes === 0 ? stepMinutes * 60 : 60;
}
