declare module 'sanitize-html' {
  type AllowedAttributes = Record<string, string[]>;
  type AllowedStyles = Record<string, Record<string, RegExp[]>>;
  type TagAttributes = Record<string, string>;
  type TransformTag = (
    tagName: string,
    attribs: TagAttributes
  ) => { tagName: string; attribs: TagAttributes; text?: string };

  interface SanitizeHtmlOptions {
    allowedTags?: string[];
    allowedAttributes?: AllowedAttributes;
    allowedStyles?: AllowedStyles;
    allowedSchemes?: string[];
    transformTags?: Record<string, TransformTag>;
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
