// The log text the feedback form holds, for the screen that reads it out line
// by line. Not a route param: up to 2 MB has no business in navigation state,
// which React Navigation keeps and may persist. One form exists at a time, so
// one slot is enough; the form sets it whenever its text changes.

let held: string | null = null;

export function holdFeedbackLogs(text: string | null): void {
  held = text;
}

export function heldFeedbackLogs(): string | null {
  return held;
}

/** The held text as the lines the reading screen lists; a final newline
 *  makes no empty last line. */
export function feedbackLogLines(text: string | null): string[] {
  if (!text) return [];
  const lines = text.split('\n');
  if (lines[lines.length - 1] === '') lines.pop();
  return lines;
}
