// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../commands', () => ({
  getBranchTimeline: vi.fn(),
  listProjectNotes: vi.fn(),
}));

import type { HashtagItem } from '../../types';
import { createBadgeElement } from './hashtagEditorContent';
import { renderHashtagTokens } from './hashtagItems';
import { renderMarkdown } from '../../shared/markdown/renderMarkdown';
import {
  badgeReference,
  listenForReferenceCopy,
  readReferenceClipboard,
  serializeRange,
  stagedCopyHtml,
  stagedCopyText,
  writeReferenceClipboard,
} from './referenceClipboard';

const NOTE_ID = 'b7c40af7-ca85-4b29-98f3-4728a508de83';
const SHA = '29282bb5f0e1';

const items: HashtagItem[] = [
  {
    type: 'note',
    id: NOTE_ID,
    title: 'Porting notes',
    color: '--note-color',
    bgColor: '--note-bg',
  },
  {
    type: 'commit',
    id: SHA,
    title: 'fix: keep refs',
    color: '--commit-color',
    bgColor: '--commit-bg',
  },
];

afterEach(() => {
  document.body.innerHTML = '';
  document.getSelection()?.removeAllRanges();
});

/** Mount rendered HTML and return its container. */
function mount(html: string): HTMLElement {
  const root = document.createElement('div');
  root.innerHTML = html;
  document.body.appendChild(root);
  return root;
}

/** A rendered message: `see <badge> and <badge> done`. */
function renderedMessage(): HTMLElement {
  return mount(
    `<p>${renderHashtagTokens(`see #note:${NOTE_ID} and #commit:${SHA} done`, items)}</p>`
  );
}

function rangeOf(root: Node): Range {
  const range = document.createRange();
  range.selectNodeContents(root);
  return range;
}

/** The text node holding a badge's title. */
function titleText(badge: Element): Text {
  return badge.querySelector('.hashtag-badge-label')!.firstChild as Text;
}

/** jsdom has no ClipboardEvent/DataTransfer; a `copy` with a recording `clipboardData`. */
function copyEvent(): { event: Event; data: Record<string, string> } {
  const data: Record<string, string> = {};
  const event = new Event('copy', { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'clipboardData', {
    value: {
      setData: (type: string, value: string) => {
        data[type] = value;
      },
    },
  });
  return { event, data };
}

function dataTransfer(flavours: Record<string, string>): DataTransfer {
  return { getData: (type: string) => flavours[type] ?? '' } as unknown as DataTransfer;
}

function select(range: Range) {
  const selection = document.getSelection()!;
  selection.removeAllRanges();
  selection.addRange(range);
  return selection;
}

describe('badgeReference', () => {
  it('reads every badge shape and refuses anything that is not a reference', () => {
    const rendered = mount(renderHashtagTokens(`#note:${NOTE_ID}`, items)).firstElementChild!;
    expect(badgeReference(rendered)).toBe(`#note:${NOTE_ID}`);

    const plain = mount(
      renderHashtagTokens(`#commit:${SHA}`, items, { interactive: false })
    ).firstElementChild!;
    expect(plain.hasAttribute('data-hashtag-ref')).toBe(false);
    expect(badgeReference(plain)).toBe(`#commit:${SHA}`);

    expect(badgeReference(createBadgeElement(items[0]))).toBe(`#note:${NOTE_ID}`);

    const forged = mount(
      '<span data-hashtag-ref="javascript:alert(1)">x</span>'
    ).firstElementChild!;
    expect(badgeReference(forged)).toBeNull();
    expect(badgeReference(mount('<span>plain</span>').firstElementChild!)).toBeNull();
  });
});

describe('serializeRange', () => {
  it('copies whole badges as their source form in text and as tagged spans in HTML', () => {
    const root = renderedMessage();
    expect(serializeRange(rangeOf(root), 'text')).toBe(
      `see #note:${NOTE_ID} and #commit:${SHA} done`
    );
    expect(serializeRange(rangeOf(root), 'html')).toBe(
      `<p>see <span data-staged-ref="#note:${NOTE_ID}">Porting notes</span> and ` +
        `<span data-staged-ref="#commit:${SHA}">fix: keep refs</span> done</p>`
    );
  });

  it('degrades a partly selected badge to the selected part of its title', () => {
    const root = renderedMessage();
    const [note, commit] = Array.from(root.querySelectorAll('.hashtag-badge'));
    const range = document.createRange();
    range.setStart(titleText(note), titleText(note).data.indexOf('notes'));
    range.setEnd(titleText(commit), titleText(commit).data.indexOf(':') + 1);

    expect(serializeRange(range, 'text')).toBe('notes and fix:');
    expect(serializeRange(range, 'html')).toBe('notes and fix:');
  });

  it('keeps the reference when the selection is exactly the title text', () => {
    const root = renderedMessage();
    const note = root.querySelector('.hashtag-badge')!;
    const range = document.createRange();
    range.selectNodeContents(titleText(note));
    expect(serializeRange(range, 'text')).toBe(`#note:${NOTE_ID}`);
  });

  it('serializes the composer editor: badges, caret anchors and line breaks', () => {
    const editor = mount('');
    editor.setAttribute('contenteditable', 'true');
    editor.append('ask ', createBadgeElement(items[0]), '\u200B', document.createElement('br'));
    editor.append('then ', createBadgeElement(items[1]), '\u200B');

    expect(serializeRange(rangeOf(editor), 'text')).toBe(
      `ask #note:${NOTE_ID}\nthen #commit:${SHA}`
    );
    expect(serializeRange(rangeOf(editor), 'html')).toBe(
      `ask <span data-staged-ref="#note:${NOTE_ID}">Porting notes</span><br>` +
        `then <span data-staged-ref="#commit:${SHA}">fix: keep refs</span>`
    );
  });

  it('keeps structure in HTML and separates blocks and cells in text', () => {
    const root = mount(
      '<h2>Title</h2>\n<p><strong>bold</strong> <em>em</em> <code>c</code> ' +
        '<a href="https://example.com/x">site</a> <a href="javascript:x">bad</a></p>\n' +
        '<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n' +
        '<table><tr><td>a</td><td>b</td></tr></table>' +
        '<custom-el>tail</custom-el>'
    );
    expect(serializeRange(rangeOf(root), 'text')).toBe(
      'Title\n\nbold em c site bad\n\none\ntwo\n\na\tb\ntail'
    );
    expect(serializeRange(rangeOf(root), 'html')).toBe(
      '<h2>Title</h2><p><strong>bold</strong> <em>em</em> <code>c</code> ' +
        '<a href="https://example.com/x">site</a> bad</p>' +
        '<ul><li>one</li><li>two</li></ul><table><tbody><tr><td>a</td><td>b</td></tr></tbody></table>tail'
    );
  });

  it('keeps the blank line between paragraphs that the browser’s own copy gives', () => {
    const badge = renderHashtagTokens(`#note:${NOTE_ID}`, items);
    const root = mount(`<div><p>first ${badge}</p>\n<p>second</p></div>`);
    expect(serializeRange(rangeOf(root), 'text')).toBe(`first #note:${NOTE_ID}\n\nsecond`);
    expect(serializeRange(rangeOf(root), 'html')).toBe(
      `<div><p>first <span data-staged-ref="#note:${NOTE_ID}">Porting notes</span></p><p>second</p></div>`
    );
  });

  it('separates list items by a single newline and a heading from a paragraph by a blank one', () => {
    const badge = renderHashtagTokens(`#commit:${SHA}`, items);
    const list = mount(`<ul>\n<li>one ${badge}</li>\n<li>two</li>\n<li>three</li>\n</ul>`);
    expect(serializeRange(rangeOf(list), 'text')).toBe(`one #commit:${SHA}\ntwo\nthree`);
    expect(serializeRange(rangeOf(list), 'html')).toBe(
      `<ul><li>one <span data-staged-ref="#commit:${SHA}">fix: keep refs</span></li><li>two</li><li>three</li></ul>`
    );

    const headed = mount(`<h3>Plan</h3>\n<p>see ${badge}</p>`);
    expect(serializeRange(rangeOf(headed), 'text')).toBe(`Plan\n\nsee #commit:${SHA}`);
    expect(serializeRange(rangeOf(headed), 'html')).toBe(
      `<h3>Plan</h3><p>see <span data-staged-ref="#commit:${SHA}">fix: keep refs</span></p>`
    );
  });

  it('never doubles a blank line a trailing <br> or an empty paragraph already supplies', () => {
    const root = mount('<p>a<br></p><p>b</p><p></p><p>c</p>');
    expect(serializeRange(rangeOf(root), 'text')).toBe('a\n\nb\n\nc');
  });

  it('keeps the column of an empty cell, as the browser’s own copy does', () => {
    const badge = renderHashtagTokens(`#note:${NOTE_ID}`, items);
    const root = mount(
      `<table><tr><td>a ${badge}</td><td></td><td>c</td></tr>` +
        '<tr><td></td><td>e</td><td>f</td></tr></table>'
    );
    expect(serializeRange(rangeOf(root), 'text')).toBe(`a #note:${NOTE_ID}\t\tc\ne\tf`);
    expect(serializeRange(rangeOf(root), 'html')).toBe(
      `<table><tbody><tr><td>a <span data-staged-ref="#note:${NOTE_ID}">Porting notes</span></td>` +
        '<td></td><td>c</td></tr><tr><td></td><td>e</td><td>f</td></tr></tbody></table>'
    );
  });

  it('puts no tab after a cell that ends in a line break, as the browser’s own copy does', () => {
    const badge = renderHashtagTokens(`#commit:${SHA}`, items);
    const root = mount(`<table><tr><td>a<br></td><td>${badge}</td></tr></table>`);
    expect(serializeRange(rangeOf(root), 'text')).toBe(`a\n#commit:${SHA}`);
    expect(serializeRange(rangeOf(root), 'html')).toBe(
      `<table><tbody><tr><td>a<br></td><td><span data-staged-ref="#commit:${SHA}">fix: keep refs</span></td></tr></tbody></table>`
    );
  });

  it('separates a table head from its body by a single newline', () => {
    const badge = renderHashtagTokens(`#note:${NOTE_ID}`, items);
    const root = mount(
      '<table>\n<thead>\n<tr><th>Item</th><th>Note</th></tr>\n</thead>\n' +
        `<tbody>\n<tr><td>one</td><td>${badge}</td></tr>\n<tr><td>two</td><td></td></tr>\n</tbody>\n</table>`
    );
    expect(serializeRange(rangeOf(root), 'text')).toBe(`Item\tNote\none\t#note:${NOTE_ID}\ntwo\t`);
    expect(serializeRange(rangeOf(root), 'html')).toBe(
      '<table><thead><tr><th>Item</th><th>Note</th></tr></thead>' +
        `<tbody><tr><td>one</td><td><span data-staged-ref="#note:${NOTE_ID}">Porting notes</span></td></tr>` +
        '<tr><td>two</td><td></td></tr></tbody></table>'
    );
  });

  it('copies a rendered GFM table one row per line and one tab per column', () => {
    const root = mount(
      renderMarkdown(
        '| Item | Note | Extra |\n|---|---|---|\n' +
          `| one | #note:${NOTE_ID} | |\n| two | three | |\n`,
        { renderInlineText: (text) => renderHashtagTokens(text, items) }
      )
    );
    expect(root.querySelector('tr')!.innerHTML.startsWith('\n<th>')).toBe(true);
    expect(serializeRange(rangeOf(root), 'text')).toBe(
      `Item\tNote\tExtra\none\t#note:${NOTE_ID}\t\ntwo\tthree\t`
    );
    expect(serializeRange(rangeOf(root), 'html')).toBe(
      '<table><thead><tr><th>Item</th><th>Note</th><th>Extra</th></tr></thead>' +
        `<tbody><tr><td>one</td><td><span data-staged-ref="#note:${NOTE_ID}">Porting notes</span></td><td></td></tr>` +
        '<tr><td>two</td><td>three</td><td></td></tr></tbody></table>'
    );
  });

  it('leaves out what the page never shows, as the browser’s own copy would', () => {
    const root = mount(
      `<div><p>see ${renderHashtagTokens(`#note:${NOTE_ID}`, items)}</p>` +
        '<style>.x { color: red }</style><script>var leak = 1;</script>' +
        '<template><p>stamp</p></template><noscript>enable js</noscript>' +
        '<p hidden>secret</p><p>done <span hidden>also secret</span></p></div>'
    );
    expect(serializeRange(rangeOf(root), 'text')).toBe(`see #note:${NOTE_ID}\n\ndone `);
    expect(serializeRange(rangeOf(root), 'html')).toBe(
      `<div><p>see <span data-staged-ref="#note:${NOTE_ID}">Porting notes</span></p><p>done </p></div>`
    );
  });

  it('never keeps a badge that has no text to select', () => {
    const root = mount(`<p>see <span data-token="#note:${NOTE_ID}"> </span> done</p>`);
    expect(serializeRange(rangeOf(root), 'text')).toBe('see  done');
    expect(serializeRange(rangeOf(root), 'html')).toBe('<p>see  done</p>');
  });

  it('escapes titles and text read from the DOM', () => {
    const hostile: HashtagItem = { ...items[0], title: '"><img src=x onerror=alert(1)>' };
    const root = mount(`<p>${renderHashtagTokens(`#note:${NOTE_ID} <b>`, [hostile])}</p>`);
    const html = serializeRange(rangeOf(root), 'html');
    expect(html).not.toContain('<img');
    expect(html).toBe(
      `<p><span data-staged-ref="#note:${NOTE_ID}">&quot;&gt;&lt;img src=x onerror=alert(1)&gt;</span> &lt;b&gt;</p>`
    );
  });
});

describe('reading this app’s own HTML back', () => {
  it('round-trips the HTML flavour to the text flavour', () => {
    const root = renderedMessage();
    const html = `<div data-staged-copy>${serializeRange(rangeOf(root), 'html')}</div>`;
    expect(stagedCopyText(html)).toBe(serializeRange(rangeOf(root), 'text'));
  });

  it('prefers own HTML over text/plain and never reads foreign HTML', () => {
    const own = dataTransfer({
      'text/html': `<div data-staged-copy>see <span data-staged-ref="#note:${NOTE_ID}">Porting notes</span></div>`,
      'text/plain': 'see Porting notes',
    });
    expect(readReferenceClipboard(own)).toBe(`see #note:${NOTE_ID}`);

    const foreign = dataTransfer({
      'text/html': `<meta charset="utf-8"><span data-staged-ref="#note:${NOTE_ID}">x</span><b>bold</b>`,
      'text/plain': 'x bold',
    });
    expect(stagedCopyText(foreign.getData('text/html'))).toBeUndefined();
    expect(readReferenceClipboard(foreign)).toBe('x bold');
    expect(readReferenceClipboard(dataTransfer({ 'text/plain': `#note:${NOTE_ID}` }))).toBe(
      `#note:${NOTE_ID}`
    );
  });

  it('turns tagged spans back into reference text for a rich-text target', () => {
    const html =
      `<div data-staged-copy><p>see <span data-staged-ref="#note:${NOTE_ID}">Porting notes</span> ` +
      `<strong>now</strong></p><p><span data-staged-ref="nope">kept text</span></p></div>`;
    expect(stagedCopyHtml(html)).toBe(
      `<p>see #note:${NOTE_ID} <strong>now</strong></p><p>kept text</p>`
    );
    expect(stagedCopyHtml('<p><a href="https://x.y">link</a></p>')).toBeUndefined();
  });

  it('parses inertly', () => {
    const onerror = vi.fn();
    vi.stubGlobal('alertProbe', onerror);
    stagedCopyText('<div data-staged-copy><img src=x onerror="alertProbe()">t</div>');
    expect(onerror).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });
});

describe('writeReferenceClipboard', () => {
  it('writes both flavours wrapped for recognition on paste', () => {
    const root = renderedMessage();
    const { event, data } = copyEvent();
    writeReferenceClipboard((event as ClipboardEvent).clipboardData!, select(rangeOf(root)));
    expect(data['text/plain']).toBe(`see #note:${NOTE_ID} and #commit:${SHA} done`);
    expect(data['text/html']).toMatch(/^<div data-staged-copy><p>see <span data-staged-ref=/);
  });
});

describe('listenForReferenceCopy', () => {
  it('takes over a copy that includes a badge and leaves any other copy alone', () => {
    const unlisten = listenForReferenceCopy();
    try {
      const root = renderedMessage();
      select(rangeOf(root));
      const withBadge = copyEvent();
      root.dispatchEvent(withBadge.event);
      expect(withBadge.event.defaultPrevented).toBe(true);
      expect(withBadge.data['text/plain']).toBe(`see #note:${NOTE_ID} and #commit:${SHA} done`);
      expect(withBadge.data['text/html']).toContain('data-staged-copy');

      const plain = mount('<p>no references here</p>');
      select(rangeOf(plain));
      const without = copyEvent();
      plain.dispatchEvent(without.event);
      expect(without.event.defaultPrevented).toBe(false);
      expect(without.data).toEqual({});
    } finally {
      unlisten();
    }
  });

  it('leaves editors to write their own clipboard', () => {
    const unlisten = listenForReferenceCopy();
    try {
      const editor = mount('');
      editor.setAttribute('contenteditable', 'true');
      editor.append('ask ', createBadgeElement(items[0]), '\u200B');
      select(rangeOf(editor));
      const { event, data } = copyEvent();
      editor.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
      expect(data).toEqual({});
    } finally {
      unlisten();
    }
  });

  it('stops listening once unlistened', () => {
    listenForReferenceCopy()();
    const root = renderedMessage();
    select(rangeOf(root));
    const { event, data } = copyEvent();
    root.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(data).toEqual({});
  });
});
