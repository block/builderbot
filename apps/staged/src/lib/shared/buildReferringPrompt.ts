/**
 * Build a prompt string that references a timeline item via hashtag.
 *
 * If there's already user-entered text, appends the reference on a new line.
 * Otherwise, starts a fresh "Re: #..." prompt.
 */
export function buildReferringPrompt(existing: string, ref: string): string {
  if (existing.trim()) {
    return existing.trimEnd() + '\n' + ref;
  }
  return `Re: ${ref}\n`;
}
