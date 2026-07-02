export function getMarkdownDiagramSvgMarkup(target: EventTarget | null): string | null {
  if (!(target instanceof Element)) return null;

  const figure = target.closest('.markdown-diagram');
  if (!figure) return null;

  const svg = figure.querySelector('.markdown-pikchr-svg');
  if (!(svg instanceof SVGSVGElement)) return null;

  return svg.outerHTML;
}

export function isMarkdownDiagramActivationKey(event: KeyboardEvent): boolean {
  return event.key === 'Enter' || event.key === ' ';
}
