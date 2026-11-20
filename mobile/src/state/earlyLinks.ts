import { Linking } from 'react-native';

// Links that arrive while no screen listens yet.
//
// Under the UIKit scene life cycle a launch of Aperio into the background (a
// background task after iOS ended the app) starts React Native without a
// window (`modules/background-launch`). The bundle runs, but no screen mounts.
// When the user then opens the app through a link — a Live Activity's
// `aperio://event`, a calendar file "opened in" Aperio — the scene's start
// finds React Native running and passes the link as an ordinary `url` event,
// not as the initial URL, and at that moment no screen listens: React Native
// drops an event nobody listens to. This listener exists from the moment the
// bundle runs, keeps such links, and hands them to the screen that owns them
// when it mounts.
//
// Imported by the entry file, so it is in place before anything else.

/** At most this many: a few taps before a screen mounts, never a backlog. */
const MAX_KEPT = 5;

const kept: string[] = [];
let listening = 0;

Linking.addEventListener('url', ({ url }) => {
  if (listening > 0) return;
  kept.push(url);
  if (kept.length > MAX_KEPT) kept.shift();
});

/**
 * Listen for links: first the kept ones this screen `owns`, in the order they
 * came, then every new one. Links another screen owns stay kept for it.
 * Returns the unsubscribe.
 */
export function listenForLinks(
  owns: (url: string) => boolean,
  handle: (url: string) => void,
): () => void {
  listening += 1;
  const mine = kept.filter(owns);
  for (const url of mine) kept.splice(kept.indexOf(url), 1);
  for (const url of mine) handle(url);
  const subscription = Linking.addEventListener('url', ({ url }) => handle(url));
  return () => {
    listening -= 1;
    subscription.remove();
  };
}
