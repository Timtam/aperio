import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';

import type { WeekdayQuestionText } from '@aperio/shared';

import { WeekdayQuestionDialog } from './WeekdayQuestionDialog';

/**
 * The weekday question on the desktop (138a): "Zeitzone nicht ändern" first
 * and focused, the answers after it in the order the question gives them, the
 * message as every button's description, Escape as "nicht ändern", and no
 * button styled as a danger or a default.
 */

afterEach(() => {
  document.body.innerHTML = '';
});

const QUESTION: WeekdayQuestionText = {
  title: 'Welcher Wochentag soll gelten?',
  message: 'In Berlin fällt der Beginn auf Montag, 23. November, 00:30; die Regel sagt jeden Sonntag.',
  keep: 'Zeitzone nicht ändern',
  answers: [
    { answer: 'rule', label: 'Jeden Sonntag, wie die Regel sagt (Termine rücken einen Tag vor)' },
    { answer: 'now', label: 'Jeden Montag, wo die Termine jetzt liegen' },
  ],
};

function open(question: WeekdayQuestionText = QUESTION) {
  const onKeep = vi.fn();
  const onAnswer = vi.fn();
  render(<WeekdayQuestionDialog isOpen question={question} onKeep={onKeep} onAnswer={onAnswer} />);
  const dialog = screen.getByRole('dialog', { name: question.title });
  const buttons = within(dialog)
    .getAllByRole('button')
    .filter((button) => !button.classList.contains('modal__close'));
  return { dialog, buttons, onKeep, onAnswer };
}

describe('WeekdayQuestionDialog', () => {
  it('puts "Zeitzone nicht ändern" first and focused, then the answers', () => {
    const { buttons } = open();
    expect(buttons.map((button) => button.textContent)).toEqual([
      'Zeitzone nicht ändern',
      'Jeden Sonntag, wie die Regel sagt (Termine rücken einen Tag vor)',
      'Jeden Montag, wo die Termine jetzt liegen',
    ]);
    expect(document.activeElement).toBe(buttons[0]);
  });

  it('describes every button with the message, and styles none as danger or default', () => {
    const { buttons } = open();
    for (const button of buttons) {
      expect(button).toHaveAccessibleDescription(QUESTION.message);
      expect(button.className).not.toMatch(/danger|primary/);
    }
  });

  it('keeps with the button and with Escape, and answers with the answer', () => {
    const { buttons, onKeep, onAnswer } = open();
    fireEvent.click(buttons[0]);
    expect(onKeep).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(buttons[1], { key: 'Escape' });
    expect(onKeep).toHaveBeenCalledTimes(2);
    expect(onAnswer).not.toHaveBeenCalled();
    fireEvent.click(buttons[1]);
    fireEvent.click(buttons[2]);
    expect(onAnswer.mock.calls).toEqual([['rule'], ['now']]);
    expect(onKeep).toHaveBeenCalledTimes(2);
  });

  it('has no button for an answer it was not given', () => {
    const { buttons } = open({ ...QUESTION, answers: [QUESTION.answers[0]] });
    expect(buttons.map((button) => button.textContent)).toEqual([
      'Zeitzone nicht ändern',
      'Jeden Sonntag, wie die Regel sagt (Termine rücken einen Tag vor)',
    ]);
  });
});
