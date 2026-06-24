import { marked, Renderer, type Tokens } from 'marked';

import { sanitize } from '../../shared/sanitize';
import { renderNoteDiagramCodeBlock, type NoteDiagramRenderingOptions } from './diagramRendering';

interface TrustedHtmlReplacement {
  placeholder: string;
  html: string;
}

let fallbackPlaceholderSequence = 0;

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

export function renderNoteMarkdown(
  text: string,
  options: NoteDiagramRenderingOptions = {}
): string {
  const trustedHtml: TrustedHtmlReplacement[] = [];
  const renderedMarkdown = sanitize(
    marked.parse(text, {
      breaks: true,
      gfm: true,
      renderer: createNoteMarkdownRenderer(options, trustedHtml),
    }) as string
  );

  return restoreTrustedHtml(renderedMarkdown, trustedHtml);
}

function createNoteMarkdownRenderer(
  options: NoteDiagramRenderingOptions,
  trustedHtml: TrustedHtmlReplacement[]
): Renderer {
  const renderer = new Renderer();
  const renderCode = renderer.code.bind(renderer);

  renderer.code = (token: Tokens.Code) => {
    const rendered = renderCode(token);
    const diagram = renderNoteDiagramCodeBlock(token, rendered, options);
    if (!diagram) return rendered;
    if (!diagram.trustedHtml) return diagram.html;

    return stashTrustedHtml(diagram.html, trustedHtml);
  };

  return renderer;
}

function stashTrustedHtml(html: string, trustedHtml: TrustedHtmlReplacement[]): string {
  const placeholder = `STAGED_NOTE_TRUSTED_DIAGRAM_${trustedHtml.length}_${createPlaceholderNonce()}`;
  trustedHtml.push({ placeholder, html });
  return placeholder;
}

function restoreTrustedHtml(
  renderedMarkdown: string,
  trustedHtml: TrustedHtmlReplacement[]
): string {
  return trustedHtml.reduce((html, replacement) => {
    return html.replaceAll(replacement.placeholder, replacement.html);
  }, renderedMarkdown);
}

function createPlaceholderNonce(): string {
  const randomUuid = globalThis.crypto?.randomUUID?.();
  if (randomUuid) return randomUuid.replaceAll('-', '_');

  return `${Date.now().toString(36)}_${fallbackPlaceholderSequence++}`;
}
