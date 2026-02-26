import { useEffect } from 'react';

type MermaidAPI = {
  initialize: (config: Record<string, unknown>) => void;
  render: (id: string, source: string) => Promise<{ svg: string }>;
};

let mermaidPromise: Promise<MermaidAPI> | null = null;
let mermaidReady = false;

async function getMermaid(): Promise<MermaidAPI> {
  if (!mermaidPromise) {
    mermaidPromise = import('mermaid').then((mod) => mod.default as unknown as MermaidAPI);
  }
  const mermaid = await mermaidPromise;
  if (!mermaidReady) {
    const theme =
      document.documentElement.getAttribute('data-theme') === 'dark' ? 'dark' : 'default';
    mermaid.initialize({ startOnLoad: false, theme });
    mermaidReady = true;
  }
  return mermaid;
}

/**
 * Renders all mermaid code blocks within a content element.
 * Call this after markdown has been rendered to the DOM.
 */
export async function renderMermaidBlocks(contentEl: HTMLElement) {
  const mermaid = await getMermaid();

  // Find mermaid containers (created by MarkdownViewer's code component)
  const containers = contentEl.querySelectorAll('.mermaid-container[data-mermaid-source]');
  if (containers.length === 0) return;

  // Render sequentially to avoid mermaid parser state leaks
  for (let i = 0; i < containers.length; i++) {
    const container = containers[i] as HTMLElement;
    const source = container.getAttribute('data-mermaid-source');
    if (!source) continue;

    const id = `mermaid-${Date.now()}-${i}`;
    try {
      const { svg } = await mermaid.render(id, source.trim());
      container.innerHTML = svg;
      container.removeAttribute('data-mermaid-source');

      // Add expand button
      const expandBtn = document.createElement('button');
      expandBtn.className = 'expand-zoom-btn';
      expandBtn.textContent = 'Expand';
      expandBtn.onclick = () => openExpandModal(source.trim());
      container.style.position = 'relative';
      container.appendChild(expandBtn);
    } catch (err) {
      console.error('Mermaid render error:', err);
      // Leave the pre/code fallback visible
    }
  }
}

/**
 * Re-renders all mermaid diagrams with the current theme.
 */
export async function rerenderMermaidForTheme(_contentEl: HTMLElement) {
  const mermaid = await getMermaid();
  const theme =
    document.documentElement.getAttribute('data-theme') === 'dark' ? 'dark' : 'default';
  mermaid.initialize({ startOnLoad: false, theme });
  mermaidReady = true;
}

function openExpandModal(source: string) {
  // Create modal overlay
  const overlay = document.createElement('div');
  overlay.className = 'expand-modal-overlay open';
  overlay.onclick = (e) => {
    if (e.target === overlay) overlay.remove();
  };

  const closeBtn = document.createElement('button');
  closeBtn.className = 'expand-modal-close';
  closeBtn.innerHTML = '&times; Close';
  closeBtn.onclick = () => overlay.remove();

  const body = document.createElement('div');
  body.className = 'expand-modal-body';

  overlay.appendChild(closeBtn);
  overlay.appendChild(body);
  document.body.appendChild(overlay);

  // Render mermaid in modal
  getMermaid().then(async (mermaid) => {
    const id = `mermaid-modal-${Date.now()}`;
    try {
      const { svg } = await mermaid.render(id, source);
      body.innerHTML = svg;
    } catch {
      body.textContent = 'Failed to render diagram';
    }
  });

  // Close on Escape
  const handleKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      overlay.remove();
      document.removeEventListener('keydown', handleKey);
    }
  };
  document.addEventListener('keydown', handleKey);
}

/**
 * Hook to render mermaid blocks when content changes.
 */
export function useMermaid(contentRef: React.RefObject<HTMLDivElement | null>, deps: unknown[]) {
  useEffect(() => {
    if (!contentRef.current) return;
    renderMermaidBlocks(contentRef.current);
  }, deps);
}
