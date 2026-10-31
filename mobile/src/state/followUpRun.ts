/**
 * `run`, never twice at once and never dropped. A call while a run is under
 * way starts no second one beside it: one more run follows it, however many
 * calls came meanwhile. Each call's promise resolves once that follow-up is
 * done too, so a caller that awaits sees a run that started after its call.
 *
 * For work that reads and then acts on what it read, where a run that read
 * before a change must not be the last word on it. `run` is expected not to
 * throw; if it does, the loop stops there and the promise rejects.
 */
export function withFollowUp(run: () => Promise<void>): () => Promise<void> {
  let inFlight: Promise<void> | null = null;
  let again = false;
  return () => {
    if (inFlight != null) {
      again = true;
      return inFlight;
    }
    inFlight = (async () => {
      try {
        do {
          again = false;
          await run();
        } while (again);
      } finally {
        inFlight = null;
      }
    })();
    return inFlight;
  };
}
