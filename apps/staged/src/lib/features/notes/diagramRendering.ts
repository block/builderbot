import type { Tokens } from 'marked';

import { getNoteDiagramFormat, type NoteDiagramFormat } from './diagramFormats';
import { sanitizePikchrSvg, type NotePikchrRenderer } from './pikchrRendering';

export interface NoteDiagramRenderingOptions {
  pikchrRenderer?: NotePikchrRenderer | null;
}

export interface RenderedNoteDiagramCodeBlock {
  html: string;
  trustedHtml: boolean;
}

export function renderNoteDiagramCodeBlock(
  token: Tokens.Code,
  renderedSource: string,
  options: NoteDiagramRenderingOptions = {}
): RenderedNoteDiagramCodeBlock | null {
  const format = getNoteDiagramFormat(token.lang);
  if (!format) return null;

  const renderedDiagramSource = withDiagramSourceClass(renderedSource, format);
  const renderedPikchr = options.pikchrRenderer?.(token.text);
  if (!renderedPikchr || renderedPikchr.kind !== 'svg') {
    return { html: renderedDiagramSource, trustedHtml: false };
  }
  const renderedSvg = sanitizePikchrSvg(renderedPikchr.svg);
  if (!renderedSvg) {
    return { html: renderedDiagramSource, trustedHtml: false };
  }

  return {
    html: renderPikchrPreview(renderedSvg),
    trustedHtml: true,
  };
}

function withDiagramSourceClass(renderedSource: string, format: NoteDiagramFormat): string {
  return renderedSource.replace(
    '<pre>',
    `<pre class="note-diagram-source note-diagram-source-${format.language}">`
  );
}

function renderPikchrPreview(renderedSvg: string): string {
  return [
    '<figure class="note-diagram note-diagram-pikchr">',
    '<div class="note-diagram-preview note-diagram-preview-pikchr">',
    renderedSvg,
    '</div>',
    '</figure>',
  ].join('');
}
