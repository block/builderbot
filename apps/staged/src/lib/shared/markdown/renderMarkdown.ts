import { marked, Renderer, type Tokens } from 'marked';

import { sanitize } from '../sanitize';
import {
  renderMarkdownDiagramCodeBlock,
  type MarkdownDiagramRenderingOptions,
} from './diagramRendering';

export type MarkdownRenderingOptions = MarkdownDiagramRenderingOptions & {
  renderInlineText?: (text: string) => string;
};

interface TrustedHtmlReplacement {
  placeholder: string;
  html: string;
}

let fallbackPlaceholderSequence = 0;

export function renderMarkdown(text: string, options: MarkdownRenderingOptions = {}): string {
  const trustedHtml: TrustedHtmlReplacement[] = [];
  const renderedMarkdown = sanitize(
    marked.parse(text, {
      breaks: true,
      gfm: true,
      renderer: createMarkdownRenderer(options, trustedHtml),
    }) as string
  );

  return restoreTrustedHtml(renderedMarkdown, trustedHtml);
}

function createMarkdownRenderer(
  options: MarkdownRenderingOptions,
  trustedHtml: TrustedHtmlReplacement[]
): Renderer {
  const renderer = new Renderer();
  const renderCode = renderer.code.bind(renderer);
  const renderText = renderer.text.bind(renderer);

  renderer.code = (token: Tokens.Code) => {
    const rendered = renderCode(token);
    const diagram = renderMarkdownDiagramCodeBlock(token, rendered, options);
    if (!diagram) return rendered;
    if (!diagram.trustedHtml) return diagram.html;

    return stashTrustedHtml(diagram.html, trustedHtml);
  };

  renderer.text = (token: Tokens.Text) => {
    if (!options.renderInlineText) return renderText(token);
    if ('tokens' in token && token.tokens) return renderText(token);
    if ('escaped' in token && token.escaped) return renderText(token);
    return options.renderInlineText(token.text);
  };

  return renderer;
}

function stashTrustedHtml(html: string, trustedHtml: TrustedHtmlReplacement[]): string {
  const placeholder = `STAGED_MARKDOWN_TRUSTED_DIAGRAM_${trustedHtml.length}_${createPlaceholderNonce()}`;
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
