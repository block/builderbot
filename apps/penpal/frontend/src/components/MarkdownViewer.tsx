import { useEffect, useRef, useMemo, forwardRef, useImperativeHandle } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeRaw from 'rehype-raw';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { dracula as prismDracula } from 'react-syntax-highlighter/dist/esm/styles/prism';
import type { Components } from 'react-markdown';
import type { Heading } from './TableOfContents';
import remarkSourceLine from './remarkSourceLine';
import rehypeCommentHighlights from './rehypeCommentHighlights';
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
            return (
              <div data-unwrap-pre="" {...(sourceLine ? { 'data-source-line': String(sourceLine) } : {})}>
                <SyntaxHighlighter
                  style={dracula}
                  language={match[1]}
                  PreTag="div"
                  customStyle={{ margin: 0, padding: '16px', borderRadius: '6px', fontSize: '0.85em' }}
                >
                  {String(children).replace(/\n$/, '')}
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
      [],
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
