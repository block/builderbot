import { useEffect, useRef, useMemo, forwardRef, useImperativeHandle } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeRaw from 'rehype-raw';
import type { Components } from 'react-markdown';
import type { Heading } from './TableOfContents';

interface MarkdownViewerProps {
  content: string;
  rawMarkdown: string;
  onHeadingsExtracted?: (headings: Heading[]) => void;
  className?: string;
}

const MarkdownViewer = forwardRef<HTMLDivElement, MarkdownViewerProps>(
  function MarkdownViewer({ content, rawMarkdown, onHeadingsExtracted, className }, ref) {
    const innerRef = useRef<HTMLDivElement>(null);

    // Expose the inner ref to the parent
    useImperativeHandle(ref, () => innerRef.current!, []);

    // Pre-process raw markdown to compute source line mapping
    const sourceLineData = useMemo(() => {
      const lines = rawMarkdown.split('\n');
      const blockLines: number[] = [];
      let inFence = false;

      for (let i = 0; i < lines.length; i++) {
        const trimmed = lines[i].trim();
        if (trimmed.startsWith('```')) {
          if (!inFence) {
            blockLines.push(i + 1);
            inFence = true;
          } else {
            inFence = false;
          }
          continue;
        }
        if (inFence) continue;
        if (/^#{1,6}\s/.test(trimmed)) {
          blockLines.push(i + 1);
          continue;
        }
        if (/^[-*+]\s|^\d+\.\s/.test(trimmed)) {
          blockLines.push(i + 1);
          continue;
        }
        if (trimmed.startsWith('>')) {
          blockLines.push(i + 1);
          continue;
        }
        if (/^[-*_]{3,}\s*$/.test(trimmed)) {
          blockLines.push(i + 1);
          continue;
        }
        if (trimmed.length > 0 && (i === 0 || lines[i - 1].trim() === '')) {
          blockLines.push(i + 1);
        }
      }
      return blockLines;
    }, [rawMarkdown]);

    // Extract headings after render
    useEffect(() => {
      if (!innerRef.current || !onHeadingsExtracted) return;
      const headings: Heading[] = [];
      innerRef.current.querySelectorAll('h1, h2, h3').forEach((el) => {
        const level = parseInt(el.tagName[1], 10) as 1 | 2 | 3;
        const text = el.textContent || '';
        const id =
          el.id || text.toLowerCase().replace(/[^\w]+/g, '-').replace(/(^-|-$)/g, '');
        if (!el.id) el.id = id;
        headings.push({ level, text, id });
      });
      onHeadingsExtracted(headings);
    }, [content, onHeadingsExtracted]);

    // After markdown renders, add data-source-line attributes to block elements
    useEffect(() => {
      if (!innerRef.current) return;
      const el = innerRef.current;
      let blockIdx = 0;

      const blockElements = el.querySelectorAll(
        ':scope > p, :scope > h1, :scope > h2, :scope > h3, :scope > h4, :scope > h5, :scope > h6, :scope > ul > li, :scope > ol > li, :scope > blockquote, :scope > pre, :scope > hr, :scope > table, :scope > div.mermaid-container',
      );

      blockElements.forEach((blockEl) => {
        if (blockIdx < sourceLineData.length) {
          blockEl.setAttribute('data-source-line', String(sourceLineData[blockIdx]));
          blockIdx++;
        }
      });
    }, [content, sourceLineData]);

    // Custom components to generate IDs for headings and handle mermaid
    const components: Components = useMemo(
      () => ({
        h1: ({ children, ...props }) => {
          const text = String(children);
          const id = text.toLowerCase().replace(/[^\w]+/g, '-').replace(/(^-|-$)/g, '');
          return (
            <h1 id={id} {...props}>
              {children}
            </h1>
          );
        },
        h2: ({ children, ...props }) => {
          const text = String(children);
          const id = text.toLowerCase().replace(/[^\w]+/g, '-').replace(/(^-|-$)/g, '');
          return (
            <h2 id={id} {...props}>
              {children}
            </h2>
          );
        },
        h3: ({ children, ...props }) => {
          const text = String(children);
          const id = text.toLowerCase().replace(/[^\w]+/g, '-').replace(/(^-|-$)/g, '');
          return (
            <h3 id={id} {...props}>
              {children}
            </h3>
          );
        },
        code: ({ className: codeClassName, children, ...props }) => {
          const match = /language-(\w+)/.exec(codeClassName || '');
          if (match && match[1] === 'mermaid') {
            return (
              <div
                className="mermaid-container"
                data-mermaid-source={String(children)}
              >
                <pre>
                  <code>{children}</code>
                </pre>
              </div>
            );
          }
          return (
            <code className={codeClassName} {...props}>
              {children}
            </code>
          );
        },
        pre: ({ children, ...props }) => {
          // Check if the child is a mermaid container — unwrap the pre
          const child = Array.isArray(children) ? children[0] : children;
          if (
            child &&
            typeof child === 'object' &&
            'props' in child &&
            (child as { props?: Record<string, unknown> }).props?.['data-mermaid-source'] !==
              undefined
          ) {
            return <>{children}</>;
          }
          return <pre {...props}>{children}</pre>;
        },
      }),
      [],
    );

    return (
      <div ref={innerRef} className={`content ${className || ''}`} id="content">
        <ReactMarkdown
          remarkPlugins={[remarkGfm]}
          rehypePlugins={[rehypeRaw]}
          components={components}
        >
          {content}
        </ReactMarkdown>
      </div>
    );
  },
);

export default MarkdownViewer;
