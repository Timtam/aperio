// A tap on the day-start notification asks for the day-start review, and a tap
// on any other reminder asks it to close before an editor opens behind it
// (`ReminderTapRouter`). The day-start checks, which own the review, answer.
// An opening asked for before the checks have mounted (a cold start) waits
// here; a closing then has nothing to close.

interface Handlers {
  open: () => void;
  close: () => void;
}

let handlers: Handlers | null = null;
let pending = false;

/** Ask for the day-start review, now or as soon as someone listens. */
export function requestDayStartReview(): void {
  if (handlers) handlers.open();
  else pending = true;
}

/** Close the review if it is open: the review is a native modal, so an editor
 *  opened under it would be out of sight and out of reach. */
export function closeDayStartReview(): void {
  pending = false;
  handlers?.close();
}

/** Answer the requests; an opening asked for before is answered at once.
 *  Returns the unsubscribe. */
export function onDayStartReviewRequests(next: Handlers): () => void {
  handlers = next;
  if (pending) {
    pending = false;
    next.open();
  }
  return () => {
    if (handlers === next) handlers = null;
  };
}
