/**
 * sectionLinks — links from rendered markdown to a place in the same
 * document: `[dry summer](#how-the-beds-cope-with-a-dry-summer)`.
 *
 * The rendered headings carry no ids. The sanitizer strips them, and an id
 * is document-wide while two notes can be on screen at once (a project note
 * with a child note opened over it) and share heading text. So a link is
 * resolved when it is followed, against the headings of the markdown it sits
 * in: their slugs are derived from their text by GitHub's rule
 * (headingSlug.ts) and the first match is scrolled to. A fragment no heading
 * matches may still name an anchor the author wrote (`<a name="manual">`),
 * which the sanitizer keeps. Nothing outside that markdown is looked at, and
 * the webview never navigates.
 *
 * This is about `href`s. A `#note:<id>` / `#commit:<sha>` / `#review:<id>`
 * reference in prose is not a link at all: it renders as a badge
 * (`[data-hashtag-ref]`) with its own click handling, which render sites run
 * first. Written as a link target (`[x](#note:abc)`) it is a fragment like
 * any other and matches no heading, since a slug never holds a `:`.
 */

import { createHeadingSlugger, lowercase } from './headingSlug';

const HEADINGS = 'h1, h2, h3, h4, h5, h6';

/**
 * The fragment a link points at when its target is a fragment and nothing
 * else (`#ordered-steps`), else null: `https://…#x`, `page.md#x` and
 * `buzz://…` go elsewhere and are not section links.
 *
 * The markdown renderer percent-encodes link targets, so a non-ASCII slug
 * arrives encoded and is decoded here. A target that is not valid encoding
 * is taken as written.
 */
export function sectionLinkFragment(href: string | null | undefined): string | null {
  if (!href?.startsWith('#')) return null;
  const raw = href.slice(1);
  try {
    return decodeURIComponent(raw);
  } catch {
    return raw;
  }
}

/**
 * The text a heading is slugged by: what its markdown source says, which is
 * what GitHub slugs and what an author derives a link from. That is the
 * rendered text except where the note viewer draws something else:
 *
 * - A hashtag reference is drawn as a badge labelled with the title of what
 *   it refers to, or a shortened id. The badge carries the reference as
 *   written (`data-hashtag-ref`), so the slug is the same whether or not the
 *   item has loaded and whatever it is renamed to.
 * - An embedded image is drawn as a figure that repeats its alt text in a
 *   caption. An image has no text on GitHub, so the figure has none here.
 */
export function headingSlugText(node: Node): string {
  if (node.nodeType === Node.TEXT_NODE) return node.nodeValue ?? '';
  if (node.nodeType !== Node.ELEMENT_NODE) return '';
  const element = node as Element;
  const reference = element.getAttribute('data-hashtag-ref');
  if (reference !== null) return reference;
  if (element.matches('figure.markdown-media')) return '';
  let text = '';
  for (const child of element.childNodes) text += headingSlugText(child);
  return text;
}

/**
 * The heading in `content` that `fragment` names, or null.
 *
 * Every heading is slugged in document order with one slugger, so repeated
 * heading text resolves as on GitHub: the first is `notes`, the next
 * `notes-1`. Slugs are lowercase, so the fragment is matched lowercased, the
 * way a slug is, and `#Ordered-Steps` finds the same heading as
 * `#ordered-steps`.
 */
export function findSectionHeading(content: Element, fragment: string): HTMLElement | null {
  const wanted = lowercase(fragment);
  if (!wanted) return null;
  const slug = createHeadingSlugger();
  for (const heading of content.querySelectorAll<HTMLElement>(HEADINGS)) {
    if (slug(headingSlugText(heading)) === wanted) return heading;
  }
  return null;
}

/**
 * The anchor in `content` named `fragment` (`<a name="manual"></a>`), or
 * null. A name is matched exactly, as a browser matches it: unlike a slug it
 * is the author's own spelling. Compared here rather than in a selector, so
 * no fragment has to be escaped into one.
 */
export function findNamedAnchor(content: Element, fragment: string): HTMLElement | null {
  if (!fragment) return null;
  for (const anchor of content.querySelectorAll<HTMLElement>('a[name]')) {
    if (anchor.getAttribute('name') === fragment) return anchor;
  }
  return null;
}

/** What `fragment` names in `content`: a heading, else a named anchor, else null. */
export function findSectionTarget(content: Element, fragment: string): HTMLElement | null {
  return findSectionHeading(content, fragment) ?? findNamedAnchor(content, fragment);
}

function prefersReducedMotion(): boolean {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
}

/** The target's `scroll-margin-top` in px: the room to leave above it. */
function scrollMarginTop(target: HTMLElement): number {
  return parseFloat(getComputedStyle(target).getPropertyValue('scroll-margin-top')) || 0;
}

/**
 * Where `scroller` has to be scrolled to for `target` to sit `margin` below
 * its top edge, as the layout stands now. While a scroll runs and nothing
 * else moves, this stays the same number.
 */
function restingScrollTop(target: HTMLElement, scroller: HTMLElement, margin: number): number {
  const portTop = scroller.getBoundingClientRect().top + scroller.clientTop;
  const offset = target.getBoundingClientRect().top - portTop - margin;
  return Math.max(scroller.scrollTop + offset, 0);
}

/**
 * Open every closed `<details>` around `target`, up to `within`, so it is
 * laid out and can take focus: inside a closed one it has no box to scroll
 * to. A target in a block's own `<summary>` shows while the block is closed,
 * so that block is left as the reader had it.
 */
function openEnclosingDetails(target: HTMLElement, within: HTMLElement): void {
  for (let el = target.parentElement; el && el !== within; el = el.parentElement) {
    if (el.localName !== 'details') continue;
    const details = el as HTMLDetailsElement;
    if (details.open) continue;
    const summary = [...details.children].find((child) => child.localName === 'summary');
    if (!summary?.contains(target)) details.open = true;
  }
}

/**
 * Move focus to `target`, which is not focusable as rendered. It is made so
 * for as long as it holds focus and not left that way in the markup: the
 * attribute goes when focus leaves, or at once if focus did not land.
 */
function focusTarget(target: HTMLElement): void {
  const madeFocusable = !target.hasAttribute('tabindex');
  if (madeFocusable) target.setAttribute('tabindex', '-1');
  target.focus({ preventScroll: true });
  if (!madeFocusable) return;
  if (target.ownerDocument.activeElement === target) {
    target.addEventListener('blur', () => target.removeAttribute('tabindex'), { once: true });
  } else {
    target.removeAttribute('tabindex');
  }
}

/** The target has landed once it and the scroll have held still this long. */
const SETTLED_MS = 1000;
/** The longest a target is followed, however much keeps moving. */
const FOLLOW_LIMIT_MS = 5000;
/** A change in the resting position that is layout moving the target, not rounding. */
const SHIFT_PX = 1;
/** The reader scrolling, clicking or typing in the scroll box: the position is theirs again. */
const TAKEOVER_EVENTS = ['wheel', 'touchstart', 'pointerdown', 'keydown'] as const;
/** A move of the scroll position in one frame that is a scroll, not rounding. */
const STRAY_PX = 2;
/** How long a smooth scroll is left to get going before it is aimed again. */
const REAIM_GAP_MS = 100;
/**
 * The frames after an aim in which the scroll position can still be going
 * the old way: a smooth scroll that replaces one in flight takes effect a
 * frame late (seen in Chromium and WebKit), so the first frame shows the old
 * one carrying on. Two, to be clear of it.
 */
const AIM_LAG_FRAMES = 2;

/**
 * How much further from `to` the scroll position is at `top` than it was at
 * `from`, in px; negative when it has come nearer. Measured to the nearest
 * the box can get to `to`: a target near the end of the note rests short of
 * the top edge, and the end itself moves when the content's height does.
 */
function movedAway(from: number, top: number, to: number, scroller: HTMLElement): number {
  const reach = Math.min(to, scroller.scrollHeight - scroller.clientHeight);
  return Math.abs(top - reach) - Math.abs(from - reach);
}

/** The scroll boxes that are following a target, each with the way to stop. */
const following = new WeakMap<HTMLElement, () => void>();

/**
 * Keep `target` where the scroll was aimed while the layout above it is
 * still changing.
 *
 * The resting position is only right for the layout it was measured in. A
 * note's images load lazily, and one with no stored size takes up no room
 * until it has loaded, so an image the scroll passes on its way pushes the
 * target down after the aim was taken, and the scroll stops short of it.
 * Engines hold a resting scroll box steady when content above grows (scroll
 * anchoring), but not one in the middle of a smooth scroll, and not every
 * engine does it at all.
 *
 * So the resting position is taken again on each frame and, when it has
 * moved, the scroll is aimed at the new one. A smooth scroll starts slowly,
 * and one aimed again on every frame never gets going, so a smooth scroll is
 * aimed again no sooner than `REAIM_GAP_MS` after the last time: a target
 * that keeps moving is caught up with at the first frame after the gap.
 *
 * This ends when the target and the scroll have held still for
 * `SETTLED_MS`, when the reader takes over, when another target is followed
 * in the same box, when the target leaves the document, or after
 * `FOLLOW_LIMIT_MS`.
 *
 * The reader has taken over when they scroll, click or type in the box
 * (`TAKEOVER_EVENTS`), and when the box is scrolled by anything else, such
 * as the note's find bringing a match into view. That shows as the scroll
 * position moving away from where this loop sent it. The loop's own scroll
 * only ever nears its aim once it is under way (`AIM_LAG_FRAMES`), and the
 * engine holding the target in place as content above it grows (scroll
 * anchoring) moves the position away from the aim but towards where the
 * target now rests, so a move away from both is someone else's.
 */
function followTarget(
  target: HTMLElement,
  scroller: HTMLElement,
  margin: number,
  aim: number,
  behavior: ScrollBehavior
): void {
  following.get(scroller)?.();
  if (typeof requestAnimationFrame !== 'function') return;

  const started = performance.now();
  let stillSince = started;
  let aimedAt = started;
  let lag = AIM_LAG_FRAMES;
  let lastTop = scroller.scrollTop;
  let frame = 0;

  const stop = () => {
    cancelAnimationFrame(frame);
    for (const type of TAKEOVER_EVENTS) scroller.removeEventListener(type, stop, true);
    if (following.get(scroller) === stop) following.delete(scroller);
  };
  const check = () => {
    const now = performance.now();
    if (!scroller.contains(target) || !target.isConnected || now - started > FOLLOW_LIMIT_MS) {
      stop();
      return;
    }
    const top = scroller.scrollTop;
    const resting = restingScrollTop(target, scroller, margin);
    if (lag > 0) {
      lag -= 1;
    } else if (
      movedAway(lastTop, top, aim, scroller) > STRAY_PX &&
      movedAway(lastTop, top, resting, scroller) > STRAY_PX
    ) {
      stop();
      return;
    }
    if (Math.abs(resting - aim) > SHIFT_PX) {
      if (behavior !== 'smooth' || now - aimedAt >= REAIM_GAP_MS) {
        aim = resting;
        aimedAt = now;
        lag = AIM_LAG_FRAMES;
        scroller.scrollTo({ top: aim, behavior });
      }
      stillSince = now;
    } else if (top !== lastTop) {
      stillSince = now;
    } else if (now - stillSince >= SETTLED_MS) {
      stop();
      return;
    }
    lastTop = top;
    frame = requestAnimationFrame(check);
  };

  for (const type of TAKEOVER_EVENTS) {
    scroller.addEventListener(type, stop, { capture: true, passive: true });
  }
  following.set(scroller, stop);
  frame = requestAnimationFrame(check);
}

/**
 * Bring `target` (a heading or a named anchor) to the top of `scroller` and
 * move focus to it.
 *
 * Only `scroller` scrolls: `scrollIntoView` would also scroll every box
 * around it, up to the page. The target stops its `scroll-margin-top` below
 * the scroller's top edge, so a render site with something stuck over that
 * edge clears it in CSS. A target folded away in a `<details>` is unfolded
 * first, and the target is followed until the layout around it has settled
 * (see `followTarget`).
 *
 * Focus follows the scroll, as it does for a fragment link in a page, so
 * Tab and a screen reader carry on from the section rather than from the
 * link left behind.
 */
export function revealSectionTarget(target: HTMLElement, scroller: HTMLElement): void {
  if (!scroller.contains(target)) return;
  openEnclosingDetails(target, scroller);
  const margin = scrollMarginTop(target);
  const aim = restingScrollTop(target, scroller, margin);
  const behavior: ScrollBehavior = prefersReducedMotion() ? 'auto' : 'smooth';
  scroller.scrollTo({ top: aim, behavior });
  focusTarget(target);
  followTarget(target, scroller, margin, aim, behavior);
}

export interface SectionLinkScope {
  /** The element one document's markdown is rendered into: the link and its target are both in it. */
  content: Element | null | undefined;
  /** The box that scrolls that document. */
  scroller: HTMLElement | null | undefined;
}

/**
 * Follow a click on a section link inside `content`.
 *
 * Returns whether the click was one, so a delegated handler can stop there.
 * A section link's default is always prevented, matched or not: the default
 * is the webview navigating to the fragment, which finds no heading (they
 * have no ids), would scroll the page as well as the note for a named
 * anchor, and rewrites the app's URL. A fragment that names nothing in
 * `content` therefore does nothing.
 *
 * A link activated from the keyboard arrives here too, as the click Enter
 * raises on a focused link.
 */
export function followSectionLink(
  event: MouseEvent,
  { content, scroller }: SectionLinkScope
): boolean {
  if (!content || !(event.target instanceof Element)) return false;
  const anchor = event.target.closest('a[href]');
  if (!anchor || !content.contains(anchor)) return false;
  const fragment = sectionLinkFragment(anchor.getAttribute('href'));
  if (fragment === null) return false;

  event.preventDefault();
  const target = findSectionTarget(content, fragment);
  if (target && scroller) revealSectionTarget(target, scroller);
  return true;
}
