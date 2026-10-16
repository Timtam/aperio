import { useCallback, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { exceptionsLossQuestion, type ExceptionsLoss } from '@aperio/shared';

import { ConfirmDialog } from './ConfirmDialog';

/**
 * The question a refused move of a series asks before Exchange drops
 * occurrences of it (decision 250): a confirm dialog of its own, after the
 * scope question, with the same words the editors use (what the move rewrites
 * and how many occurrences that loses). Yes runs `yes` — the same move again
 * with the consent — and Cancel leaves the series as it was.
 */
export function useExceptionsLossQuestion(): {
  ask: (loss: ExceptionsLoss, title: string, yes: () => void) => void;
  dialog: ReactNode;
} {
  const { t } = useTranslation();
  const [asked, setAsked] = useState<{
    loss: ExceptionsLoss;
    title: string;
    yes: () => void;
  } | null>(null);
  const ask = useCallback(
    (loss: ExceptionsLoss, title: string, yes: () => void) => setAsked({ loss, title, yes }),
    [],
  );
  const question = asked && exceptionsLossQuestion(asked.loss, asked.title, t);
  const dialog =
    asked && question ? (
      <ConfirmDialog
        isOpen
        onClose={() => setAsked(null)}
        title={question.title}
        message={question.message}
        confirmLabel={question.confirm}
        onConfirm={asked.yes}
      />
    ) : null;
  return { ask, dialog };
}
