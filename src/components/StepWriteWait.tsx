import { useLayoutEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import { Modal } from './Modal';

/**
 * The "please wait" dialog while a series is created in steps (decisions 227,
 * 265): Exchange takes a new series without its deleted occurrences, and the
 * adapter deletes them again one by one, which can take up to half a minute.
 * Without it a blind user heard silence for that long and could not tell work
 * from a hang, and a sighted one saw a form that looked ready.
 *
 * It has no button and closes when the write is done; its sentence is its one
 * stop and what the screen reader reads on open.
 */
export function StepWriteWaitDialog({ open }: { open: boolean }) {
  const { t } = useTranslation();
  const noteRef = useRef<HTMLParagraphElement>(null);
  return (
    <Modal
      isOpen={open}
      onClose={ignore}
      title={t('dialogs.stepWrite.title')}
      className="modal--confirm"
      dismissOnBackdrop={false}
      initialFocusRef={noteRef}
      busy
    >
      <div className="form">
        <FocusableNote ref={noteRef} className="form__hint">
          {t('dialogs.stepWrite.body')}
        </FocusableNote>
      </div>
    </Modal>
  );
}

function ignore() {}

/**
 * The "please wait" dialog of a surface without a dialog of its own — a drop
 * in the sidebar, Ctrl+D —, from the dialog stack (`stepWriteWait`): while the
 * write runs, as above; when it failed, the same dialog says why, focused,
 * with only Close (decision 210), for sighted users as for the screen reader.
 */
export function StepWriteDialog({
  notice,
  onClose,
}: {
  notice: string | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const noteRef = useRef<HTMLParagraphElement>(null);
  // The sentence that replaced the wait is a new stop: focused at once, so the
  // screen reader reads it and focus never falls to the page.
  useLayoutEffect(() => {
    if (notice != null) noteRef.current?.focus({ preventScroll: true });
  }, [notice]);
  if (notice == null) {
    return (
      <Modal
        isOpen
        onClose={ignore}
        title={t('dialogs.stepWrite.title')}
        className="modal--confirm"
        dismissOnBackdrop={false}
        initialFocusRef={noteRef}
        busy
      >
        <div className="form">
          <FocusableNote ref={noteRef} className="form__hint">
            {t('dialogs.stepWrite.body')}
          </FocusableNote>
        </div>
      </Modal>
    );
  }
  return (
    <Modal
      isOpen
      onClose={onClose}
      title={t('dialogs.stepWrite.failedTitle')}
      className="modal--confirm"
      dismissOnBackdrop={false}
      initialFocusRef={noteRef}
    >
      <div className="form">
        <FocusableNote key="notice" ref={noteRef} className="form__hint form__hint--warning">
          {notice}
        </FocusableNote>
        <div className="form__actions">
          <button type="button" className="form__action form__action--primary" onClick={onClose}>
            {t('dialogs.close')}
          </button>
        </div>
      </div>
    </Modal>
  );
}
