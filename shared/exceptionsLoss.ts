// Asking before a save makes the provider drop occurrences of a series the
// user changed or deleted on their own (decisions 243-253), and naming the
// deleted ones that came back when Aperio could not delete them again.
//
// Only the adapter knows, from the provider's fresh copy, what an update
// rewrites and how many occurrences that loses. Without the user's consent it
// sends nothing and refuses with
// `exceptions-would-be-lost: {rewrite}:{changed}:{deleted}`
// (`cal_core::SeriesRewrite::detail`). Both editors ask with the sentence
// built here and send the same save again with `accepts_exception_loss`; a
// path that does not ask yet says the same sentence as a refusal.

import type { SeriesRewrite } from './generated/SeriesRewrite';
import { localDateKey } from './dateKey';
import { formatLongDay } from './intlNames';
import { joinNames } from './listWords';

type Translate = (key: string, values?: Record<string, unknown>) => string;

/** What a save would rewrite, and how many occurrences it would lose. */
export interface ExceptionsLoss {
  rewrite: SeriesRewrite;
  /** Occurrences changed on their own: they take the series' details again. */
  changed: number;
  /** Deleted occurrences Aperio cannot delete again: they come back. */
  deleted: number;
}

const REWRITES: readonly SeriesRewrite[] = ['slot', 'zone', 'pattern'];

/** The loss a refusal's detail names, as `cal_core::SeriesRewrite::detail`
 *  writes it (`slot:2:1`); `null` for anything else. */
export function parseExceptionsLoss(detail: string): ExceptionsLoss | null {
  const parts = detail.trim().split(':');
  if (parts.length !== 3) return null;
  const [token, changed, deleted] = parts;
  const rewrite = REWRITES.find((r) => r === token);
  if (!rewrite || !/^\d+$/.test(changed) || !/^\d+$/.test(deleted)) return null;
  return { rewrite, changed: Number(changed), deleted: Number(deleted) };
}

/** What the save rewrites and what that loses, as two sentences. */
export function exceptionsLossSentences(
  loss: ExceptionsLoss,
  t: Translate,
): { what: string; lost: string } {
  const what = t(`dialogs.event.exceptionsLoss.rewrite.${loss.rewrite}`);
  const lost =
    loss.changed > 0 && loss.deleted > 0
      ? t('dialogs.event.exceptionsLoss.lost.both', {
          changed: t('dialogs.event.exceptionsLoss.count.changed', { count: loss.changed }),
          deleted: t('dialogs.event.exceptionsLoss.count.deleted', { count: loss.deleted }),
        })
      : loss.deleted > 0
        ? t('dialogs.event.exceptionsLoss.lost.deleted', { count: loss.deleted })
        : t('dialogs.event.exceptionsLoss.lost.changed', { count: loss.changed });
  return { what, lost };
}

/** The question both editors ask before saving anyway. */
export function exceptionsLossQuestion(
  loss: ExceptionsLoss,
  title: string,
  t: Translate,
): { title: string; message: string; confirm: string } {
  const { what, lost } = exceptionsLossSentences(loss, t);
  return {
    title: t('dialogs.event.exceptionsLoss.title'),
    message: t('dialogs.event.exceptionsLoss.message', { title, what, lost }),
    confirm: t('dialogs.event.exceptionsLoss.confirm'),
  };
}

/**
 * The sentence naming the deleted occurrences that came back and could not be
 * deleted again, by their days; `null` when there are none. The save itself
 * landed, so it says so first.
 */
export function deletionsNotRestoredSentence(
  instants: readonly string[] | undefined,
  title: string,
  language: string,
  t: Translate,
): string | null {
  if (!instants || instants.length === 0) return null;
  const days = instants.map((iso) => {
    const at = new Date(iso);
    return Number.isNaN(at.getTime()) ? iso : formatLongDay(localDateKey(at), language);
  });
  return t('dialogs.event.deletionsNotRestored', {
    title,
    count: instants.length,
    days: joinNames(days, t),
  });
}
