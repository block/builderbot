import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';

import { renderHashtagTokens } from '../../features/sessions/hashtagItems';
import type { HashtagItem } from '../../types';
import { renderMarkdown } from './renderMarkdown';
import {
  findNamedAnchor,
  findSectionHeading,
  followSectionLink,
  headingSlugText,
  sectionLinkFragment,
} from './sectionLinks';

// jsdom is only provided to `*.component.test.ts` files (see vitest.config.ts).
//
// jsdom lays nothing out and has no `Element.scrollTo`, so each note here is
// given its geometry by hand and a spy to scroll with. What these tests show
// is which heading a link resolves to, where the scroll is aimed, and that
// the page does not navigate. That the note visibly moves is not something
// jsdom can show, nor that Enter on a focused link raises the click.
//
// A link's target is followed for a moment after the click, frame by frame
// (see `followTarget`). The geometry given here does not change on its own,
// so outside the tests that move it by hand those frames find nothing to do.

/** Where the scroll box's top edge sits in the viewport. */
const PORT_TOP = 100;
/** The height of the scroll box. */
const PORT_HEIGHT = 500;

interface Note {
  scroller: HTMLElement;
  content: HTMLElement;
  scrollTo: Mock<(options: ScrollToOptions) => void>;
  /** Set how far the box can be scrolled: the `scrollTop` at the end of the note. */
  endsAt(scrollTop: number): void;
  /** What `followSectionLink` returned for each click, and whether it prevented the default. */
  clicks: { handled: boolean; defaultPrevented: boolean }[];
  link(href: string): HTMLAnchorElement;
  heading(text: string): HTMLElement;
}

function at(el: Element, top: number) {
  vi.spyOn(el, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, top, 600, 24));
}

/**
 * A note as a viewer mounts it: a scroll box around the element the markdown
 * is rendered into, with a delegated click handler on the box. Headings are
 * laid out 400px apart, the first at the top of the box, in a note long
 * enough that no scroll here comes to its end unless `endsAt` shortens it.
 */
function mountNote(markdown: string, html = renderMarkdown(markdown)): Note {
  const scroller = document.createElement('div');
  const content = document.createElement('div');
  content.className = 'markdown-content';
  content.innerHTML = html;
  scroller.append(content);
  document.body.append(scroller);

  at(scroller, PORT_TOP);
  const endsAt = (scrollTop: number) =>
    Object.defineProperty(scroller, 'scrollHeight', {
      configurable: true,
      value: scrollTop + PORT_HEIGHT,
    });
  Object.defineProperty(scroller, 'clientHeight', { configurable: true, value: PORT_HEIGHT });
  endsAt(100_000);
  content
    .querySelectorAll('h1, h2, h3, h4, h5, h6')
    .forEach((heading, i) => at(heading, PORT_TOP + i * 400));

  const scrollTo = vi.fn<(options: ScrollToOptions) => void>();
  scroller.scrollTo = scrollTo as unknown as Element['scrollTo'];

  const clicks: Note['clicks'] = [];
  scroller.addEventListener('click', (event) => {
    const handled = followSectionLink(event, { content, scroller });
    clicks.push({ handled, defaultPrevented: event.defaultPrevented });
    // jsdom cannot load a page; a link left to its default would only log that.
    if (!handled) event.preventDefault();
  });

  return {
    scroller,
    content,
    scrollTo,
    endsAt,
    clicks,
    link(href) {
      const link = [...content.querySelectorAll('a')].find((a) => a.getAttribute('href') === href);
      if (!link) throw new Error(`no link to ${href} in ${content.innerHTML}`);
      return link;
    },
    heading(text) {
      const heading = [...content.querySelectorAll<HTMLElement>('h1, h2, h3, h4, h5, h6')].find(
        (h) => h.textContent === text
      );
      if (!heading) throw new Error(`no heading ${text} in ${content.innerHTML}`);
      return heading;
    },
  };
}

/** The heading a click on `href` resolved to: the one focus moved to. */
function followed(note: Note, href: string): Element | null {
  note.link(href).click();
  return document.activeElement === document.body ? null : document.activeElement;
}

/**
 * jsdom follows a fragment link in a task after the click, not during it, so
 * `location.hash` says nothing until that task has had its turn.
 */
const navigationSettled = () => new Promise((resolve) => setTimeout(resolve, 20));

beforeEach(() => {
  document.body.innerHTML = '';
  window.location.hash = '';
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('sectionLinkFragment', () => {
  it('reads a target that is only a fragment', () => {
    expect(sectionLinkFragment('#ordered-steps')).toBe('ordered-steps');
    expect(sectionLinkFragment('#')).toBe('');
  });

  it('decodes a percent-encoded fragment, and takes a malformed one as written', () => {
    expect(sectionLinkFragment('#%C3%BCber-caf%C3%A9')).toBe('über-café');
    expect(sectionLinkFragment('#100%-done')).toBe('100%-done');
  });

  it.each([
    'https://example.com/page#ordered-steps',
    'http://example.com',
    'buzz://channel/123',
    'mailto:someone@example.com',
    'other-note.md#ordered-steps',
    '/docs#ordered-steps',
    '',
    null,
    undefined,
  ])('is not a section link: %j', (href) => {
    expect(sectionLinkFragment(href)).toBeNull();
  });
});

describe('followSectionLink', () => {
  const NOTE = [
    '# Plan',
    '',
    'Start with the [difficult cases section](#how-the-plan-behaves-in-difficult-cases).',
    '',
    '## Ordered steps',
    '',
    '| Step | See |',
    '| --- | --- |',
    '| Mute | [silencing](#silencing-this-apps-own-alerts) |',
    '| Check | [rule 22](#need-in-both-directions-rule-22) |',
    '',
    '## How the plan behaves in difficult cases',
    '',
    'Back to the [steps](#ordered-steps).',
    '',
    "### Silencing this app's own alerts",
    '',
    '### Need in both directions (rule 22)',
  ].join('\n');

  it('scrolls the note to a later heading and moves focus to it', async () => {
    const note = mountNote(NOTE);
    const target = note.heading('How the plan behaves in difficult cases');
    note.scroller.scrollTop = 30;

    note.link('#how-the-plan-behaves-in-difficult-cases').click();

    expect(note.clicks).toEqual([{ handled: true, defaultPrevented: true }]);
    // Third heading: 800px below the box's top edge, in a box already scrolled by 30.
    expect(note.scrollTo.mock.calls).toEqual([[{ top: 830, behavior: 'smooth' }]]);
    expect(document.activeElement).toBe(target);
    await navigationSettled();
    expect(window.location.hash).toBe('');
  });

  it('scrolls back to an earlier heading', () => {
    const note = mountNote(NOTE);
    // Scrolled so the third heading is at the top: the second is 400px above it.
    note.scroller.scrollTop = 800;
    note.content
      .querySelectorAll('h1, h2, h3')
      .forEach((heading, i) => at(heading, PORT_TOP + i * 400 - 800));

    note.link('#ordered-steps').click();

    expect(note.scrollTo.mock.calls).toEqual([[{ top: 400, behavior: 'smooth' }]]);
    expect(document.activeElement).toBe(note.heading('Ordered steps'));
  });

  it('follows links inside table cells, to ### headings', () => {
    const note = mountNote(NOTE);
    expect(note.link('#silencing-this-apps-own-alerts').closest('td')).not.toBeNull();

    expect(followed(note, '#silencing-this-apps-own-alerts')).toBe(
      note.heading("Silencing this app's own alerts")
    );
    expect(followed(note, '#need-in-both-directions-rule-22')).toBe(
      note.heading('Need in both directions (rule 22)')
    );
    expect(note.scrollTo.mock.calls.map(([options]) => options.top)).toEqual([1200, 1600]);
    expect(note.clicks.every((click) => click.handled && click.defaultPrevented)).toBe(true);
  });

  it('resolves a link by the element clicked inside it', () => {
    const note = mountNote('See [the **ordered** `steps`](#ordered-steps).\n\n## Ordered steps');

    note.link('#ordered-steps').querySelector('code')!.click();

    expect(document.activeElement).toBe(note.heading('Ordered steps'));
  });

  it('matches the fragment whatever its case', () => {
    const note = mountNote('[steps](#Ordered-Steps)\n\n## Ordered steps');
    expect(followed(note, '#Ordered-Steps')).toBe(note.heading('Ordered steps'));
  });

  it('lowercases the fragment as a slug is: a capital sigma that ends a word is σ', () => {
    const note = mountNote('[plan](#plan) [road](#ΟΔΟΣ)\n\n# Plan\n\n## ΟΔΟΣ');
    const href = note.content.querySelectorAll('a')[1].getAttribute('href')!;
    expect(decodeURIComponent(href)).toBe('#ΟΔΟΣ');

    expect(followed(note, href)).toBe(note.heading('ΟΔΟΣ'));
  });

  it('numbers repeated headings as GitHub does, the title heading included', () => {
    const note = mountNote(
      [
        '# Notes',
        '[first](#notes) [second](#notes-1) [third](#notes-2) [none](#notes-3)',
        '## Notes',
        'under the second',
        '### Notes',
      ].join('\n\n')
    );
    const [h1, h2, h3] = note.content.querySelectorAll('h1, h2, h3');

    expect(followed(note, '#notes')).toBe(h1);
    expect(followed(note, '#notes-1')).toBe(h2);
    expect(followed(note, '#notes-2')).toBe(h3);
    expect(note.scrollTo).toHaveBeenCalledTimes(3);

    // One past the last: nothing to go to, so focus and scroll stay put.
    expect(followed(note, '#notes-3')).toBe(h3);
    expect(note.scrollTo).toHaveBeenCalledTimes(3);
  });

  it('slugs a heading by its rendered text: inline code, emphasis and links', () => {
    const note = mountNote(
      [
        '[a](#calling-write_note-twice) [b](#the-really-long-way) [c](#see-the-docs)',
        '## Calling `write_note()` twice',
        '## The *really* **long** way',
        '## See [the docs](https://example.com)',
      ].join('\n\n')
    );

    expect(followed(note, '#calling-write_note-twice')).toBe(
      note.heading('Calling write_note() twice')
    );
    expect(followed(note, '#the-really-long-way')).toBe(note.heading('The really long way'));
    expect(followed(note, '#see-the-docs')).toBe(note.heading('See the docs'));
  });

  it('follows links to non-ASCII and emoji headings, which the renderer percent-encodes', () => {
    const note = mountNote(
      [
        '[a](#über-café) [b](#日本語の-見出し) [c](#-launch-plan)',
        '## Über Café',
        '## 日本語の 見出し',
        '## 🚀 Launch plan',
      ].join('\n\n')
    );
    const hrefs = [...note.content.querySelectorAll('a')].map((a) => a.getAttribute('href')!);
    expect(hrefs[0]).toBe('#%C3%BCber-caf%C3%A9');

    expect(followed(note, hrefs[0])).toBe(note.heading('Über Café'));
    expect(followed(note, hrefs[1])).toBe(note.heading('日本語の 見出し'));
    expect(followed(note, hrefs[2])).toBe(note.heading('🚀 Launch plan'));
  });

  it('resolves each link in its own note when two on screen share a heading', () => {
    const parent = mountNote('# Parent\n\n[steps](#ordered-steps)\n\n## Ordered steps');
    const child = mountNote('# Child\n\n## Intro\n\n[steps](#ordered-steps)\n\n## Ordered steps');

    child.link('#ordered-steps').click();
    expect(document.activeElement).toBe(child.heading('Ordered steps'));
    expect(child.scrollTo.mock.calls).toEqual([[{ top: 800, behavior: 'smooth' }]]);
    expect(parent.scrollTo).not.toHaveBeenCalled();

    parent.link('#ordered-steps').click();
    expect(document.activeElement).toBe(parent.heading('Ordered steps'));
    expect(parent.scrollTo.mock.calls).toEqual([[{ top: 400, behavior: 'smooth' }]]);
    expect(child.scrollTo).toHaveBeenCalledTimes(1);
  });

  it('does not look outside its own note for a heading only the other has', () => {
    const parent = mountNote('[child section](#only-in-child)\n\n## Parent section');
    const child = mountNote('## Only in child');

    parent.link('#only-in-child').click();

    expect(parent.clicks).toEqual([{ handled: true, defaultPrevented: true }]);
    expect(parent.scrollTo).not.toHaveBeenCalled();
    expect(child.scrollTo).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(document.body);
  });

  it.each(['#no-such-section', '#', '#note:abc123', '#100%-done'])(
    'does nothing for %j, which matches no heading, and does not navigate',
    async (href) => {
      const note = mountNote(`[link](${href})\n\n## Ordered steps`);

      expect(() => note.link(href).click()).not.toThrow();

      expect(note.clicks).toEqual([{ handled: true, defaultPrevented: true }]);
      expect(note.scrollTo).not.toHaveBeenCalled();
      expect(document.activeElement).toBe(document.body);
      await navigationSettled();
      expect(window.location.hash).toBe('');
    }
  );

  it('would navigate the page if the click were left to its default', async () => {
    // The control for the `location.hash` assertions above: jsdom does follow
    // a fragment link nothing has prevented.
    document.body.innerHTML = '<a href="#ordered-steps">steps</a>';
    document.querySelector('a')!.click();
    await navigationSettled();
    expect(window.location.hash).toBe('#ordered-steps');
  });

  it.each([
    'https://example.com/page#ordered-steps',
    'http://example.com/',
    'mailto:someone@example.com',
    'other-note.md#ordered-steps',
  ])('leaves %j to the other link handling', (href) => {
    const note = mountNote(`[link](${href})\n\n## Ordered steps`);

    note.link(href).click();

    expect(note.clicks).toEqual([{ handled: false, defaultPrevented: false }]);
    expect(note.scrollTo).not.toHaveBeenCalled();
  });

  it('leaves a hashtag reference alone: it renders as a badge, not a link', () => {
    const markdown = 'See #note:abc123 and #commit:deadbeef.\n\n## Ordered steps';
    const note = mountNote(
      markdown,
      renderMarkdown(markdown, { renderInlineText: (text) => renderHashtagTokens(text, []) })
    );
    const badges = note.content.querySelectorAll<HTMLElement>('[data-hashtag-ref]');
    expect([...badges].map((badge) => badge.dataset.hashtagRef)).toEqual([
      '#note:abc123',
      '#commit:deadbeef',
    ]);
    expect(note.content.querySelector('a')).toBeNull();

    badges[0].click();

    expect(note.clicks).toEqual([{ handled: false, defaultPrevented: false }]);
  });

  it('ignores clicks that are not on a link, and links outside the note', () => {
    const note = mountNote('Plain text.\n\n## Ordered steps');
    const outside = document.createElement('a');
    outside.setAttribute('href', '#ordered-steps');
    note.scroller.append(outside);

    note.content.querySelector('p')!.click();
    outside.click();

    expect(note.clicks.map((click) => click.handled)).toEqual([false, false]);
    expect(note.scrollTo).not.toHaveBeenCalled();
  });

  it('is not handled before the note has rendered', () => {
    const link = document.createElement('a');
    link.setAttribute('href', '#ordered-steps');
    document.body.append(link);
    const event = new MouseEvent('click', { bubbles: true, cancelable: true });
    link.addEventListener('click', (e) => e.preventDefault());
    let handled: boolean | null = null;
    document.body.addEventListener(
      'click',
      (e) => (handled = followSectionLink(e, { content: undefined, scroller: undefined })),
      { once: true }
    );

    link.dispatchEvent(event);

    expect(handled).toBe(false);
  });

  it('leaves the room the heading asks for above it', () => {
    const style = document.createElement('style');
    style.textContent = '.markdown-content h2 { scroll-margin-top: 16px; }';
    document.head.append(style);
    try {
      const note = mountNote('[steps](#ordered-steps)\n\n# Plan\n\n## Ordered steps');
      note.link('#ordered-steps').click();
      expect(note.scrollTo.mock.calls).toEqual([[{ top: 384, behavior: 'smooth' }]]);
    } finally {
      style.remove();
    }
  });

  it('never aims above the top of the note', () => {
    const style = document.createElement('style');
    style.textContent = '.markdown-content h1 { scroll-margin-top: 16px; }';
    document.head.append(style);
    try {
      const note = mountNote('# Plan\n\n[top](#plan)');
      note.link('#plan').click();
      expect(note.scrollTo.mock.calls).toEqual([[{ top: 0, behavior: 'smooth' }]]);
    } finally {
      style.remove();
    }
  });

  it('jumps rather than glides when the reader asked for reduced motion', () => {
    vi.stubGlobal(
      'matchMedia',
      vi.fn((query: string) => ({ matches: query === '(prefers-reduced-motion: reduce)' }))
    );
    const note = mountNote('[steps](#ordered-steps)\n\n## Ordered steps');

    note.link('#ordered-steps').click();

    expect(note.scrollTo.mock.calls).toEqual([[{ top: 0, behavior: 'auto' }]]);
  });

  it('makes the heading focusable only while it holds focus', () => {
    const note = mountNote('[steps](#ordered-steps)\n\n## Ordered steps\n\n## Later');
    const heading = note.heading('Ordered steps');
    const focus = vi.spyOn(heading, 'focus');

    note.link('#ordered-steps').click();

    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
    expect(heading.getAttribute('tabindex')).toBe('-1');

    heading.blur();
    expect(heading.hasAttribute('tabindex')).toBe(false);

    // And again, after it was cleaned up.
    note.link('#ordered-steps').click();
    expect(document.activeElement).toBe(heading);
    expect(heading.getAttribute('tabindex')).toBe('-1');
  });
});

describe('headings the viewer draws differently from their source', () => {
  const COMMIT_ITEM: HashtagItem = {
    type: 'commit',
    id: '0a1b2c3d',
    title: 'Peel the mango',
    color: '--commit-color',
    bgColor: '--commit-bg',
  };
  const MARKDOWN = '[fix](#mango-commit0a1b2c3d)\n\n## Intro\n\n## Mango #commit:0a1b2c3d';

  // The same heading in the three states a viewer can render it in. The
  // slug is the one GitHub gives the source, `mango-commit0a1b2c3d`.
  it.each<[string, HashtagItem[] | null, string]>([
    ['before any reference has loaded', null, 'Mango #commit:0a1b2c3d'],
    ['when the reference is unknown', [], 'Mango 0a1b2c3d'],
    ['when the reference shows its title', [COMMIT_ITEM], 'Mango Peel the mango'],
  ])('slugs a hashtag reference in a heading as it was written, %s', (_state, items, shown) => {
    const note = mountNote(
      MARKDOWN,
      renderMarkdown(
        MARKDOWN,
        items ? { renderInlineText: (text) => renderHashtagTokens(text, items) } : {}
      )
    );
    const heading = note.content.querySelectorAll('h2')[1];
    expect(heading.textContent).toBe(shown);
    expect(headingSlugText(heading)).toBe('Mango #commit:0a1b2c3d');

    expect(followed(note, '#mango-commit0a1b2c3d')).toBe(heading);
  });

  it('does not resolve a link written from the title a reference happens to show', () => {
    const markdown = '[fix](#mango-peel-the-mango)\n\n## Mango #commit:0a1b2c3d';
    const note = mountNote(
      markdown,
      renderMarkdown(markdown, {
        renderInlineText: (text) => renderHashtagTokens(text, [COMMIT_ITEM]),
      })
    );

    note.link('#mango-peel-the-mango').click();

    expect(note.scrollTo).not.toHaveBeenCalled();
  });

  it('slugs a written-out entity as the viewer shows it, which differs once references are drawn', () => {
    // GitHub gives `quince--lime`. Not matched with the reference hook on:
    // that hook shows `&amp;` as written (a rendering quirk older than
    // section links), and a slug that disagreed with the visible heading
    // would be the worse surprise.
    const markdown = '[a](#quince--lime) [b](#quince-amp-lime)\n\n## Quince &amp; Lime';
    const plain = mountNote(markdown);
    const drawn = mountNote(
      markdown,
      renderMarkdown(markdown, { renderInlineText: (text) => renderHashtagTokens(text, []) })
    );
    expect(plain.content.querySelector('h2')!.textContent).toBe('Quince & Lime');
    expect(drawn.content.querySelector('h2')!.textContent).toBe('Quince &amp; Lime');

    expect(followed(plain, '#quince--lime')).toBe(plain.content.querySelector('h2'));
    expect(followed(drawn, '#quince-amp-lime')).toBe(drawn.content.querySelector('h2'));
  });

  it('gives an image in a heading no text, keeping the hyphen of the space beside it', () => {
    // GitHub: `## ![pelican](…) Heron` is `-heron`, `## Ibis ![pelican](…)` is `ibis-`.
    const media = 'staged-media://123e4567-e89b-42d3-a456-426614174000.png';
    const note = mountNote(
      [
        '[a](#-heron) [b](#ibis-) [c](#-kiwi)',
        '## ![pelican](https://example.com/p.png) Heron',
        '## Ibis ![pelican](https://example.com/p.png)',
        // An embedded image is drawn as a figure whose caption repeats the alt text.
        `## ![pelican](${media}) Kiwi`,
      ].join('\n\n')
    );
    const [heron, ibis, kiwi] = note.content.querySelectorAll('h2');
    expect(kiwi.querySelector('figure.markdown-media figcaption')?.textContent).toBe('pelican');

    expect(followed(note, '#-heron')).toBe(heron);
    expect(followed(note, '#ibis-')).toBe(ibis);
    expect(followed(note, '#-kiwi')).toBe(kiwi);
  });
});

describe('named anchors', () => {
  const NOTE = [
    '[manual](#manual)',
    '## Intro',
    'Before <a name="manual"></a>the manual.',
    '## Later',
  ].join('\n\n');

  it('falls back to an anchor the author named when no heading matches', async () => {
    const note = mountNote(NOTE);
    const anchor = note.content.querySelector<HTMLElement>('a[name="manual"]')!;
    at(anchor, PORT_TOP + 250);

    note.link('#manual').click();

    expect(note.clicks).toEqual([{ handled: true, defaultPrevented: true }]);
    expect(note.scrollTo.mock.calls).toEqual([[{ top: 250, behavior: 'smooth' }]]);
    expect(document.activeElement).toBe(anchor);
    await navigationSettled();
    expect(window.location.hash).toBe('');

    anchor.blur();
    expect(anchor.hasAttribute('tabindex')).toBe(false);
  });

  it('prefers a heading to an anchor of the same name', () => {
    const note = mountNote(
      '[steps](#ordered-steps)\n\n<a name="ordered-steps"></a>\n\n## Ordered steps'
    );
    expect(note.content.querySelector('a[name="ordered-steps"]')).not.toBeNull();

    expect(followed(note, '#ordered-steps')).toBe(note.heading('Ordered steps'));
  });

  it("matches an anchor's name exactly, and decodes the fragment first", () => {
    const note = mountNote(
      [
        '[a](#Manual) [b](#réglages) [c](#a"]b)',
        '<a name="manual"></a>',
        '<a name="réglages"></a>',
        "<a name='a\"]b'></a>",
      ].join('\n\n')
    );
    const named = (name: string) =>
      [...note.content.querySelectorAll('a[name]')].find((a) => a.getAttribute('name') === name);
    const hrefs = [...note.content.querySelectorAll('a[href]')].map((a) => a.getAttribute('href')!);
    expect(hrefs[1]).toBe('#r%C3%A9glages');

    note.link(hrefs[0]).click();
    expect(note.scrollTo).not.toHaveBeenCalled();
    expect(followed(note, hrefs[1])).toBe(named('réglages'));
    // A name that would break out of a selector is only ever compared.
    expect(followed(note, hrefs[2])).toBe(named('a"]b'));
  });

  it('does not look in another note for the anchor', () => {
    const parent = mountNote('[manual](#manual)\n\n## Parent section');
    const child = mountNote(NOTE);

    parent.link('#manual').click();

    expect(parent.clicks).toEqual([{ handled: true, defaultPrevented: true }]);
    expect(parent.scrollTo).not.toHaveBeenCalled();
    expect(child.scrollTo).not.toHaveBeenCalled();
    expect(findNamedAnchor(child.content, 'manual')).not.toBeNull();
    expect(findNamedAnchor(child.content, '')).toBeNull();
  });
});

describe('a target folded away in <details>', () => {
  const NOTE = [
    '[hidden](#hidden-part) [more](#more)',
    '## Intro',
    '<details><summary>More <a name="more"></a></summary>',
    '<details><summary>Deeper</summary>',
    '## Hidden part',
    '</details>',
    '</details>',
    '<details><summary>Unrelated</summary>',
    'Other.',
    '</details>',
  ].join('\n\n');

  it('opens every block around the target before scrolling to it', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Hidden part');
    const [outer, inner, unrelated] = note.content.querySelectorAll('details');
    expect(inner.contains(target) && outer.contains(inner)).toBe(true);
    expect([outer.open, inner.open, unrelated.open]).toEqual([false, false, false]);
    // Where the heading is only once the blocks around it are open.
    const rect = vi.spyOn(target, 'getBoundingClientRect');
    rect.mockImplementation(() => new DOMRect(0, outer.open && inner.open ? 700 : 0, 600, 24));

    note.link('#hidden-part').click();

    expect([outer.open, inner.open, unrelated.open]).toEqual([true, true, false]);
    expect(note.scrollTo.mock.calls).toEqual([[{ top: 600, behavior: 'smooth' }]]);
    expect(document.activeElement).toBe(target);
  });

  it("leaves a block closed when the target is in the block's own summary", () => {
    const note = mountNote(NOTE);
    const anchor = note.content.querySelector<HTMLElement>('a[name="more"]')!;
    const outer = note.content.querySelector('details')!;

    expect(followed(note, '#more')).toBe(anchor);

    expect(outer.open).toBe(false);
  });

  it('does not leave the target focusable when focus did not land on it', () => {
    const note = mountNote('[steps](#ordered-steps)\n\n## Ordered steps');
    const heading = note.heading('Ordered steps');
    // What an engine does for an element it cannot focus: nothing.
    vi.spyOn(heading, 'focus').mockImplementation(() => {});

    note.link('#ordered-steps').click();

    expect(document.activeElement).toBe(document.body);
    expect(heading.hasAttribute('tabindex')).toBe(false);
  });

  it('keeps a tabindex the target already had', () => {
    const note = mountNote('[steps](#ordered-steps)\n\n## Ordered steps');
    const heading = note.heading('Ordered steps');
    heading.setAttribute('tabindex', '0');

    note.link('#ordered-steps').click();
    heading.blur();

    expect(heading.getAttribute('tabindex')).toBe('0');
  });
});

describe('a target that moves after the scroll was aimed', () => {
  const NOTE = '[steps](#ordered-steps) [plan](#plan)\n\n# Plan\n\n## Ordered steps';
  const SMOOTH = 'smooth';

  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  const frame = () => vi.advanceTimersToNextFrame();

  /**
   * The geometry part-way through or after a scroll: the box scrolled to
   * `scrollTop`, with `grown` px of content having appeared above `target`,
   * which lies `at` px down the note.
   */
  function layout(note: Note, target: Element, laidOutAt: number, scrollTop: number, grown = 0) {
    note.scroller.scrollTop = scrollTop;
    at(target, PORT_TOP + laidOutAt + grown - scrollTop);
  }

  it('aims again when content above the target grows during the scroll', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');

    note.link('#ordered-steps').click();
    expect(note.scrollTo.mock.calls).toEqual([[{ top: 400, behavior: SMOOTH }]]);

    // Part-way through the scroll: nothing else has moved.
    layout(note, target, 400, 150);
    vi.advanceTimersByTime(200);
    expect(note.scrollTo).toHaveBeenCalledTimes(1);

    // An image above the heading loads and takes up 300px.
    layout(note, target, 400, 180, 300);
    frame();
    expect(note.scrollTo.mock.calls).toEqual([
      [{ top: 400, behavior: SMOOTH }],
      [{ top: 700, behavior: SMOOTH }],
    ]);

    // The scroll carries on to the new place and rests there.
    layout(note, target, 400, 450, 300);
    frame();
    layout(note, target, 400, 700, 300);
    frame();
    frame();
    expect(note.scrollTo).toHaveBeenCalledTimes(2);
  });

  it('aims again when the target moves just after the scroll has come to rest', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    note.link('#ordered-steps').click();
    layout(note, target, 400, 400);
    vi.advanceTimersByTime(500);

    layout(note, target, 400, 400, 120);
    frame();

    expect(note.scrollTo.mock.calls.at(-1)).toEqual([{ top: 520, behavior: SMOOTH }]);
    expect(note.scrollTo).toHaveBeenCalledTimes(2);
  });

  it('does nothing more when the engine has already kept the target in place', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    note.link('#ordered-steps').click();
    layout(note, target, 400, 400);
    vi.advanceTimersByTime(300);

    // Scroll anchoring: the content grew and the box was scrolled with it.
    layout(note, target, 400, 700, 300);
    frame();

    // Told where it already is, which moves nothing.
    expect(note.scrollTo.mock.calls.at(-1)).toEqual([{ top: 700, behavior: SMOOTH }]);
    frame();
    expect(note.scrollTo).toHaveBeenCalledTimes(2);
  });

  it.each(['wheel', 'touchstart', 'pointerdown', 'keydown'])(
    'leaves the position to the reader after a %s in the note',
    (type) => {
      const note = mountNote(NOTE);
      const target = note.heading('Ordered steps');
      note.link('#ordered-steps').click();
      layout(note, target, 400, 150);
      vi.advanceTimersByTime(200);

      // The event alone: the scroll position has not moved when the target does.
      note.content.querySelector('p')!.dispatchEvent(new Event(type, { bubbles: true }));
      layout(note, target, 400, 150, 300);
      vi.advanceTimersByTime(200);

      expect(note.scrollTo).toHaveBeenCalledTimes(1);
    }
  );

  it('leaves the position alone after a scroll that raised no event in the note', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    note.link('#ordered-steps').click();
    layout(note, target, 400, 400);
    vi.advanceTimersByTime(300);

    // The note's find glides a match into view: the box sees no input for it.
    layout(note, target, 400, 330);
    frame();
    expect(vi.getTimerCount()).toBe(0);
    layout(note, target, 400, 150);
    frame();

    // Then an image between the match and the heading loads.
    layout(note, target, 400, 150, 300);
    vi.advanceTimersByTime(300);

    expect(note.scrollTo).toHaveBeenCalledTimes(1);
  });

  it('leaves the position alone when such a scroll takes over part-way through the glide', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    note.link('#ordered-steps').click();
    layout(note, target, 400, 150);
    vi.advanceTimersByTime(200);

    layout(note, target, 400, 90);
    frame();
    layout(note, target, 400, 90, 300);
    vi.advanceTimersByTime(300);

    expect(note.scrollTo).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('does not take the scroll stopping at the end of the note for the reader', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    // The heading lies 400px down a note that scrolls no further than 250.
    note.endsAt(250);
    note.link('#ordered-steps').click();
    layout(note, target, 400, 250);
    vi.advanceTimersByTime(300);

    // Content below the heading goes, and the engine pulls the scroll back with the end.
    note.endsAt(200);
    layout(note, target, 400, 200);
    frame();

    // Still followed: content above the heading grows.
    note.endsAt(500);
    layout(note, target, 400, 200, 300);
    frame();

    expect(note.scrollTo.mock.calls.at(-1)).toEqual([{ top: 700, behavior: SMOOTH }]);
  });

  it('lets a glide get going before aiming it again, however often the target moves', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    note.link('#ordered-steps').click();

    // Half a second in which every frame finds the target 10px further down.
    for (let i = 1; i <= 30; i++) {
      layout(note, target, 400, i * 5, i * 10);
      frame();
    }
    // Not an aim a frame, which would be 31: one for each tenth of a second.
    expect(note.scrollTo.mock.calls.length).toBeGreaterThan(3);
    expect(note.scrollTo.mock.calls.length).toBeLessThan(8);

    // The target stops moving, and the next aim is where it stopped.
    vi.advanceTimersByTime(150);
    expect(note.scrollTo.mock.calls.at(-1)).toEqual([{ top: 700, behavior: SMOOTH }]);
  });

  it('does not take the engine keeping the target in place between two aims for the reader', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    note.link('#ordered-steps').click();
    layout(note, target, 400, 400);
    vi.advanceTimersByTime(300);

    // Content above grows frame after frame, and the engine scrolls the box with it.
    layout(note, target, 400, 450, 50);
    frame();
    expect(note.scrollTo.mock.calls.at(-1)).toEqual([{ top: 450, behavior: SMOOTH }]);
    for (let grown = 100; grown <= 300; grown += 50) {
      layout(note, target, 400, 400 + grown, grown);
      frame();
    }
    expect(note.scrollTo).toHaveBeenCalledTimes(2);

    vi.advanceTimersByTime(150);
    expect(note.scrollTo.mock.calls.at(-1)).toEqual([{ top: 700, behavior: SMOOTH }]);
    expect(note.scrollTo).toHaveBeenCalledTimes(3);
  });

  it('does not take the glide it replaced, carrying on for a frame, for the reader', () => {
    const note = mountNote(NOTE);
    const plan = note.heading('Plan');
    note.link('#ordered-steps').click();
    layout(note, note.heading('Ordered steps'), 400, 150);
    vi.advanceTimersByTime(200);

    // Back up to the first heading, while the glide down is still running.
    layout(note, plan, 0, 150);
    note.link('#plan').click();
    expect(note.scrollTo.mock.calls.at(-1)).toEqual([{ top: 0, behavior: SMOOTH }]);
    // The engine turns a frame late: first further down, then back up.
    layout(note, plan, 0, 190);
    frame();
    layout(note, plan, 0, 170);
    frame();
    layout(note, plan, 0, 120);
    frame();

    // Still followed: content above the heading grows.
    layout(note, plan, 0, 60, 80);
    vi.advanceTimersByTime(150);
    expect(note.scrollTo.mock.calls.at(-1)).toEqual([{ top: 80, behavior: SMOOTH }]);
  });

  it('nor its own glide when the target moves behind it, before or after it is aimed back', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    note.link('#ordered-steps').click();
    layout(note, target, 400, 300);
    vi.advanceTimersByTime(200);

    // Content above the heading goes while the glide moves on: the heading now
    // rests behind it, and the glide is still nearing where it was sent.
    layout(note, target, 400, 320, -300);
    frame();
    expect(note.scrollTo.mock.calls.at(-1)).toEqual([{ top: 100, behavior: SMOOTH }]);
    layout(note, target, 400, 340, -300);
    frame();
    layout(note, target, 400, 310, -300);
    frame();
    layout(note, target, 400, 250, -300);
    frame();

    // Still followed.
    layout(note, target, 400, 250, -200);
    vi.advanceTimersByTime(150);
    expect(note.scrollTo.mock.calls.at(-1)).toEqual([{ top: 200, behavior: SMOOTH }]);
  });

  it('stops watching once the target has held still for a second', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    note.link('#ordered-steps').click();
    layout(note, target, 400, 400);
    vi.advanceTimersByTime(900);

    // Still watching: the scroll only came to rest at the first frame.
    layout(note, target, 400, 400, 50);
    frame();
    expect(note.scrollTo).toHaveBeenCalledTimes(2);

    layout(note, target, 400, 450, 50);
    vi.advanceTimersByTime(1200);
    layout(note, target, 400, 450, 350);
    vi.advanceTimersByTime(200);

    expect(note.scrollTo).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('keeps watching for as long as the scroll is still moving', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    note.link('#ordered-steps').click();

    // A glide of 1.6 seconds, with nothing else moving.
    for (let top = 10; top <= 400; top += 10) {
      layout(note, target, 400, top);
      vi.advanceTimersByTime(40);
    }
    expect(note.scrollTo).toHaveBeenCalledTimes(1);

    layout(note, target, 400, 400, 200);
    frame();

    expect(note.scrollTo.mock.calls.at(-1)).toEqual([{ top: 600, behavior: SMOOTH }]);
  });

  it('gives up after five seconds, however much keeps moving', () => {
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    note.link('#ordered-steps').click();

    for (let step = 1; step <= 40; step++) {
      layout(note, target, 400, 0, step * 10);
      vi.advanceTimersByTime(200);
    }

    // One aim at the click, then one for each move in the first five seconds.
    expect(note.scrollTo.mock.calls.length).toBeGreaterThan(22);
    expect(note.scrollTo.mock.calls.length).toBeLessThan(28);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('follows only the target of the latest link', () => {
    const note = mountNote(NOTE);
    const first = note.heading('Ordered steps');
    note.link('#ordered-steps').click();
    note.link('#plan').click();
    expect(note.scrollTo.mock.calls.map(([options]) => options.top)).toEqual([400, 0]);

    at(first, PORT_TOP + 900);
    frame();
    frame();

    expect(note.scrollTo).toHaveBeenCalledTimes(2);
  });

  it('stops when the note is rendered again and the target is gone', () => {
    const note = mountNote(NOTE);
    note.link('#ordered-steps').click();

    note.content.innerHTML = '';
    expect(() => frame()).not.toThrow();

    expect(note.scrollTo).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('stops when the note is closed and its scroll box leaves the document', () => {
    const note = mountNote(NOTE);
    note.link('#ordered-steps').click();

    // The box still holds the target; neither is in the document.
    note.scroller.remove();
    expect(() => frame()).not.toThrow();

    expect(note.scrollTo).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('jumps to the new place when the reader asked for reduced motion', () => {
    vi.stubGlobal(
      'matchMedia',
      vi.fn((query: string) => ({ matches: query === '(prefers-reduced-motion: reduce)' }))
    );
    const note = mountNote(NOTE);
    const target = note.heading('Ordered steps');
    note.link('#ordered-steps').click();

    layout(note, target, 400, 400, 300);
    frame();

    expect(note.scrollTo.mock.calls).toEqual([
      [{ top: 400, behavior: 'auto' }],
      [{ top: 700, behavior: 'auto' }],
    ]);
  });
});

describe('findSectionHeading', () => {
  it('finds nothing for an empty fragment, even beside a heading with an empty slug', () => {
    const note = mountNote('## ???\n\n## Ordered steps');
    expect(findSectionHeading(note.content, '')).toBeNull();
    expect(findSectionHeading(note.content, 'ordered-steps')).toBe(note.heading('Ordered steps'));
  });
});
