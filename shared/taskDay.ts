// Pure date helpers shared by the desktop and mobile frontends. Today only the
// "today" key lives here; the rest of the desktop's src/intl/taskDay.ts (the
// calendar day-bucketing helpers) can converge into this module later.

/**
 * Local `YYYY-MM-DD` for today.
 *
 * Built from the local wall-clock (getFullYear/getMonth/getDate), NOT a
 * `toISOString().slice(0, 10)` — a UTC slice would roll the day over at the
 * wrong moment and mis-bucket the Upcoming/Deferred gate (DESIGN §9.12) and the
 * "resurfaces on" due text near midnight.
 */
export function todayIsoKey(): string {
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}
