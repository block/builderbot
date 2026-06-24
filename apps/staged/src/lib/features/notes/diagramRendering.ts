import type { Tokens } from 'marked';

import { getNoteDiagramFormat, type NoteDiagramFormat } from './diagramFormats';

export function renderNoteDiagramCodeBlock(
  token: Tokens.Code,
  renderedSource: string
): string | null {
  const format = getNoteDiagramFormat(token.lang);
  if (!format) return null;

  const renderedDiagramSource = withDiagramSourceClass(renderedSource, format);
  if (format.language !== 'pikchr') return renderedDiagramSource;

  return renderPikchrPreview(renderedDiagramSource);
}

function withDiagramSourceClass(renderedSource: string, format: NoteDiagramFormat): string {
  return renderedSource.replace(
    '<pre>',
    `<pre class="note-diagram-source note-diagram-source-${format.language}">`
  );
}

function renderPikchrPreview(renderedSource: string): string {
  return [
    '<figure class="note-diagram note-diagram-pikchr">',
    '<figcaption class="note-diagram-caption">Pikchr</figcaption>',
    '<div class="note-diagram-preview note-diagram-preview-pikchr">',
    renderedSource,
    '</div>',
    '</figure>',
  ].join('');
}
