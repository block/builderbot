import { useEffect, useRef, useState } from 'react';

interface FindBarProps {
  onClose: () => void;
}

// Persistent Highlight objects — reuse them to avoid stale references
const matchesHighlight = new Highlight();
const activeHighlight = new Highlight();

function findAllRanges(root: Element, query: string, exclude: Element | null): Range[] {
  const ranges: Range[] = [];
  if (!query) return ranges;
  const lower = query.toLowerCase();
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      if (exclude?.contains(node)) return NodeFilter.FILTER_REJECT;
      return NodeFilter.FILTER_ACCEPT;
    },
  });
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    const text = node.textContent?.toLowerCase() || '';
    let idx = text.indexOf(lower);
    while (idx !== -1) {
      const range = new Range();
      range.setStart(node, idx);
      range.setEnd(node, idx + query.length);
      ranges.push(range);
      idx = text.indexOf(lower, idx + 1);
    }
  }
  return ranges;
}

function updateHighlights(ranges: Range[], activeIdx: number) {
  matchesHighlight.clear();
  activeHighlight.clear();
  for (const r of ranges) matchesHighlight.add(r);
  if (ranges[activeIdx]) activeHighlight.add(ranges[activeIdx]);
}

export default function FindBar({ onClose }: FindBarProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState('');
  const [currentIdx, setCurrentIdx] = useState(0);
  const [ranges, setRanges] = useState<Range[]>([]);

  // Register highlight objects once on mount, clean up on unmount
  useEffect(() => {
    CSS.highlights.set('find-matches', matchesHighlight);
    CSS.highlights.set('find-active', activeHighlight);
    return () => {
      matchesHighlight.clear();
      activeHighlight.clear();
      CSS.highlights.delete('find-matches');
      CSS.highlights.delete('find-active');
    };
  }, []);

  // Focus input on mount
  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  // Recompute matches + highlights when query changes
  useEffect(() => {
    if (!query) {
      setRanges([]);
      setCurrentIdx(0);
      updateHighlights([], -1);
      return;
    }
    const content = document.querySelector('.main-content');
    if (!content) return;
    const found = findAllRanges(content, query, barRef.current);
    setRanges(found);
    setCurrentIdx(0);
    updateHighlights(found, 0);
    if (found.length > 0) {
      requestAnimationFrame(() => scrollToRange(found[0]));
    }
  }, [query]);

  function scrollToRange(range: Range) {
    const container = document.querySelector('.main-content');
    if (!container) return;
    const rangeRect = range.getBoundingClientRect();
    const containerRect = container.getBoundingClientRect();
    const offsetTop = rangeRect.top - containerRect.top + container.scrollTop;
    container.scrollTo({
      top: offsetTop - containerRect.height / 2,
      behavior: 'smooth',
    });
  }

  function navigate(delta: number) {
    if (ranges.length === 0) return;
    const next = (currentIdx + delta + ranges.length) % ranges.length;
    setCurrentIdx(next);
    updateHighlights(ranges, next);
    scrollToRange(ranges[next]);
  }

  function handleClose() {
    updateHighlights([], -1);
    onClose();
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === 'Escape') {
      handleClose();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      e.stopPropagation();
      navigate(e.shiftKey ? -1 : 1);
    }
  }

  return (
    <div className="find-bar" ref={barRef}>
      <input
        ref={inputRef}
        type="text"
        className="find-bar-input"
        placeholder="Find in page..."
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        spellCheck={false}
        onKeyDown={handleKeyDown}
      />
      {query && (
        <span className="find-bar-count">
          {ranges.length === 0 ? 'No matches' : `${currentIdx + 1} of ${ranges.length}`}
        </span>
      )}
      <button className="find-bar-btn" onClick={() => navigate(-1)} disabled={ranges.length === 0} aria-label="Previous match" title="Previous (Shift+Enter)">
        &#8593;
      </button>
      <button className="find-bar-btn" onClick={() => navigate(1)} disabled={ranges.length === 0} aria-label="Next match" title="Next (Enter)">
        &#8595;
      </button>
      <button className="find-bar-btn find-bar-close" onClick={handleClose} aria-label="Close find bar" title="Close (Escape)">
        &#10005;
      </button>
    </div>
  );
}
