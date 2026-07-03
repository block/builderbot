import type { Tokens } from 'marked';

import { getMarkdownDiagramFormat, type MarkdownDiagramFormat } from './diagramFormats';
import { sanitizePikchrSvg, type PikchrRenderer } from './pikchrRendering';

export interface MarkdownDiagramRenderingOptions {
  pikchrRenderer?: PikchrRenderer | null;
}

export interface RenderedMarkdownDiagramCodeBlock {
  html: string;
  trustedHtml: boolean;
}

export function renderMarkdownDiagramCodeBlock(
  token: Tokens.Code,
  renderedSource: string,
  options: MarkdownDiagramRenderingOptions = {}
): RenderedMarkdownDiagramCodeBlock | null {
  const format = getMarkdownDiagramFormat(token.lang);
  if (!format) return null;

  const renderedDiagramSource = withDiagramSourceClass(renderedSource, format);
  const renderedPikchr = options.pikchrRenderer?.(token.text);
  if (!renderedPikchr || renderedPikchr.kind !== 'svg') {
    return { html: renderedDiagramSource, trustedHtml: false };
  }
  const renderedSvg = sanitizePikchrSvg(renderedPikchr.svg, { source: token.text });
  if (!renderedSvg) {
    return { html: renderedDiagramSource, trustedHtml: false };
  }

  return {
    html: renderPikchrPreview(renderedSvg),
    trustedHtml: true,
  };
}

function withDiagramSourceClass(renderedSource: string, format: MarkdownDiagramFormat): string {
  return renderedSource.replace(
    '<pre>',
    `<pre class="markdown-diagram-source markdown-diagram-source-${format.language}">`
  );
}

function renderPikchrPreview(renderedSvg: string): string {
  return [
    '<figure class="markdown-diagram markdown-diagram-pikchr" role="button" tabindex="0" aria-label="Open diagram full screen" title="Open diagram full screen">',
    '<div class="markdown-diagram-preview markdown-diagram-preview-pikchr">',
    renderedSvg,
    '</div>',
    '</figure>',
  ].join('');
}
