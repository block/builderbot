import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';

// Polyfill CSS Highlight API for test environment
if (typeof globalThis.Highlight === 'undefined') {
  globalThis.Highlight = class Highlight {
    clear() {}
    add() {}
    delete() {}
  } as unknown as typeof globalThis.Highlight;
  (globalThis.CSS as Record<string, unknown>).highlights = new Map();
}

// Polyfill Range.prototype.getBoundingClientRect (not available in jsdom)
if (!Range.prototype.getBoundingClientRect) {
  Range.prototype.getBoundingClientRect = function () {
    return { top: 0, right: 0, bottom: 0, left: 0, width: 0, height: 0, x: 0, y: 0, toJSON: () => ({}) } as DOMRect;
  };
}

// Must import after polyfill
const { default: FindBar } = await import('./FindBar');

// E-PENPAL-FIND-BAR: verifies FindBar rendering, match counting, navigation, and close behavior.
describe('FindBar', () => {
  let mainContent: HTMLDivElement;

  beforeEach(() => {
    // Create a .main-content element that FindBar searches within
    mainContent = document.createElement('div');
    mainContent.className = 'main-content';
    mainContent.innerHTML = '<p>The quick brown fox jumps over the lazy dog. The fox is quick.</p>';
    // Mock scrollTo on the element (not available in jsdom)
    mainContent.scrollTo = vi.fn();
    document.body.appendChild(mainContent);
  });

  afterEach(() => {
    document.body.removeChild(mainContent);
    vi.restoreAllMocks();
  });

  it('renders the find bar with input and buttons', () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    expect(screen.getByPlaceholderText('Find in page...')).toBeInTheDocument();
    expect(screen.getByLabelText('Previous match')).toBeInTheDocument();
    expect(screen.getByLabelText('Next match')).toBeInTheDocument();
    expect(screen.getByLabelText('Close find bar')).toBeInTheDocument();
  });

  it('focuses the input on mount', () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');
    expect(document.activeElement).toBe(input);
  });

  it('shows match count when typing a query', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');

    await act(async () => {
      fireEvent.change(input, { target: { value: 'fox' } });
    });

    // "fox" appears twice in the content
    expect(screen.getByText('1 of 2')).toBeInTheDocument();
  });

  it('shows "No matches" for a query with no results', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');

    await act(async () => {
      fireEvent.change(input, { target: { value: 'zebra' } });
    });

    expect(screen.getByText('No matches')).toBeInTheDocument();
  });

  it('does not show match count when query is empty', () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    // No count element should appear when query is empty
    expect(screen.queryByText(/of/)).not.toBeInTheDocument();
    expect(screen.queryByText('No matches')).not.toBeInTheDocument();
  });

  it('navigates to next match on Next button click', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');

    await act(async () => {
      fireEvent.change(input, { target: { value: 'fox' } });
    });

    expect(screen.getByText('1 of 2')).toBeInTheDocument();

    // Click Next
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Next match'));
    });

    expect(screen.getByText('2 of 2')).toBeInTheDocument();
  });

  it('navigates to previous match on Previous button click', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');

    await act(async () => {
      fireEvent.change(input, { target: { value: 'fox' } });
    });

    // Go to next first
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Next match'));
    });

    expect(screen.getByText('2 of 2')).toBeInTheDocument();

    // Now go back
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Previous match'));
    });

    expect(screen.getByText('1 of 2')).toBeInTheDocument();
  });

  it('wraps around when navigating past last match', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');

    await act(async () => {
      fireEvent.change(input, { target: { value: 'fox' } });
    });

    // Click Next twice to wrap around (2 matches)
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Next match'));
    });
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Next match'));
    });

    // Should wrap to first match
    expect(screen.getByText('1 of 2')).toBeInTheDocument();
  });

  it('calls onClose when Escape is pressed', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');

    await act(async () => {
      fireEvent.keyDown(input, { key: 'Escape' });
    });

    expect(onClose).toHaveBeenCalledOnce();
  });

  it('calls onClose when close button is clicked', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Close find bar'));
    });

    expect(onClose).toHaveBeenCalledOnce();
  });

  it('navigates forward on Enter key and backward on Shift+Enter', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');

    await act(async () => {
      fireEvent.change(input, { target: { value: 'the' } });
    });

    // "the" appears 3 times (case-insensitive): "The quick...", "the lazy", "The fox..."
    expect(screen.getByText('1 of 3')).toBeInTheDocument();

    // Enter should navigate forward
    await act(async () => {
      fireEvent.keyDown(input, { key: 'Enter' });
    });

    expect(screen.getByText('2 of 3')).toBeInTheDocument();

    // Shift+Enter should navigate backward
    await act(async () => {
      fireEvent.keyDown(input, { key: 'Enter', shiftKey: true });
    });

    expect(screen.getByText('1 of 3')).toBeInTheDocument();
  });

  it('disables navigation buttons when there are no matches', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    // With no query, buttons should be disabled
    expect(screen.getByLabelText('Previous match')).toBeDisabled();
    expect(screen.getByLabelText('Next match')).toBeDisabled();

    // Type a query that has no matches
    const input = screen.getByPlaceholderText('Find in page...');
    await act(async () => {
      fireEvent.change(input, { target: { value: 'xyznonexistent' } });
    });

    expect(screen.getByLabelText('Previous match')).toBeDisabled();
    expect(screen.getByLabelText('Next match')).toBeDisabled();
  });

  // E-PENPAL-FIND-BAR: verifies scrollTo is called when matches are found.
  it('calls scrollTo on initial match', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');

    await act(async () => {
      fireEvent.change(input, { target: { value: 'fox' } });
      // Allow requestAnimationFrame to fire
      await new Promise(resolve => requestAnimationFrame(resolve));
    });

    expect(mainContent.scrollTo).toHaveBeenCalled();
    // scrollTo should be called with an object containing 'top' and 'behavior: smooth'
    const call = (mainContent.scrollTo as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(call).toHaveProperty('behavior', 'smooth');
    expect(call).toHaveProperty('top');
  });

  // E-PENPAL-FIND-BAR: verifies scrollTo is called when navigating between matches.
  it('calls scrollTo when navigating between matches', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');

    await act(async () => {
      fireEvent.change(input, { target: { value: 'fox' } });
      await new Promise(resolve => requestAnimationFrame(resolve));
    });

    (mainContent.scrollTo as ReturnType<typeof vi.fn>).mockClear();

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Next match'));
      await new Promise(resolve => requestAnimationFrame(resolve));
    });

    expect(mainContent.scrollTo).toHaveBeenCalled();
  });

  // E-PENPAL-FIND-BAR: verifies scrollTo targets .file-main-scroll on file pages.
  it('scrolls .file-main-scroll instead of .main-content on file pages', async () => {
    // Simulate file page layout: .main-content has overflow:hidden,
    // scrollable container is .file-main-scroll inside it.
    const scrollContainer = document.createElement('div');
    scrollContainer.className = 'file-main-scroll';
    scrollContainer.scrollTo = vi.fn();
    // Move the text content into the scroll container
    while (mainContent.firstChild) {
      scrollContainer.appendChild(mainContent.firstChild);
    }
    mainContent.appendChild(scrollContainer);

    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');

    await act(async () => {
      fireEvent.change(input, { target: { value: 'fox' } });
      await new Promise(resolve => requestAnimationFrame(resolve));
    });

    // scrollTo should be called on .file-main-scroll, not .main-content
    expect(scrollContainer.scrollTo).toHaveBeenCalled();
    expect(mainContent.scrollTo).not.toHaveBeenCalled();
  });

  // E-PENPAL-FIND-BAR: verifies scrollTo is NOT called when no matches are found.
  it('does not call scrollTo when no matches found', async () => {
    const onClose = vi.fn();
    render(<FindBar onClose={onClose} />);

    const input = screen.getByPlaceholderText('Find in page...');

    await act(async () => {
      fireEvent.change(input, { target: { value: 'xyznonexistent' } });
      await new Promise(resolve => requestAnimationFrame(resolve));
    });

    expect(mainContent.scrollTo).not.toHaveBeenCalled();
  });
});
