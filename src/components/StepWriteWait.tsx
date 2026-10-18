import { useRef } from 'react';
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
