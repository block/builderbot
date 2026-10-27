import type { HashtagItem } from '../../types';

/**
 * Whether two hashtag items would render and resolve the same way. Only the
 * fields a consumer can observe are compared: the `type:id` key plus the
 * labels and the note revision, so an edited note still propagates while a
 * rebuilt list with the same content does not.
 */
function sameHashtagItem(a: HashtagItem, b: HashtagItem): boolean {
  return (
    a.type === b.type &&
    a.id === b.id &&
    a.title === b.title &&
    a.subtitle === b.subtitle &&
    a.noteUpdatedAt === b.noteUpdatedAt
  );
}

/**
 * Return `previous` when `next` holds the same items in the same order, else
 * `next`. Consumers such as SessionChatPane key caches on the array's
 * identity, so a host that rebuilds its list on every unrelated upstream
 * change (a git-state merge into the timeline, a cached→fresh reload with the
 * same items) should hand out the array it gave them last time.
 */
export function reuseHashtagItems(previous: HashtagItem[], next: HashtagItem[]): HashtagItem[] {
  if (previous === next) return previous;
  if (previous.length !== next.length) return next;
  for (let i = 0; i < next.length; i++) {
    if (!sameHashtagItem(previous[i], next[i])) return next;
  }
  return previous;
}
