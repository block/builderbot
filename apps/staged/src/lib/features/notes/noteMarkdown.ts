import { marked, Renderer, type Tokens } from 'marked';

import { sanitize } from '../../shared/sanitize';
import { getNoteDiagramFormat } from './diagramFormats';

const NOTE_MARKDOWN_RENDERER = createNoteMarkdownRenderer();

export function noteMarkdownWithTitle(title: string, content: string): string {
  const normalizedTitle = title.trim();
  if (!normalizedTitle) return content;

  const normalizedContent = content.trimStart();
  if (!normalizedContent) return `# ${normalizedTitle}`;
  if (startsWithMarkdownH1(normalizedContent)) return content;

  return `# ${normalizedTitle}\n\n${normalizedContent}`;
}

function startsWithMarkdownH1(content: string): boolean {
  return /^#[ \t]+\S/.test(content);
}

export function renderNoteMarkdown(text: string): string {
  return sanitize(
    marked.parse(text, {
      breaks: true,
      gfm: true,
      renderer: NOTE_MARKDOWN_RENDERER,
    }) as string
  );
}

function createNoteMarkdownRenderer(): Renderer {
  const renderer = new Renderer();
  const renderCode = renderer.code.bind(renderer);

  renderer.code = (token: Tokens.Code) => {
    const rendered = renderCode(token);
    const format = getNoteDiagramFormat(token.lang);
    if (!format) return rendered;

    return rendered.replace(
      '<pre>',
      `<pre class="note-diagram-source note-diagram-source-${format.language}">`
    );
  };

  return renderer;
}
