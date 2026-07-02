import type { Pikchr } from 'pikchr-js';
import sanitizeHtml from 'sanitize-html';

const PIKCHR_SVG_CLASS = 'markdown-pikchr-svg';
const MAX_PIKCHR_SOURCE_LENGTH = 20_000;
const MAX_PIKCHR_SVG_LENGTH = 250_000;
const PIKCHR_SVG_ROOT = /^\s*<svg[\s>]/i;
const PIKCHR_SIDE_LABEL_GAP = '0.35em';

const CSS_NUMBER = String.raw`[-+]?(?:\d+(?:\.\d+)?|\.\d+)(?:e[-+]?\d+)?`;
const CSS_LENGTH = new RegExp(`^(?:${CSS_NUMBER})(?:px|pt|pc|mm|cm|in|em|rem|%)?$`);
const PIKCHR_THEME_COLOR_VAR_NAMES = [
  '--pikchr-ink',
  '--pikchr-surface',
  '--pikchr-muted',
  '--pikchr-red',
  '--pikchr-green',
  '--pikchr-blue',
  '--pikchr-yellow',
  '--pikchr-orange',
  '--pikchr-purple',
  '--pikchr-cyan',
] as const;
const PIKCHR_THEME_COLOR_VAR_PATTERN = String.raw`var\((?:${PIKCHR_THEME_COLOR_VAR_NAMES.join('|')})\)`;
const CSS_COLOR = new RegExp(
  String.raw`^(?:none|transparent|currentColor|[a-zA-Z]+|#[0-9a-fA-F]{3,8}|rgba?\(\s*\d{1,3}\s*,\s*\d{1,3}\s*,\s*\d{1,3}(?:\s*,\s*(?:0|1|0?\.\d+))?\s*\)|${PIKCHR_THEME_COLOR_VAR_PATTERN})$`,
  'i'
);
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
const RGB_COLOR = /^rgb\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})\s*\)$/i;
const HEX_COLOR = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i;
const PIKCHR_NUMERIC_COLOR_LITERAL = /\b0x([0-9a-fA-F]{6})\b/g;
const THEMED_PIKCHR_COLORS = new Map([
  ['#000000', 'var(--pikchr-ink)'],
  ['#ffffff', 'var(--pikchr-surface)'],
  ['#808080', 'var(--pikchr-muted)'],
  ['#a9a9a9', 'var(--pikchr-muted)'],
  ['#c0c0c0', 'var(--pikchr-muted)'],
  ['#696969', 'var(--pikchr-muted)'],
  ['#ff0000', 'var(--pikchr-red)'],
  ['#dc143c', 'var(--pikchr-red)'],
  ['#008000', 'var(--pikchr-green)'],
  ['#00ff00', 'var(--pikchr-green)'],
  ['#0000ff', 'var(--pikchr-blue)'],
  ['#ffff00', 'var(--pikchr-yellow)'],
  ['#ffd700', 'var(--pikchr-yellow)'],
  ['#ffa500', 'var(--pikchr-orange)'],
  ['#800080', 'var(--pikchr-purple)'],
  ['#ff00ff', 'var(--pikchr-purple)'],
  ['#ee82ee', 'var(--pikchr-purple)'],
  ['#00ffff', 'var(--pikchr-cyan)'],
  ['#008080', 'var(--pikchr-cyan)'],
]);
const NAMED_PIKCHR_COLOR_KEYS = new Map([
  ['black', '#000000'],
  ['white', '#ffffff'],
  ['gray', '#808080'],
  ['grey', '#808080'],
  ['darkgray', '#a9a9a9'],
  ['darkgrey', '#a9a9a9'],
  ['silver', '#c0c0c0'],
  ['dimgray', '#696969'],
  ['dimgrey', '#696969'],
  ['red', '#ff0000'],
  ['crimson', '#dc143c'],
  ['green', '#008000'],
  ['lime', '#00ff00'],
  ['blue', '#0000ff'],
  ['yellow', '#ffff00'],
  ['gold', '#ffd700'],
  ['orange', '#ffa500'],
  ['purple', '#800080'],
  ['fuchsia', '#ff00ff'],
  ['magenta', '#ff00ff'],
  ['violet', '#ee82ee'],
  ['cyan', '#00ffff'],
  ['aqua', '#00ffff'],
  ['teal', '#008080'],
]);

interface SanitizePikchrSvgOptions {
  source?: string;
}

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

    const svg = sanitizePikchrSvg(rendered.svg, { source });
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

export function sanitizePikchrSvg(
  svg: string,
  options: SanitizePikchrSvgOptions = {}
): string | null {
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
  return applyThemedPikchrPalette(normalized, options.source);
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

function applyThemedPikchrPalette(svg: string, source: string | undefined): string {
  const preservedColors = collectPreservedPikchrColorKeys(source);
  return svg
    .replace(/\b(fill|stroke)="([^"]*)"/gi, (_match, attribute: string, value: string) => {
      return `${attribute}="${themePikchrColor(value, preservedColors)}"`;
    })
    .replace(/\bstyle="([^"]*)"/gi, (_match, style: string) => {
      return `style="${themePikchrStyle(style, preservedColors)}"`;
    });
}

function themePikchrStyle(style: string, preservedColors: Set<string>): string {
  return style.replace(
    /(^|;)\s*(fill|stroke)\s*:\s*([^;]+)/gi,
    (_match, prefix: string, property: string, value: string) => {
      return `${prefix}${property}:${themePikchrColor(value, preservedColors)}`;
    }
  );
}

function themePikchrColor(value: string, preservedColors: Set<string>): string {
  const trimmedValue = value.trim();
  const colorKey = canonicalColorKey(trimmedValue);
  if (!colorKey || preservedColors.has(colorKey)) return trimmedValue;

  return THEMED_PIKCHR_COLORS.get(colorKey) ?? trimmedValue;
}

function collectPreservedPikchrColorKeys(source: string | undefined): Set<string> {
  const preservedColors = new Set<string>();
  if (!source) return preservedColors;

  const sourceWithoutLabels = source.replace(/"[^"]*(?:"|$)/g, ' ');
  for (const match of sourceWithoutLabels.matchAll(PIKCHR_NUMERIC_COLOR_LITERAL)) {
    preservedColors.add(`#${match[1].toLowerCase()}`);
  }

  return preservedColors;
}

function canonicalColorKey(value: string): string | null {
  const normalizedValue = value.toLowerCase();
  const namedColor = NAMED_PIKCHR_COLOR_KEYS.get(normalizedValue);
  if (namedColor) return namedColor;

  const rgb = RGB_COLOR.exec(normalizedValue);
  if (rgb) return colorKeyFromRgb(Number(rgb[1]), Number(rgb[2]), Number(rgb[3]));

  const hex = HEX_COLOR.exec(normalizedValue);
  if (!hex) return null;

  const digits = hex[1];
  if (digits.length === 3) {
    return `#${digits
      .split('')
      .map((digit) => `${digit}${digit}`)
      .join('')}`;
  }
  return `#${digits}`;
}

function colorKeyFromRgb(r: number, g: number, b: number): string | null {
  if ([r, g, b].some((component) => component < 0 || component > 255)) return null;

  return `#${[r, g, b].map((component) => component.toString(16).padStart(2, '0')).join('')}`;
}
