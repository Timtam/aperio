import { afterLanded, type CodedError } from '@aperio/shared';

import { setSectionColor } from '../api/client';

/**
 * A section's colour, written once the section itself was created or
 * renamed. That write landed, so this one's failure is a later step's
 * (decision 277), never "nothing was changed": the section shows as it now
 * stands, and the editor closes, since creating it again would make two.
 * `null` when the colour went.
 */
export function sectionColorAfterLanded(
  sectionId: string,
  listId: string,
  color: string | null,
): Promise<CodedError | null> {
  return setSectionColor(sectionId, listId, color).then(
    () => null,
    (err: unknown) => afterLanded(err, 'section-color'),
  );
}
