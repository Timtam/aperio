import { afterEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import { SettingsSelectorDetail } from './SettingsSelectorDetail';

afterEach(() => {
  document.body.innerHTML = '';
});

interface Item {
  id: string;
  name: string;
}

const groups = [
  {
    id: 'acc1',
    label: 'iCloud',
    items: [
      { id: 'a', name: 'Privat' },
      { id: 'b', name: 'Arbeit' },
    ],
  },
  { id: 'acc2', label: 'Local', items: [{ id: 'c', name: 'Inbox' }] },
];

function renderSelector() {
  return render(
    <SettingsSelectorDetail<Item>
      groups={groups}
      getItemId={(i) => i.id}
      getItemName={(i) => i.name}
      getItemSummary={(i) => `${i.id}-sum`}
      getItemBadge={(i) => i.id}
      selectorLabel="Choose list"
      optionLabel={({ account, name, summary }) =>
        `${account} > ${name}, ${summary}`
      }
      detailHeading={({ account, name }) => `Editing ${account} / ${name}`}
      renderDetail={(item) => <button type="button">edit {item.name}</button>}
    />,
  );
}

describe('SettingsSelectorDetail', () => {
  it('is a single labelled listbox (one tab stop) defaulting to the first item', () => {
    renderSelector();
    const listbox = screen.getByRole('listbox', { name: 'Choose list' });
    expect(listbox).toHaveAttribute('tabindex', '0');

    // Selection follows focus via aria-activedescendant on the first option.
    const selected = screen.getByRole('option', { selected: true });
    expect(selected).toHaveAccessibleName('iCloud > Privat, a-sum');
    expect(listbox).toHaveAttribute('aria-activedescendant', selected.id);

    // The detail region is headed after the selected item.
    expect(
      screen.getByRole('region', { name: 'Editing iCloud / Privat' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'edit Privat' })).toBeInTheDocument();
  });

  it('every option carries "{account} > {name}, {summary}" as its accessible name', () => {
    renderSelector();
    expect(
      screen.getByRole('option', { name: 'iCloud > Arbeit, b-sum' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('option', { name: 'Local > Inbox, c-sum' }),
    ).toBeInTheDocument();
  });

  it('Arrow keys move the selection across account groups and swap the detail', () => {
    renderSelector();
    const listbox = screen.getByRole('listbox');

    fireEvent.keyDown(listbox, { key: 'ArrowDown' });
    expect(screen.getByRole('option', { selected: true })).toHaveAccessibleName(
      'iCloud > Arbeit, b-sum',
    );
    expect(
      screen.getByRole('region', { name: 'Editing iCloud / Arbeit' }),
    ).toBeInTheDocument();

    // Crosses the account boundary into the second group.
    fireEvent.keyDown(listbox, { key: 'ArrowDown' });
    expect(screen.getByRole('option', { selected: true })).toHaveAccessibleName(
      'Local > Inbox, c-sum',
    );
    expect(screen.getByRole('button', { name: 'edit Inbox' })).toBeInTheDocument();

    // Clamps at the end (no wrap).
    fireEvent.keyDown(listbox, { key: 'ArrowDown' });
    expect(screen.getByRole('option', { selected: true })).toHaveAccessibleName(
      'Local > Inbox, c-sum',
    );

    fireEvent.keyDown(listbox, { key: 'ArrowUp' });
    expect(screen.getByRole('option', { selected: true })).toHaveAccessibleName(
      'iCloud > Arbeit, b-sum',
    );

    fireEvent.keyDown(listbox, { key: 'Home' });
    expect(screen.getByRole('option', { selected: true })).toHaveAccessibleName(
      'iCloud > Privat, a-sum',
    );

    fireEvent.keyDown(listbox, { key: 'End' });
    expect(screen.getByRole('option', { selected: true })).toHaveAccessibleName(
      'Local > Inbox, c-sum',
    );
  });

  it('clicking an option selects it and swaps the detail', () => {
    renderSelector();
    fireEvent.click(screen.getByRole('option', { name: 'iCloud > Arbeit, b-sum' }));
    expect(screen.getByRole('option', { selected: true })).toHaveAccessibleName(
      'iCloud > Arbeit, b-sum',
    );
    expect(
      screen.getByRole('region', { name: 'Editing iCloud / Arbeit' }),
    ).toBeInTheDocument();
  });

  it('opens on `preferredItemId` instead of the first item, and falls back when it is not in the list', () => {
    const { unmount } = render(
      <SettingsSelectorDetail<Item>
        groups={groups}
        getItemId={(i) => i.id}
        getItemName={(i) => i.name}
        getItemSummary={() => 'sum'}
        selectorLabel="Choose list"
        preferredItemId="c"
        optionLabel={({ name }) => name}
        detailHeading={({ name }) => `Editing ${name}`}
        renderDetail={() => null}
      />,
    );
    expect(screen.getByRole('option', { selected: true })).toHaveAccessibleName(
      'Inbox',
    );
    unmount();

    render(
      <SettingsSelectorDetail<Item>
        groups={groups}
        getItemId={(i) => i.id}
        getItemName={(i) => i.name}
        getItemSummary={() => 'sum'}
        selectorLabel="Choose list"
        preferredItemId="gone"
        optionLabel={({ name }) => name}
        detailHeading={({ name }) => `Editing ${name}`}
        renderDetail={() => null}
      />,
    );
    expect(screen.getByRole('option', { selected: true })).toHaveAccessibleName(
      'Privat',
    );
  });

  it('reports every selection move to `onSelectionChange`', () => {
    const seen: (string | null)[] = [];
    const onSelectionChange = (id: string | null) => {
      seen.push(id);
    };
    render(
      <SettingsSelectorDetail<Item>
        groups={groups}
        getItemId={(i) => i.id}
        getItemName={(i) => i.name}
        getItemSummary={() => 'sum'}
        selectorLabel="Choose list"
        onSelectionChange={onSelectionChange}
        optionLabel={({ name }) => name}
        detailHeading={({ name }) => name}
        renderDetail={() => null}
      />,
    );
    expect(seen).toEqual([null, 'a']);

    fireEvent.keyDown(screen.getByRole('listbox'), { key: 'ArrowDown' });
    expect(seen).toEqual([null, 'a', 'b']);
  });

  it('renders the detail heading at the level the caller asks for', () => {
    const { unmount } = render(
      <SettingsSelectorDetail<Item>
        groups={groups}
        getItemId={(i) => i.id}
        getItemName={(i) => i.name}
        getItemSummary={() => 'sum'}
        selectorLabel="Choose list"
        optionLabel={({ name }) => name}
        detailHeading={({ name }) => name}
        renderDetail={() => null}
      />,
    );
    expect(screen.getByRole('heading', { level: 3, name: 'Privat' })).toBeInTheDocument();
    unmount();

    render(
      <SettingsSelectorDetail<Item>
        groups={groups}
        getItemId={(i) => i.id}
        getItemName={(i) => i.name}
        getItemSummary={() => 'sum'}
        selectorLabel="Choose list"
        detailHeadingLevel={4}
        optionLabel={({ name }) => name}
        detailHeading={({ name }) => name}
        renderDetail={() => null}
      />,
    );
    expect(screen.getByRole('heading', { level: 4, name: 'Privat' })).toBeInTheDocument();
  });

  it('renders nothing fatal for an empty group set', () => {
    render(
      <SettingsSelectorDetail<Item>
        groups={[]}
        getItemId={(i) => i.id}
        getItemName={(i) => i.name}
        getItemSummary={() => ''}
        selectorLabel="Choose list"
        optionLabel={({ name }) => name}
        detailHeading={({ name }) => name}
        renderDetail={() => null}
      />,
    );
    expect(screen.getByRole('listbox')).toBeInTheDocument();
    expect(screen.queryByRole('option')).toBeNull();
    expect(screen.queryByRole('region')).toBeNull();
  });
});
