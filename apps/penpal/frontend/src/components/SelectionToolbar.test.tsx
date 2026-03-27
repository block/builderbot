import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import React from 'react';
import SelectionToolbar, { getSelectionMarkdown } from './SelectionToolbar';

// E-PENPAL-ANCHOR-COMPUTE: verifies computeAnchor uses document-order start line for backwards selections.
describe('SelectionToolbar', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('uses document-order start line for backwards text selection', async () => {
    // Build a content div with two paragraphs at different source lines.
    const contentDiv = document.createElement('div');
    const para1 = document.createElement('p');
    para1.setAttribute('data-source-line', '1');
    const text1 = document.createTextNode('line one');
    para1.appendChild(text1);
    const para5 = document.createElement('p');
    para5.setAttribute('data-source-line', '5');
    const text5 = document.createTextNode('line five');
    para5.appendChild(text5);
    contentDiv.appendChild(para1);
    contentDiv.appendChild(para5);
    document.body.appendChild(contentDiv);

    const onComment = vi.fn();
    const contentRef = { current: contentDiv } as React.RefObject<HTMLDivElement | null>;

    render(
      <SelectionToolbar
        contentRef={contentRef}
        rawMarkdown={'line one\n\n\n\nline five'}
        onComment={onComment}
      />,
    );

    // Simulate a backwards selection: the user dragged from para5 up to para1.
    // sel.anchorNode = text5 (where drag started), sel.focusNode = text1 (where it ended).
    // Range always reflects document order: startContainer = text1, endContainer = text5.
    const mockRange = {
      startContainer: text1,
      startOffset: 0,
      endContainer: text1,
      endOffset: text1.nodeValue!.length,
      commonAncestorContainer: para1,
      getBoundingClientRect: () =>
        ({ top: 10, bottom: 20, left: 5, right: 50, width: 45, height: 10 }) as DOMRect,
      intersectsNode: (n: Node) => n === text1,
    };

    vi.spyOn(window, 'getSelection').mockReturnValue({
      anchorNode: text5, // backwards: drag started in para5
      focusNode: text1,  // backwards: drag ended in para1
      isCollapsed: false,
      rangeCount: 1,
      toString: () => 'line one',
      getRangeAt: () => mockRange as unknown as Range,
      removeAllRanges: vi.fn(),
    } as unknown as Selection);

    // Trigger mouseup on the content div and wait for the 10ms debounce.
    await act(async () => {
      contentDiv.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
      await new Promise((r) => setTimeout(r, 20));
    });

    // Click the Comment button.
    const commentBtn = screen.getByText('Comment');
    await act(async () => {
      commentBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    expect(onComment).toHaveBeenCalledOnce();
    const [anchor] = onComment.mock.calls[0];
    // Must use range.startContainer (text1 in para1 = line 1),
    // NOT sel.anchorNode (text5 in para5 = line 5).
    expect(anchor.startLine).toBe(1);

    document.body.removeChild(contentDiv);
  });
});

// E-PENPAL-COPY-MD: verifies getSelectionMarkdown extracts raw markdown from data-source-line attributes.
describe('getSelectionMarkdown', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('extracts markdown lines matching selected data-source-line range', () => {
    const rawMarkdown = '# Heading\n\nFirst paragraph.\n\nSecond paragraph.\n\nThird paragraph.';
    //                    line 1       line 3             line 5              line 7

    const contentDiv = document.createElement('div');
    const h1 = document.createElement('h1');
    h1.setAttribute('data-source-line', '1');
    h1.textContent = 'Heading';
    const p1 = document.createElement('p');
    p1.setAttribute('data-source-line', '3');
    const text1 = document.createTextNode('First paragraph.');
    p1.appendChild(text1);
    const p2 = document.createElement('p');
    p2.setAttribute('data-source-line', '5');
    const text2 = document.createTextNode('Second paragraph.');
    p2.appendChild(text2);
    const p3 = document.createElement('p');
    p3.setAttribute('data-source-line', '7');
    p3.textContent = 'Third paragraph.';

    contentDiv.appendChild(h1);
    contentDiv.appendChild(p1);
    contentDiv.appendChild(p2);
    contentDiv.appendChild(p3);
    document.body.appendChild(contentDiv);

    // Mock a selection spanning from p1 (line 3) to p2 (line 5)
    const mockRange = {
      startContainer: text1,
      startOffset: 0,
      endContainer: text2,
      endOffset: text2.nodeValue!.length,
    };

    vi.spyOn(window, 'getSelection').mockReturnValue({
      anchorNode: text1,
      focusNode: text2,
      isCollapsed: false,
      rangeCount: 1,
      toString: () => 'First paragraph.Second paragraph.',
      getRangeAt: () => mockRange as unknown as Range,
      removeAllRanges: vi.fn(),
    } as unknown as Selection);

    const result = getSelectionMarkdown(rawMarkdown, contentDiv);

    // Should extract lines 3-6 (from data-source-line 3 up to but not including data-source-line 7)
    expect(result).toBe('First paragraph.\n\nSecond paragraph.');

    document.body.removeChild(contentDiv);
  });

  it('extracts a single block when start and end are in the same element', () => {
    const rawMarkdown = '# Title\n\nSome content here.';
    //                    line 1     line 3

    const contentDiv = document.createElement('div');
    const h1 = document.createElement('h1');
    h1.setAttribute('data-source-line', '1');
    h1.textContent = 'Title';
    const p = document.createElement('p');
    p.setAttribute('data-source-line', '3');
    const textNode = document.createTextNode('Some content here.');
    p.appendChild(textNode);
    contentDiv.appendChild(h1);
    contentDiv.appendChild(p);
    document.body.appendChild(contentDiv);

    const mockRange = {
      startContainer: textNode,
      startOffset: 0,
      endContainer: textNode,
      endOffset: textNode.nodeValue!.length,
    };

    vi.spyOn(window, 'getSelection').mockReturnValue({
      anchorNode: textNode,
      focusNode: textNode,
      isCollapsed: false,
      rangeCount: 1,
      toString: () => 'Some content here.',
      getRangeAt: () => mockRange as unknown as Range,
      removeAllRanges: vi.fn(),
    } as unknown as Selection);

    const result = getSelectionMarkdown(rawMarkdown, contentDiv);

    // Line 3 is the last data-source-line element, so it extracts to end of file
    expect(result).toBe('Some content here.');

    document.body.removeChild(contentDiv);
  });

  it('returns null when selection is collapsed', () => {
    const contentDiv = document.createElement('div');
    document.body.appendChild(contentDiv);

    vi.spyOn(window, 'getSelection').mockReturnValue({
      anchorNode: null,
      focusNode: null,
      isCollapsed: true,
      rangeCount: 0,
      toString: () => '',
      getRangeAt: vi.fn(),
      removeAllRanges: vi.fn(),
    } as unknown as Selection);

    const result = getSelectionMarkdown('# Hello', contentDiv);
    expect(result).toBeNull();

    document.body.removeChild(contentDiv);
  });

  it('returns null when selection is outside content element', () => {
    const contentDiv = document.createElement('div');
    const outsideDiv = document.createElement('div');
    const outsideText = document.createTextNode('outside');
    outsideDiv.appendChild(outsideText);
    document.body.appendChild(contentDiv);
    document.body.appendChild(outsideDiv);

    vi.spyOn(window, 'getSelection').mockReturnValue({
      anchorNode: outsideText,
      focusNode: outsideText,
      isCollapsed: false,
      rangeCount: 1,
      toString: () => 'outside',
      getRangeAt: () => ({
        startContainer: outsideText,
        endContainer: outsideText,
      }) as unknown as Range,
      removeAllRanges: vi.fn(),
    } as unknown as Selection);

    const result = getSelectionMarkdown('# Hello', contentDiv);
    expect(result).toBeNull();

    document.body.removeChild(contentDiv);
    document.body.removeChild(outsideDiv);
  });

  it('copies markdown via clipboard on Copy markdown button click', async () => {
    const rawMarkdown = '# Title\n\nBody text here.';
    const contentDiv = document.createElement('div');
    const h1 = document.createElement('h1');
    h1.setAttribute('data-source-line', '1');
    h1.textContent = 'Title';
    const p = document.createElement('p');
    p.setAttribute('data-source-line', '3');
    const textNode = document.createTextNode('Body text here.');
    p.appendChild(textNode);
    contentDiv.appendChild(h1);
    contentDiv.appendChild(p);
    document.body.appendChild(contentDiv);

    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });

    const onComment = vi.fn();
    const contentRef = { current: contentDiv } as React.RefObject<HTMLDivElement | null>;

    render(
      <SelectionToolbar
        contentRef={contentRef}
        rawMarkdown={rawMarkdown}
        onComment={onComment}
      />,
    );

    const mockRange = {
      startContainer: textNode,
      startOffset: 0,
      endContainer: textNode,
      endOffset: textNode.nodeValue!.length,
      commonAncestorContainer: p,
      getBoundingClientRect: () =>
        ({ top: 10, bottom: 20, left: 5, right: 50, width: 45, height: 10 }) as DOMRect,
    };

    vi.spyOn(window, 'getSelection').mockReturnValue({
      anchorNode: textNode,
      focusNode: textNode,
      isCollapsed: false,
      rangeCount: 1,
      toString: () => 'Body text here.',
      getRangeAt: () => mockRange as unknown as Range,
      removeAllRanges: vi.fn(),
    } as unknown as Selection);

    // Trigger mouseup to show toolbar
    await act(async () => {
      contentDiv.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
      await new Promise((r) => setTimeout(r, 20));
    });

    // Click Copy markdown button
    const copyBtn = screen.getByText('Copy markdown');
    await act(async () => {
      copyBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    expect(writeText).toHaveBeenCalledWith('Body text here.');

    document.body.removeChild(contentDiv);
  });
});
