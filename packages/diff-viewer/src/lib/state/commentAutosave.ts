import type { Comment, CommentSaveStatus } from '../types';
export type { CommentSaveStatus } from '../types';

export interface CommentAutosaveSnapshot {
  comment: Comment | null;
  status: CommentSaveStatus;
  error: unknown;
}

interface CommentAutosaveOptions {
  initialComment?: Comment | null;
  initialContent?: string;
  debounceMs?: number;
  addComment: (content: string) => Promise<Comment | null>;
  updateComment: (commentId: string, content: string) => Promise<void>;
  onChange?: (snapshot: CommentAutosaveSnapshot) => void;
}

export interface CommentAutosaveController {
  setContent(content: string): void;
  reset(comment: Comment | null, content?: string): void;
  updateExternalComment(comment: Comment): void;
  flush(): Promise<Comment | null>;
  dispose(): void;
  getSnapshot(): CommentAutosaveSnapshot;
}

const DEFAULT_DEBOUNCE_MS = 650;

function locallyUpdatedComment(comment: Comment, content: string): Comment {
  return {
    ...comment,
    content,
    githubCommentStale: comment.githubCommentId != null ? true : comment.githubCommentStale,
  };
}

export function shouldDeleteCommentOnDismiss(comment: Comment | null, content: string): boolean {
  return comment !== null && content.trim().length === 0;
}

export function createCommentAutosaveController({
  initialComment = null,
  initialContent = initialComment?.content ?? '',
  debounceMs = DEFAULT_DEBOUNCE_MS,
  addComment,
  updateComment,
  onChange,
}: CommentAutosaveOptions): CommentAutosaveController {
  let comment = initialComment;
  let content = initialContent;
  let savedContent = initialComment ? initialContent : '';
  let status: CommentSaveStatus = comment ? 'saved' : 'idle';
  let error: unknown = null;
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  let savePromise: Promise<Comment | null> | null = null;
  let disposed = false;

  function emit() {
    onChange?.({ comment, status, error });
  }

  function clearDebounce() {
    if (debounceTimer !== null) {
      clearTimeout(debounceTimer);
      debounceTimer = null;
    }
  }

  function hasPersistableContent() {
    return content.trim().length > 0;
  }

  function needsSave() {
    if (!comment && !hasPersistableContent()) return false;
    if (!comment) return true;
    return content !== savedContent;
  }

  function updateSettledStatus() {
    if (status === 'error' || status === 'saving') return;
    status = comment ? 'saved' : 'idle';
  }

  async function saveLatest(): Promise<Comment | null> {
    clearDebounce();

    if (!needsSave()) {
      updateSettledStatus();
      emit();
      return comment;
    }

    status = 'saving';
    error = null;
    emit();

    while (needsSave() && !disposed) {
      const targetContent = content;

      try {
        if (comment) {
          await updateComment(comment.id, targetContent);
          comment = locallyUpdatedComment(comment, targetContent);
        } else {
          if (!targetContent.trim()) {
            savedContent = targetContent;
            break;
          }
          const created = await addComment(targetContent);
          if (!created) {
            throw new Error('Comment was not created');
          }
          comment = created;
        }

        savedContent = targetContent;
        status = needsSave() ? 'saving' : 'saved';
        error = null;
        emit();
      } catch (e) {
        status = 'error';
        error = e;
        emit();
        return null;
      }
    }

    if (!disposed) {
      updateSettledStatus();
      emit();
    }
    return comment;
  }

  function startSave() {
    if (!savePromise) {
      savePromise = saveLatest().finally(() => {
        savePromise = null;
      });
    }
    return savePromise;
  }

  function scheduleSave() {
    clearDebounce();

    if (!needsSave()) {
      if (status !== 'saving') {
        status = comment ? 'saved' : 'idle';
        error = null;
        emit();
      }
      return;
    }

    if (status !== 'saving') {
      status = 'idle';
      error = null;
      emit();
    }

    if (savePromise) return;

    debounceTimer = setTimeout(() => {
      debounceTimer = null;
      void startSave();
    }, debounceMs);
  }

  return {
    setContent(nextContent: string) {
      content = nextContent;
      scheduleSave();
    },

    reset(nextComment: Comment | null, nextContent = nextComment?.content ?? '') {
      clearDebounce();
      comment = nextComment;
      content = nextContent;
      savedContent = nextComment ? nextContent : '';
      status = nextComment ? 'saved' : 'idle';
      error = null;
      emit();
    },

    updateExternalComment(nextComment: Comment) {
      if (comment?.id !== nextComment.id) return;
      comment = { ...nextComment, content };
      emit();
    },

    flush() {
      return startSave();
    },

    dispose() {
      disposed = true;
      clearDebounce();
    },

    getSnapshot() {
      return { comment, status, error };
    },
  };
}
