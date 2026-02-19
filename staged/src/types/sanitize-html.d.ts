declare module 'sanitize-html' {
  type AllowedAttributes = Record<string, string[]>;

  interface SanitizeHtmlOptions {
    allowedTags?: string[];
    allowedAttributes?: AllowedAttributes;
    allowedSchemes?: string[];
  }

  interface SanitizeHtmlFn {
    (dirty: string, options?: SanitizeHtmlOptions): string;
    defaults: {
      allowedTags: string[];
      allowedAttributes: AllowedAttributes;
    };
  }

  const sanitizeHtml: SanitizeHtmlFn;
  export default sanitizeHtml;
}
