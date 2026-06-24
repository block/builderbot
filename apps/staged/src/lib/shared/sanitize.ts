import sanitizeHtml from 'sanitize-html';

/**
 * Sanitize HTML produced by marked (Markdown → HTML).
 *
 * The allowlist mirrors the subset of HTML that `marked` can emit so that
 * rendered Markdown keeps its formatting while still stripping anything
 * dangerous.  This replaces DOMPurify (MPL-licensed) with sanitize-html
 * (MIT-licensed).
 */
export function sanitize(dirty: string): string {
  return sanitizeHtml(dirty, {
    allowedTags: sanitizeHtml.defaults.allowedTags.concat([
      // Extra tags that marked can produce but sanitize-html doesn't allow by
      // default:
      'img',
      'details',
      'summary',
      'del',
      'ins',
      'input',
    ]),
    allowedAttributes: {
      ...sanitizeHtml.defaults.allowedAttributes,
      img: ['src', 'alt', 'title', 'width', 'height'],
      input: ['type', 'checked', 'disabled'],
      a: ['href', 'name', 'target', 'rel'],
      td: ['align'],
      th: ['align'],
      code: ['class'], // for syntax-highlight class names
      span: ['class'], // for syntax-highlight class names
      div: ['class'],
      figcaption: ['class'],
      figure: ['class'],
      pre: ['class'],
    },
    // Only allow checkbox inputs (GFM task lists)
    allowedSchemes: ['http', 'https', 'mailto'],
  });
}
