import { describe, expect, it, vi } from 'vitest';

import {
  duringDayMarkerBurst,
  notifyDayMarkersChanged,
  subscribeDayMarkersChanged,
} from './dayMarkersChanged';

describe('dayMarkersChanged', () => {
  it('tells every subscriber, and stops once unsubscribed', () => {
    const a = vi.fn();
    const b = vi.fn();
    const offA = subscribeDayMarkersChanged(a);
    const offB = subscribeDayMarkersChanged(b);

    notifyDayMarkersChanged();
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(1);

    offA();
    notifyDayMarkersChanged();
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(2);
    offB();
  });

  it('collapses a burst into one notification, after it finishes', async () => {
    const seen: string[] = [];
    const off = subscribeDayMarkersChanged(() => seen.push('notified'));

    await duringDayMarkerBurst(async () => {
      // A reorder writing three shifted rows: nobody should re-read between
      // them and see a half-applied order.
      notifyDayMarkersChanged();
      seen.push('wrote 1');
      notifyDayMarkersChanged();
      seen.push('wrote 2');
      notifyDayMarkersChanged();
      seen.push('wrote 3');
    });

    expect(seen).toEqual(['wrote 1', 'wrote 2', 'wrote 3', 'notified']);
    off();
  });

  it('stays silent for a burst that wrote nothing', async () => {
    const listener = vi.fn();
    const off = subscribeDayMarkersChanged(listener);

    await duringDayMarkerBurst(async () => {
      // The reorder was a no-op — the marker was already at the end.
    });

    expect(listener).not.toHaveBeenCalled();
    off();
  });

  it('still notifies when the burst throws', async () => {
    const listener = vi.fn();
    const off = subscribeDayMarkersChanged(listener);

    await expect(
      duringDayMarkerBurst(async () => {
        // Two rows landed before the third was rejected. The two that DID land
        // are real, so the readers have to hear about them — swallowing the
        // notification here would leave the list showing the old order with
        // the new one on disk.
        notifyDayMarkersChanged();
        throw new Error('write failed');
      }),
    ).rejects.toThrow('write failed');

    expect(listener).toHaveBeenCalledTimes(1);
    off();
  });

  it('only re-arms once the outermost burst ends', async () => {
    const listener = vi.fn();
    const off = subscribeDayMarkersChanged(listener);

    await duringDayMarkerBurst(async () => {
      await duringDayMarkerBurst(async () => {
        notifyDayMarkersChanged();
      });
      expect(listener).not.toHaveBeenCalled();
    });

    expect(listener).toHaveBeenCalledTimes(1);
    off();
  });
});
