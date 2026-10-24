/**
 * Where focus goes after an action of the day-start review failed and its
 * rows read again (decision 273): `null` leaves it where the person left it,
 * on the pressed button, which still shows while any row it worked on does —
 * a bulk button whose section keeps a row of its batch among them. Only when
 * the re-read took every one of those rows away (the write landed for each)
 * does focus move on, to the row queued for after them.
 */
export function focusAfterFailedAction(
  gone: { acted: readonly string[]; fallback: string | null },
  orderedIds: readonly string[],
): string | null {
  if (gone.acted.some((id) => orderedIds.includes(id))) return null;
  return gone.fallback;
}
