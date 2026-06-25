import type { Tokens } from 'marked';

import { sanitize } from '../../shared/sanitize';
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
    html: renderPikchrPreview(sanitize(renderedDiagramSource), renderedSvg),
    trustedHtml: true,
  };
}

function withDiagramSourceClass(renderedSource: string, format: NoteDiagramFormat): string {
  return renderedSource.replace(
    '<pre>',
    `<pre class="note-diagram-source note-diagram-source-${format.language}">`
  );
}

function renderPikchrPreview(renderedSource: string, renderedSvg: string): string {
  return [
    '<figure class="note-diagram note-diagram-pikchr">',
    '<figcaption class="note-diagram-caption">Pikchr</figcaption>',
    '<div class="note-diagram-preview note-diagram-preview-pikchr">',
    renderedSvg,
    '</div>',
    '<div class="note-diagram-source-wrap">',
    renderedSource,
    '</div>',
    '</figure>',
  ].join('');
}
