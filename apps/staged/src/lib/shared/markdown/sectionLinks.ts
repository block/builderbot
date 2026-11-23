/**
 * sectionLinks — links from rendered markdown to a heading in the same
 * document: `[difficult cases](#how-the-plan-behaves-in-difficult-cases)`.
 *
 * The rendered headings carry no ids. The sanitizer strips them, and an id
 * is document-wide while two notes can be on screen at once (a project note
 * with a child note opened over it) and share heading text. So a link is
 * resolved when it is followed, against the headings of the markdown it sits
 * in: their slugs are derived from their text by GitHub's rule
 * (headingSlug.ts) and the first match is scrolled to. Nothing outside that
 * markdown is looked at, and the webview never navigates.
 *
 * This is about `href`s. A `#note:<id>` / `#commit:<sha>` / `#review:<id>`
 * reference in prose is not a link at all: it renders as a badge
 * (`[data-hashtag-ref]`) with its own click handling, which render sites run
 * first. Written as a link target (`[x](#note:abc)`) it is a fragment like
 * any other and matches no heading, since a slug never holds a `:`.
 */

import { createHeadingSlugger } from './headingSlug';

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
 * The heading in `content` that `fragment` names, or null.
 *
 * Every heading is slugged in document order with one slugger, so repeated
 * heading text resolves as on GitHub: the first is `notes`, the next
 * `notes-1`. Slugs are lowercase, so the fragment is matched lowercased and
 * `#Ordered-Steps` finds the same heading as `#ordered-steps`.
 */
export function findSectionHeading(content: Element, fragment: string): HTMLElement | null {
  const wanted = fragment.toLowerCase();
  if (!wanted) return null;
  const slug = createHeadingSlugger();
  for (const heading of content.querySelectorAll<HTMLElement>(HEADINGS)) {
    if (slug(heading.textContent ?? '') === wanted) return heading;
  }
  return null;
}

function prefersReducedMotion(): boolean {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
}

/** The heading's `scroll-margin-top` in px: the room to leave above it. */
function scrollMarginTop(heading: HTMLElement): number {
  return parseFloat(getComputedStyle(heading).getPropertyValue('scroll-margin-top')) || 0;
}

/**
 * Bring `heading` to the top of `scroller` and move focus to it.
 *
 * Only `scroller` scrolls: `scrollIntoView` would also scroll every box
 * around it, up to the page. The heading stops its `scroll-margin-top`
 * below the scroller's top edge, so a render site with something stuck over
 * that edge clears it in CSS.
 *
 * Focus follows the scroll, as it does for a fragment link in a page, so
 * Tab and a screen reader carry on from the section rather than from the
 * link left behind. A heading is not focusable; it is made so for as long
 * as it holds focus and not left in the markup.
 */
export function revealSectionHeading(heading: HTMLElement, scroller: HTMLElement): void {
  if (!scroller.contains(heading)) return;
  const portTop = scroller.getBoundingClientRect().top + scroller.clientTop;
  const offset = heading.getBoundingClientRect().top - portTop - scrollMarginTop(heading);
  scroller.scrollTo({
    top: Math.max(scroller.scrollTop + offset, 0),
    behavior: prefersReducedMotion() ? 'auto' : 'smooth',
  });

  if (!heading.hasAttribute('tabindex')) {
    heading.setAttribute('tabindex', '-1');
    heading.addEventListener('blur', () => heading.removeAttribute('tabindex'), { once: true });
  }
  heading.focus({ preventScroll: true });
}

export interface SectionLinkScope {
  /** The element one document's markdown is rendered into: the link and its heading are both in it. */
  content: Element | null | undefined;
  /** The box that scrolls that document. */
  scroller: HTMLElement | null | undefined;
}

/**
 * Follow a click on a section link inside `content`.
 *
 * Returns whether the click was one, so a delegated handler can stop there.
 * A section link's default is always prevented, matched or not: the default
 * is the webview navigating to the fragment, which finds nothing (headings
 * have no ids) and only rewrites the app's URL. A fragment no heading
 * matches therefore does nothing.
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
  const heading = findSectionHeading(content, fragment);
  if (heading && scroller) revealSectionHeading(heading, scroller);
  return true;
}
