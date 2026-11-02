// documentScroll.ts — keep the app shell's document unscrolled.
//
// The shell never scrolls the document (app.css: html, body
// { overflow: hidden }); its panes scroll themselves. `overflow: hidden`
// still allows programmatic scrolling, though, and iOS Safari uses it: it
// scrolls the page to reveal a focused input above the keyboard, and
// scrolls `<body>` to bring an element into view when focus returns to it
// (issue 02: a dialog's trigger left `body.scrollTop` at 40 px for good).
// Every fixed element and hit target is then offset by that amount.

import { isTextInput } from './textInput';
import { isPinchZoomed } from './viewportGeometry';

/** Scroll the window and `<body>` back to the origin; no writes if already there. */
export function resetDocumentScroll(): void {
  if (window.scrollX !== 0 || window.scrollY !== 0) window.scrollTo(0, 0);
  const body = document.body;
  if (body.scrollTop !== 0) body.scrollTop = 0;
  if (body.scrollLeft !== 0) body.scrollLeft = 0;
}

function shouldKeepDocumentScroll(): boolean {
  // A pinch-zoomed page is panned by the user; leave it where they put it.
  if (isPinchZoomed(window.visualViewport?.scale ?? 1)) return true;
  // Safari's scroll is what keeps a focused input above the keyboard. A
  // full-screen dialog fits itself above the keyboard instead (see
  // visualViewport.svelte.ts), so only its inputs give up the scroll.
  const active = document.activeElement;
  return isTextInput(active) && !active?.closest('[data-full-screen]');
}

/**
 * Undo any scroll of the window (the scrolling element) or `<body>` as it
 * happens. A no-op wherever the document never scrolls. Returns the cleanup.
 */
export function guardDocumentScroll(): () => void {
  if (typeof window === 'undefined') return () => {};
  const body = document.body;
  const onScroll = () => {
    if (!shouldKeepDocumentScroll()) resetDocumentScroll();
  };
  // Element scroll events do not bubble, so <body> needs its own listener;
  // the window's covers the scrolling element.
  window.addEventListener('scroll', onScroll, { passive: true });
  body.addEventListener('scroll', onScroll, { passive: true });
  return () => {
    window.removeEventListener('scroll', onScroll);
    body.removeEventListener('scroll', onScroll);
  };
}
