export function noteMarkdownWithTitle(title: string, content: string): string {
  const normalizedTitle = title.trim();
  if (!normalizedTitle) return content;

  const normalizedContent = content.trimStart();
  if (!normalizedContent) return `# ${normalizedTitle}`;
  if (startsWithMarkdownH1(normalizedContent)) return content;

  return `# ${normalizedTitle}\n\n${normalizedContent}`;
}

function startsWithMarkdownH1(content: string): boolean {
  return /^#[ \t]+\S/.test(content);
}
