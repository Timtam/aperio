import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';

import { ReadOnlyText } from './ReadOnlyText';

describe('ReadOnlyText', () => {
  it('is a focusable document named by its label, one block per line', () => {
    render(<ReadOnlyText label="Notizen" value={'Erste Zeile\r\n\r\nDritte Zeile\nVierte'} />);
    const text = screen.getByRole('document', { name: 'Notizen' });
    expect(text.getAttribute('tabindex')).toBe('0');
    expect(Array.from(text.children, (line) => line.textContent)).toEqual([
      'Erste Zeile',
      '',
      'Dritte Zeile',
      'Vierte',
    ]);
    // A blank line keeps its height, so it is seen as one too.
    expect(text.children[1].querySelector('br')).not.toBeNull();
    expect(document.querySelector('textarea, input')).toBeNull();
  });
});
