// A tap on the day-start notification asks for the day-start review
// (`ReminderTapRouter`); the day-start checks, which own the review, answer.
// A tap that comes before the checks have mounted (a cold start) waits here.

type Listener = () => void;

let listener: Listener | null = null;
let pending = false;

/** Ask for the day-start review, now or as soon as someone listens. */
export function requestDayStartReview(): void {
  if (listener) listener();
  else pending = true;
}

/** Listen for requests; a request that came before is answered at once.
 *  Returns the unsubscribe. */
export function onDayStartReviewRequested(next: Listener): () => void {
  listener = next;
  if (pending) {
    pending = false;
    next();
  }
  return () => {
    if (listener === next) listener = null;
  };
}
