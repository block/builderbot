import React, { useEffect, useRef, useMemo, forwardRef, useImperativeHandle } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeRaw from 'rehype-raw';
import { Prism as SyntaxHighlighter, type SyntaxHighlighterProps } from 'react-syntax-highlighter';
import { dracula as prismDracula } from 'react-syntax-highlighter/dist/esm/styles/prism';
import type { Components } from 'react-markdown';
import type { Heading } from './TableOfContents';
import remarkSourceLine from './remarkSourceLine';
import rehypeCommentHighlights from './rehypeCommentHighlights';
import { nthIndexOf } from './rehypeCommentHighlights';
import type { ThreadHighlight } from './rehypeCommentHighlights';

// Customize Prism's Dracula to match Go's Chroma Dracula output.
// Chroma uses #f1fa8c (yellow) for strings; Prism defaults to #50fa7b (green).
const dracula: Record<string, React.CSSProperties> = {
  ...prismDracula,
  'string': { color: '#f1fa8c' },
};

const HEADING_ID_PREFIX = 'penpal-md-';

/** Generate heading ID matching Go's goldmark prefixedIDs algorithm */
function generateHeadingId(text: string): string {
  let result = HEADING_ID_PREFIX;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (/[a-zA-Z0-9]/.test(c)) {
      result += c.toLowerCase();
    } else if (c === ' ' || c === '-' || c === '_') {
      result += '-';
    }
    // skip other characters (including multi-byte)
  }
  if (result === HEADING_ID_PREFIX) {
    result += 'heading';
  }
  return result;
}

// ── Code block highlight renderer ──────────────────────────────────────
// SyntaxHighlighter's renderer prop receives a tree of token nodes per line.
// This walks the tokens, finds text matching a highlight's selectedText, and
// wraps the matching range in <mark> elements while preserving syntax styles.

interface RendererNode {
  type: 'element' | 'text';
  value?: string | number;
  tagName?: string;
  properties?: { className?: string[]; style?: React.CSSProperties; [key: string]: unknown };
  children?: RendererNode[];
}

/** Extract all text from a rendererNode tree. */
function extractText(node: RendererNode): string {
  if (node.type === 'text') return String(node.value ?? '');
  return (node.children ?? []).map(extractText).join('');
}

/** Render a rendererNode to a React element, applying inline styles from the stylesheet. */
function renderNode(
  node: RendererNode,
  stylesheet: Record<string, React.CSSProperties>,
  useInlineStyles: boolean,
  key: string | number,
): React.ReactNode {
  if (node.type === 'text') return String(node.value ?? '');
  const classNames = node.properties?.className ?? [];
  const style = useInlineStyles
    ? classNames.reduce<React.CSSProperties>((acc, cls) => ({ ...acc, ...(stylesheet[cls] || {}) }), {})
    : undefined;
  const children = (node.children ?? []).map((child, i) => renderNode(child, stylesheet, useInlineStyles, i));
  const props: Record<string, unknown> = { key, style, className: classNames.join(' ') || undefined };
  // Pass through data-* attributes (e.g., dataThreadId → data-thread-id for <mark> elements)
  if (node.properties) {
    for (const [k, v] of Object.entries(node.properties)) {
      if (k !== 'className' && k !== 'style') props[k] = v;
    }
  }
  return React.createElement(node.tagName ?? 'span', props, ...children);
}

/** Collect all leaf text spans from a rendererNode tree with their character offsets. */
function collectLeaves(node: RendererNode, offset: number, out: { node: RendererNode; start: number; end: number; parent: RendererNode; index: number }[]) {
  if (node.type === 'text') return;
  for (let i = 0; i < (node.children ?? []).length; i++) {
    const child = node.children![i];
    if (child.type === 'text') {
      const text = String(child.value ?? '');
      out.push({ node: child, start: offset, end: offset + text.length, parent: node, index: i });
      offset += text.length;
    } else {
      collectLeaves(child, offset, out);
      offset += extractText(child).length;
    }
  }
}

function createCodeHighlightRenderer(
  codeHighlights: { selectedText: string; threadId: string; pending?: boolean; occurrenceIndex?: number }[],
) {
  return ({ rows, stylesheet, useInlineStyles }: { rows: RendererNode[]; stylesheet: Record<string, React.CSSProperties>; useInlineStyles: boolean }) => {
    // Build full code text and collect all text leaves across all rows
    const allLeaves: { node: RendererNode; start: number; end: number; parent: RendererNode; index: number }[] = [];
    let globalOffset = 0;
    for (const row of rows) {
      collectLeaves(row, globalOffset, allLeaves);
      globalOffset += extractText(row).length;
    }
    const fullText = allLeaves.map(l => String(l.node.value ?? '')).join('');

    // Find match ranges for each highlight, respecting occurrenceIndex
    const matchRanges: { start: number; end: number; threadId: string; pending?: boolean }[] = [];
    for (const hl of codeHighlights) {
      const idx = nthIndexOf(fullText, hl.selectedText, hl.occurrenceIndex ?? 0);
      if (idx !== -1) {
        matchRanges.push({ start: idx, end: idx + hl.selectedText.length, threadId: hl.threadId, pending: hl.pending });
      }
    }

    if (matchRanges.length === 0) {
      // No matches — render normally
      return rows.map((row, i) => renderNode(row, stylesheet, useInlineStyles, i));
    }

    // For each text leaf, check if it overlaps any match range and split/wrap
    for (const range of matchRanges) {
      for (let li = allLeaves.length - 1; li >= 0; li--) {
        const leaf = allLeaves[li];
        const overlapStart = Math.max(range.start, leaf.start);
        const overlapEnd = Math.min(range.end, leaf.end);
        if (overlapStart >= overlapEnd) continue;

        const text = String(leaf.node.value ?? '');
        const localStart = overlapStart - leaf.start;
        const localEnd = overlapEnd - leaf.start;

        const newChildren: RendererNode[] = [];
        if (localStart > 0) {
          newChildren.push({ type: 'text', value: text.slice(0, localStart) });
        }
        // Wrap matching portion in a mark element
        newChildren.push({
          type: 'element',
          tagName: 'mark',
          properties: {
            className: range.pending ? ['comment-highlight', 'pending-highlight'] : ['comment-highlight'],
            'data-thread-id': range.threadId,
          },
          children: [{ type: 'text', value: text.slice(localStart, localEnd) }],
        });
        if (localEnd < text.length) {
          newChildren.push({ type: 'text', value: text.slice(localEnd) });
        }

        // Replace the text node in its parent's children array
        leaf.parent.children!.splice(leaf.index, 1, ...newChildren);
      }
    }

    return rows.map((row, i) => renderNode(row, stylesheet, useInlineStyles, i));
  };
}

interface MarkdownViewerProps {
  content: string;
  rawMarkdown: string;
  onHeadingsExtracted?: (headings: Heading[]) => void;
  className?: string;
  highlights?: ThreadHighlight[];
}

// E-PENPAL-MD-RENDER: data-source-line on blocks, heading ID slugification, mermaid containers.
// E-PENPAL-TOC: extracts h1/h2/h3 headings and passes to onHeadingsExtracted.
const MarkdownViewer = forwardRef<HTMLDivElement, MarkdownViewerProps>(
  function MarkdownViewer({ content, rawMarkdown: _rawMarkdown, onHeadingsExtracted, className, highlights }, ref) {
    const innerRef = useRef<HTMLDivElement>(null);

    // E-PENPAL-MD-STABLE-COMPONENTS: keep highlights in a ref so the code
    // component reads the latest value without being recreated on every change.
    // This prevents ReactMarkdown from unmounting/remounting custom components
    // (which would destroy externally-rendered mermaid SVGs).
    const highlightsRef = useRef<ThreadHighlight[]>([]);
    highlightsRef.current = highlights ?? [];

    // Expose the inner ref to the parent
    useImperativeHandle(ref, () => innerRef.current!, []);

    // Extract headings after render
    useEffect(() => {
      if (!innerRef.current || !onHeadingsExtracted) return;
      const headings: Heading[] = [];
      innerRef.current.querySelectorAll('h1, h2, h3').forEach((el) => {
        const level = parseInt(el.tagName[1], 10) as 1 | 2 | 3;
        const text = el.textContent || '';
        const id = el.id || generateHeadingId(text);
        if (!el.id) el.id = id;
        headings.push({ level, text, id });
      });
      onHeadingsExtracted(headings);
    }, [content, onHeadingsExtracted]);

    // Build rehype plugins array, including comment highlights when provided
    const rehypePlugins = useMemo(() => {
      const plugins: Array<[typeof rehypeRaw] | [typeof rehypeCommentHighlights, { highlights: ThreadHighlight[] }]> = [[rehypeRaw]];
      if (highlights && highlights.length > 0) {
        plugins.push([rehypeCommentHighlights, { highlights }]);
      }
      return plugins;
    }, [highlights]);

    // Custom components to generate IDs for headings and handle mermaid
    const components: Components = useMemo(
      () => ({
        h1: ({ children, node: _node, ...props }) => {
          const id = generateHeadingId(String(children));
          return <h1 id={id} {...props}>{children}</h1>;
        },
        h2: ({ children, node: _node, ...props }) => {
          const id = generateHeadingId(String(children));
          return <h2 id={id} {...props}>{children}</h2>;
        },
        h3: ({ children, node: _node, ...props }) => {
          const id = generateHeadingId(String(children));
          return <h3 id={id} {...props}>{children}</h3>;
        },
        code: ({ className: codeClassName, children, node, ...props }) => {
          const match = /language-(\w+)/.exec(codeClassName || '');
          if (match && match[1] === 'mermaid') {
            // Use AST node position to set data-source-line at render time,
            // matching how the Go template sets it server-side.
            const sourceLine = node?.position?.start?.line;
            return (
              <div
                className="mermaid-container"
                data-mermaid-source={String(children)}
                data-unwrap-pre=""
                {...(sourceLine ? { 'data-source-line': String(sourceLine) } : {})}
              >
                <pre>
                  <code>{children}</code>
                </pre>
              </div>
            );
          }
          // Fenced code block (has language class) — use SyntaxHighlighter
          if (match) {
            const sourceLine = node?.position?.start?.line;
            const codeText = String(children).replace(/\n$/, '');
            const codeLineCount = codeText.split('\n').length;

            // Find highlights targeting lines within this code block
            const codeHighlights: { selectedText: string; threadId: string; pending?: boolean; occurrenceIndex?: number }[] =
              highlightsRef.current.filter(hl => {
                if (!sourceLine) return false;
                // Code block spans from sourceLine (``` fence) to sourceLine + codeLineCount + 1 (closing ```)
                // Include fence line (>=) so anchors resolved to the opening ``` aren't silently dropped
                return hl.startLine >= sourceLine && hl.startLine <= sourceLine + codeLineCount;
              });

            // E-PENPAL-HIGHLIGHT-CROSS: cross-boundary highlights from rehype plugin
            const crossRaw = node?.properties?.dataCrossHighlights;
            if (typeof crossRaw === 'string') {
              try {
                const parsed = JSON.parse(crossRaw) as { threadId: string; selectedText: string; pending?: boolean }[];
                codeHighlights.push(...parsed);
              } catch { /* ignore malformed */ }
            }

            const rendererProp = codeHighlights.length > 0
              ? createCodeHighlightRenderer(codeHighlights)
              : undefined;

            return (
              <div data-unwrap-pre="" {...(sourceLine ? { 'data-source-line': String(sourceLine) } : {})}>
                <SyntaxHighlighter
                  style={dracula}
                  language={match[1]}
                  PreTag="div"
                  customStyle={{ margin: 0, padding: '16px', borderRadius: '6px', fontSize: '0.85em' }}
                  renderer={rendererProp as SyntaxHighlighterProps['renderer']}
                >
                  {codeText}
                </SyntaxHighlighter>
              </div>
            );
          }
          // Plain code (inline or fenced without language) — keep as-is
          // Filter out non-DOM props like 'node' from react-markdown
          const { node: _node, ...domProps } = props as Record<string, unknown>;
          return (
            <code className={codeClassName} {...domProps}>
              {children}
            </code>
          );
        },
        pre: ({ children, ...props }) => {
          // Unwrap pre when child has data-unwrap-pre (mermaid or SyntaxHighlighter)
          const child = Array.isArray(children) ? children[0] : children;
          if (child && typeof child === 'object' && 'props' in child) {
            const childProps = (child as { props?: Record<string, unknown> }).props;
            if (childProps?.['data-unwrap-pre'] !== undefined) {
              return <>{children}</>;
            }
          }
          // Filter out non-DOM props
          const { node: _node, ...domProps } = props as Record<string, unknown>;
          return <pre {...domProps}>{children}</pre>;
        },
      }),
      [], // E-PENPAL-MD-STABLE-COMPONENTS: stable references prevent mermaid SVG destruction
    );

    return (
      <div ref={innerRef} className={`content ${className || ''}`} id="content">
        <ReactMarkdown
          remarkPlugins={[remarkGfm, remarkSourceLine]}
          rehypePlugins={rehypePlugins}
          components={components}
        >
          {content}
        </ReactMarkdown>
      </div>
    );
  },
);

export default MarkdownViewer;
