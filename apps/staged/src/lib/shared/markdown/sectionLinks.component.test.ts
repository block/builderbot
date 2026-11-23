import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';

import { renderHashtagTokens } from '../../features/sessions/hashtagItems';
import { renderMarkdown } from './renderMarkdown';
import { findSectionHeading, followSectionLink, sectionLinkFragment } from './sectionLinks';

// jsdom is only provided to `*.component.test.ts` files (see vitest.config.ts).
//
// jsdom lays nothing out and has no `Element.scrollTo`, so each note here is
// given its geometry by hand and a spy to scroll with. What these tests show
// is which heading a link resolves to, where the scroll is aimed, and that
// the page does not navigate. That the note visibly moves is not something
// jsdom can show, nor that Enter on a focused link raises the click.

/** Where the scroll box's top edge sits in the viewport. */
const PORT_TOP = 100;

interface Note {
  scroller: HTMLElement;
  content: HTMLElement;
  scrollTo: Mock<(options: ScrollToOptions) => void>;
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
 * laid out 400px apart, the first at the top of the box.
 */
function mountNote(markdown: string, html = renderMarkdown(markdown)): Note {
  const scroller = document.createElement('div');
  const content = document.createElement('div');
  content.className = 'markdown-content';
  content.innerHTML = html;
  scroller.append(content);
  document.body.append(scroller);

  at(scroller, PORT_TOP);
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

describe('findSectionHeading', () => {
  it('finds nothing for an empty fragment, even beside a heading with an empty slug', () => {
    const note = mountNote('## ???\n\n## Ordered steps');
    expect(findSectionHeading(note.content, '')).toBeNull();
    expect(findSectionHeading(note.content, 'ordered-steps')).toBe(note.heading('Ordered steps'));
  });
});
