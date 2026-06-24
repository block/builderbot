declare module 'sanitize-html' {
  type AllowedAttributes = Record<string, string[]>;
  type AllowedStyles = Record<string, Record<string, RegExp[]>>;

  interface SanitizeHtmlOptions {
    allowedTags?: string[];
    allowedAttributes?: AllowedAttributes;
    allowedStyles?: AllowedStyles;
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
