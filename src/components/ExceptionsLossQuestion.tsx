import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { exceptionsLossQuestion, type ExceptionsLoss } from '@aperio/shared';

import { ConfirmDialog } from './ConfirmDialog';

/**
 * The question a refused move of a series asks before Exchange drops
 * occurrences of it (decision 250): a confirm dialog of its own, after the
 * scope question, with the same words the editors use (what the move rewrites
 * and how many occurrences that loses). Yes runs `yes` — the same move again
 * with the consent — and Cancel leaves the series as it was.
 *
 * `ask` says whether it could ask: not once the view that asks is gone — the
 * user switched views while the move was on its way — and the caller then
 * says the refusal's sentence instead, so the move is never refused in
 * silence.
 */
export function useExceptionsLossQuestion(): {
  ask: (loss: ExceptionsLoss, title: string, yes: () => void) => boolean;
  dialog: ReactNode;
} {
  const { t } = useTranslation();
  const [asked, setAsked] = useState<{
    loss: ExceptionsLoss;
    title: string;
    yes: () => void;
  } | null>(null);
  const shown = useRef(false);
  useEffect(() => {
    shown.current = true;
    return () => {
      shown.current = false;
    };
  }, []);
  const ask = useCallback((loss: ExceptionsLoss, title: string, yes: () => void) => {
    if (!shown.current) return false;
    setAsked({ loss, title, yes });
    return true;
  }, []);
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
