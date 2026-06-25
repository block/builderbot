import type { Pikchr } from 'pikchr-js';
import sanitizeHtml from 'sanitize-html';

const PIKCHR_SVG_CLASS = 'markdown-pikchr-svg';
const MAX_PIKCHR_SOURCE_LENGTH = 20_000;
const MAX_PIKCHR_SVG_LENGTH = 250_000;
const PIKCHR_SVG_ROOT = /^\s*<svg[\s>]/i;
const PIKCHR_SIDE_LABEL_GAP = '0.35em';

const CSS_NUMBER = String.raw`[-+]?(?:\d+(?:\.\d+)?|\.\d+)(?:e[-+]?\d+)?`;
const CSS_LENGTH = new RegExp(`^(?:${CSS_NUMBER})(?:px|pt|pc|mm|cm|in|em|rem|%)?$`);
const CSS_COLOR =
  /^(?:none|transparent|currentColor|[a-zA-Z]+|#[0-9a-fA-F]{3,8}|rgba?\(\s*\d{1,3}\s*,\s*\d{1,3}\s*,\s*\d{1,3}(?:\s*,\s*(?:0|1|0?\.\d+))?\s*\))$/;
const CSS_DASH_ARRAY = new RegExp(`^(?:${CSS_NUMBER})(?:\\s*,\\s*(?:${CSS_NUMBER}))*$`);
const DIRECT_COLOR_ATTRIBUTES = ['fill', 'stroke'] as const;
const DIRECT_COLOR_ATTRIBUTE_TAGS = new Set([
  'path',
  'text',
  'rect',
  'circle',
  'ellipse',
  'line',
  'polyline',
  'polygon',
]);

export type RenderedPikchrDiagram =
  | {
      kind: 'svg';
      svg: string;
      width: number;
      height: number;
    }
  | {
      kind: 'error';
      message: string;
    };

export type PikchrRenderer = (source: string) => RenderedPikchrDiagram;

let rendererPromise: Promise<PikchrRenderer> | null = null;

export function loadPikchrRenderer(): Promise<PikchrRenderer> {
  rendererPromise ??= import('pikchr-js')
    .then(({ default: loadPikchr }) => loadPikchr())
    .then((pikchr) => {
      return (source: string) => renderPikchrSource(pikchr, source);
    })
    .catch((error) => {
      rendererPromise = null;
      throw error;
    });
  return rendererPromise;
}

export function renderPikchrSource(pikchr: Pikchr, source: string): RenderedPikchrDiagram {
  if (source.length > MAX_PIKCHR_SOURCE_LENGTH) {
    return { kind: 'error', message: 'Pikchr source is too large to render safely.' };
  }

  try {
    const rendered = pikchr.render(source, PIKCHR_SVG_CLASS);
    if (rendered.width < 0 || rendered.height < 0 || !PIKCHR_SVG_ROOT.test(rendered.svg)) {
      return { kind: 'error', message: 'Pikchr could not render this diagram.' };
    }

    const svg = sanitizePikchrSvg(rendered.svg);
    if (!svg) {
      return { kind: 'error', message: 'Pikchr rendered unsafe SVG.' };
    }

    return {
      kind: 'svg',
      svg,
      width: rendered.width,
      height: rendered.height,
    };
  } catch {
    return { kind: 'error', message: 'Pikchr could not render this diagram.' };
  }
}

export function sanitizePikchrSvg(svg: string): string | null {
  if (svg.length > MAX_PIKCHR_SVG_LENGTH || !PIKCHR_SVG_ROOT.test(svg)) {
    return null;
  }

  const sanitized = sanitizeHtml(svg, {
    allowedTags: [
      'svg',
      'g',
      'path',
      'text',
      'rect',
      'circle',
      'ellipse',
      'line',
      'polyline',
      'polygon',
    ],
    allowedAttributes: {
      svg: ['xmlns', 'viewBox', 'viewbox', 'class', 'style', 'width', 'height'],
      g: ['class', 'transform', 'style'],
      path: ['class', 'd', 'fill', 'stroke', 'style'],
      text: [
        'class',
        'x',
        'y',
        'dx',
        'dy',
        'text-anchor',
        'dominant-baseline',
        'fill',
        'stroke',
        'style',
      ],
      rect: ['class', 'x', 'y', 'width', 'height', 'rx', 'ry', 'fill', 'stroke', 'style'],
      circle: ['class', 'cx', 'cy', 'r', 'fill', 'stroke', 'style'],
      ellipse: ['class', 'cx', 'cy', 'rx', 'ry', 'fill', 'stroke', 'style'],
      line: ['class', 'x1', 'y1', 'x2', 'y2', 'fill', 'stroke', 'style'],
      polyline: ['class', 'points', 'fill', 'stroke', 'style'],
      polygon: ['class', 'points', 'fill', 'stroke', 'style'],
    },
    allowedStyles: {
      '*': {
        fill: [CSS_COLOR],
        stroke: [CSS_COLOR],
        'stroke-width': [CSS_LENGTH],
        'stroke-dasharray': [CSS_DASH_ARRAY],
        'font-size': [/^initial$/, CSS_LENGTH],
      },
    },
    allowedSchemes: [],
    transformTags: {
      '*': normalizePikchrSvgAttributes,
    },
  });

  const normalized = sanitized.replace(/\sviewbox=/g, ' viewBox=');
  if (!PIKCHR_SVG_ROOT.test(normalized)) return null;
  return normalized;
}

function normalizePikchrSvgAttributes(tagName: string, attribs: Record<string, string>) {
  const colorNormalized = stripUnsafeDirectColorAttributes(tagName, attribs);
  if (tagName.toLowerCase() !== 'text') return colorNormalized;

  return {
    tagName,
    attribs: addSideLabelGap(colorNormalized.attribs),
  };
}

function addSideLabelGap(attribs: Record<string, string>) {
  if (attribs.dx !== undefined) return attribs;

  const anchor = attribs['text-anchor']?.trim().toLowerCase();
  if (anchor !== 'start' && anchor !== 'end') return attribs;

  return {
    ...attribs,
    dx: anchor === 'start' ? PIKCHR_SIDE_LABEL_GAP : `-${PIKCHR_SIDE_LABEL_GAP}`,
  };
}

function stripUnsafeDirectColorAttributes(tagName: string, attribs: Record<string, string>) {
  const nextAttribs = { ...attribs };
  if (!DIRECT_COLOR_ATTRIBUTE_TAGS.has(tagName.toLowerCase())) {
    return { tagName, attribs: nextAttribs };
  }

  for (const attribute of DIRECT_COLOR_ATTRIBUTES) {
    const value = nextAttribs[attribute];
    if (value === undefined) continue;

    const trimmedValue = value.trim();
    if (CSS_COLOR.test(trimmedValue)) {
      nextAttribs[attribute] = trimmedValue;
    } else {
      delete nextAttribs[attribute];
    }
  }

  return { tagName, attribs: nextAttribs };
}
