/** One file the image store refused while attaching a batch. */
export interface AttachRejection {
  name: string;
  reason: string;
}

/**
 * File-picker `accept` list for the image store. A PDF is accepted alongside
 * images: it is stored as a PDF, previewed through a rendering of its first
 * page, and opened in the native PDF viewer.
 */
export const ATTACHMENT_ACCEPT = 'image/png,image/jpeg,image/gif,image/webp,application/pdf';

/** Whether a pasted clipboard item can be attached through the image store. */
export function isAttachableClipboardType(type: string): boolean {
  return type.startsWith('image/') || type === 'application/pdf';
}

export function attachErrorReason(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** The file's basename, for a path dropped from the OS. */
export function attachBasename(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/**
 * One message for a batch of attachments (picker, paste, drop), so a later
 * success in the batch cannot hide an earlier rejection. Lists every rejected
 * name when more than one file failed.
 */
export function formatAttachErrors(rejections: AttachRejection[]): string | null {
  if (rejections.length === 0) return null;
  if (rejections.length === 1) {
    const [{ name, reason }] = rejections;
    return `Could not attach ${name}: ${reason}`;
  }
  const names = rejections.map((r) => r.name).join(', ');
  const reasons = [...new Set(rejections.map((r) => r.reason))];
  if (reasons.length === 1) {
    return `Could not attach ${rejections.length} files (${names}): ${reasons[0]}`;
  }
  const detail = rejections.map((r) => `${r.name} (${r.reason})`).join('; ');
  return `Could not attach ${rejections.length} files: ${detail}`;
}
