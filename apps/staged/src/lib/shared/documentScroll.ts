// documentScroll.ts — keep the app shell's document unscrolled.
//
// The shell never scrolls the document (app.css: html, body
// { overflow: hidden }); its panes scroll themselves. `overflow: hidden`
// still allows programmatic scrolling, though, and iOS Safari uses it: it
// scrolls the page to reveal a focused input above the keyboard. Every fixed
// element and hit target is then offset by that amount.

/** Scroll the window and `<body>` back to the origin; no writes if already there. */
export function resetDocumentScroll(): void {
  if (window.scrollX !== 0 || window.scrollY !== 0) window.scrollTo(0, 0);
  const body = document.body;
  if (body.scrollTop !== 0) body.scrollTop = 0;
  if (body.scrollLeft !== 0) body.scrollLeft = 0;
}
