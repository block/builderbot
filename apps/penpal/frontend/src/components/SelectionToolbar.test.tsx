import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import React from 'react';
import SelectionToolbar from './SelectionToolbar';

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
