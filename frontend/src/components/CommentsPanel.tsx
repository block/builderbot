import { useState, useCallback, useRef, useEffect } from 'react';
import type { ThreadResponse, Anchor } from '../types';
import { api } from '../api';
import { orderComments, formatTime, truncateText } from '../utils/comments';

interface CommentsPanelProps {
  threads: ThreadResponse[];
  anchorLines: Record<string, number>;
  project: string;
  filePath: string;
  onRefresh: () => void;
  onThreadFocus?: (threadId: string, line: number) => void;
  agentStatus?: { running: boolean; contextPercent?: number } | null;
  pendingAnchor?: Anchor | null;
  pendingText?: string;
  onCancelNewThread?: () => void;
}

function getSavedAuthor(): string {
  return localStorage.getItem('penpal-author') || '';
}

function saveAuthor(name: string) {
  if (name) localStorage.setItem('penpal-author', name);
}

function renderCommentBody(text: string): string {
  if (!text) return '';
  // Simple markdown-to-HTML: bold, italic, code, links, line breaks
  let html = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
    .replace(/\*([^*]+)\*/g, '<em>$1</em>')
    .replace(/\n/g, '<br>');
  // Wrap in paragraph
  return `<p>${html}</p>`;
}

function agentContextColorClass(pct: number): string {
  if (pct >= 85) return 'critical';
  if (pct >= 60) return 'warning';
  return '';
}

export default function CommentsPanel({
  threads,
  anchorLines,
  project,
  filePath,
  onRefresh,
  onThreadFocus,
  agentStatus,
  pendingAnchor,
  pendingText,
  onCancelNewThread,
}: CommentsPanelProps) {
  const [showResolved, setShowResolved] = useState(false);
  const [replyFormThread, setReplyFormThread] = useState<string | null>(null);
  const [highlightedThread, setHighlightedThread] = useState<string | null>(null);

  // Categorize threads
  const openThreads: ThreadResponse[] = [];
  const resolvedThreads: ThreadResponse[] = [];
  const orphanedThreads: ThreadResponse[] = [];

  (threads || []).forEach((t) => {
    const line = anchorLines[t.id];
    if (t.status === 'resolved') resolvedThreads.push(t);
    else if (line === -1 || line === undefined) orphanedThreads.push(t);
    else openThreads.push(t);
  });

  const sortByLine = (a: ThreadResponse, b: ThreadResponse) =>
    (anchorLines[a.id] || 0) - (anchorLines[b.id] || 0);
  openThreads.sort(sortByLine);
  resolvedThreads.sort(sortByLine);

  const openCount = openThreads.length + orphanedThreads.length;

  const handleThreadClick = useCallback(
    (threadId: string) => {
      const line = anchorLines[threadId];
      setHighlightedThread(threadId);
      onThreadFocus?.(threadId, line);
      setTimeout(() => setHighlightedThread(null), 3000);
    },
    [anchorLines, onThreadFocus],
  );

  const ensureAgent = useCallback(() => {
    api.startAgent(project).catch(() => {});
  }, [project]);

  return (
    <div className="comments-panel" id="comments-panel">
      <div className="comments-panel-header">
        <span>
          Comments {openCount > 0 && <span id="comments-count">{openCount}</span>}
        </span>
        {agentStatus?.running && (
          <span className="agent-indicator">
            <span className="agent-dot" />
            <span>Agent</span>
            {agentStatus.contextPercent !== undefined && (
              <span
                className={`agent-context-label ${agentContextColorClass(agentStatus.contextPercent)}`}
              >
                {Math.round(agentStatus.contextPercent)}%
              </span>
            )}
            <button
              className="agent-stop-btn"
              onClick={(e) => {
                e.stopPropagation();
                api.stopAgent(project).then(onRefresh);
              }}
            >
              Stop
            </button>
          </span>
        )}
        {resolvedThreads.length > 0 && (
          <span id="resolved-toggle">
            <label style={{ cursor: 'pointer', fontWeight: 'normal', textTransform: 'none', letterSpacing: 'normal' }}>
              <input
                type="checkbox"
                checked={showResolved}
                onChange={() => setShowResolved(!showResolved)}
                style={{ marginRight: 3, verticalAlign: 'middle' }}
              />
              <span>{resolvedThreads.length} resolved</span>
            </label>
          </span>
        )}
      </div>
      <div className="comments-panel-scroll">
        {/* Review banner */}
        {openCount > 0 && (
          <div className={`review-banner${agentStatus?.running ? ' agent-active' : ''}`}>
            <strong>In review</strong> &mdash; {openCount} open thread
            {openCount !== 1 ? 's' : ''}
          </div>
        )}

        {/* New thread form */}
        {pendingAnchor && (
          <NewThreadForm
            anchor={pendingAnchor}
            selectedText={pendingText || ''}
            project={project}
            filePath={filePath}
            onSubmit={() => {
              onCancelNewThread?.();
              onRefresh();
            }}
            onCancel={() => onCancelNewThread?.()}
            onFocus={ensureAgent}
          />
        )}

        {/* Thread cards */}
        {threads.length === 0 && !pendingAnchor && (
          <div className="no-comments">No comments yet</div>
        )}
        {openThreads.map((t) => (
          <ThreadCard
            key={t.id}
            thread={t}
            line={anchorLines[t.id]}
            isResolved={false}
            isHighlighted={highlightedThread === t.id}
            onClick={() => handleThreadClick(t.id)}
            project={project}
            filePath={filePath}
            onRefresh={onRefresh}
            replyOpen={replyFormThread === t.id}
            onToggleReply={() =>
              setReplyFormThread(replyFormThread === t.id ? null : t.id)
            }
            onEnsureAgent={ensureAgent}
          />
        ))}
        {orphanedThreads.map((t) => (
          <ThreadCard
            key={t.id}
            thread={t}
            line={-1}
            isResolved={false}
            isHighlighted={highlightedThread === t.id}
            onClick={() => handleThreadClick(t.id)}
            project={project}
            filePath={filePath}
            onRefresh={onRefresh}
            replyOpen={replyFormThread === t.id}
            onToggleReply={() =>
              setReplyFormThread(replyFormThread === t.id ? null : t.id)
            }
            onEnsureAgent={ensureAgent}
          />
        ))}
        {showResolved &&
          resolvedThreads.map((t) => (
            <ThreadCard
              key={t.id}
              thread={t}
              line={anchorLines[t.id]}
              isResolved
              isHighlighted={highlightedThread === t.id}
              onClick={() => handleThreadClick(t.id)}
              project={project}
              filePath={filePath}
              onRefresh={onRefresh}
              replyOpen={replyFormThread === t.id}
              onToggleReply={() =>
                setReplyFormThread(replyFormThread === t.id ? null : t.id)
              }
              onEnsureAgent={ensureAgent}
            />
          ))}
      </div>
    </div>
  );
}

// ── NewThreadForm ──

interface NewThreadFormProps {
  anchor: Anchor;
  selectedText: string;
  project: string;
  filePath: string;
  onSubmit: () => void;
  onCancel: () => void;
  onFocus: () => void;
}

function NewThreadForm({
  anchor,
  selectedText,
  project,
  filePath,
  onSubmit,
  onCancel,
  onFocus,
}: NewThreadFormProps) {
  const [body, setBody] = useState('');
  const [author, setAuthor] = useState(getSavedAuthor());
  const [submitting, setSubmitting] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  const handleSubmit = async () => {
    if (!body.trim() || !author.trim()) return;
    setSubmitting(true);
    saveAuthor(author.trim());
    try {
      await api.createThread({
        project,
        path: filePath,
        anchor,
        author: author.trim(),
        role: 'human',
        body: body.trim(),
      });
      onSubmit();
    } catch (err) {
      console.error('Failed to create thread:', err);
      setSubmitting(false);
    }
  };

  return (
    <div className="new-thread-form">
      {anchor.svgSnippet ? (
        <div
          className="quoted-svg"
          dangerouslySetInnerHTML={{ __html: anchor.svgSnippet }}
        />
      ) : (
        <div className="quoted-text">{truncateText(selectedText, 200)}</div>
      )}
      <textarea
        ref={textareaRef}
        placeholder="Write a comment..."
        value={body}
        onChange={(e) => setBody(e.target.value)}
        onFocus={onFocus}
        onKeyDown={(e) => {
          if (e.key === 'Escape') onCancel();
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) handleSubmit();
        }}
      />
      <input
        type="text"
        className="author-field"
        placeholder="Your name"
        value={author}
        onChange={(e) => setAuthor(e.target.value)}
      />
      <div className="form-actions">
        <button className="btn-cancel-form" onClick={onCancel}>
          Cancel
        </button>
        <button
          className="btn-submit"
          disabled={submitting || !body.trim() || !author.trim()}
          onClick={handleSubmit}
        >
          Comment
        </button>
      </div>
    </div>
  );
}

// ── ThreadCard ──

interface ThreadCardProps {
  thread: ThreadResponse;
  line: number;
  isResolved: boolean;
  isHighlighted: boolean;
  onClick: () => void;
  project: string;
  filePath: string;
  onRefresh: () => void;
  replyOpen: boolean;
  onToggleReply: () => void;
  onEnsureAgent: () => void;
}

function ThreadCard({
  thread,
  line,
  isResolved,
  isHighlighted,
  onClick,
  project,
  filePath,
  onRefresh,
  replyOpen,
  onToggleReply,
  onEnsureAgent,
}: ThreadCardProps) {
  const ordered = orderComments(thread.comments);
  const byId: Record<string, (typeof thread.comments)[0]> = {};
  thread.comments.forEach((c) => {
    byId[c.id] = c;
  });

  const handleResolve = (e: React.MouseEvent) => {
    e.stopPropagation();
    const author = getSavedAuthor() || 'anonymous';
    api.patchThread(thread.id, { project, path: filePath, status: 'resolved', resolvedBy: author })
      .then(onRefresh)
      .catch((err) => console.error('Failed to resolve:', err));
  };

  const handleReopen = (e: React.MouseEvent) => {
    e.stopPropagation();
    api.patchThread(thread.id, { project, path: filePath, status: 'open' })
      .then(onRefresh)
      .catch((err) => console.error('Failed to reopen:', err));
  };

  const handleSuggestedReply = (text: string) => {
    const author = getSavedAuthor();
    if (!author) {
      onToggleReply();
      return;
    }
    api.replyToThread(thread.id, { project, path: filePath, author, role: 'human', body: text })
      .then(onRefresh)
      .catch((err) => console.error('Failed to submit suggested reply:', err));
  };

  // Get last comment for suggested replies
  const lastComment = ordered.length > 0 ? ordered[ordered.length - 1] : null;
  const showSuggestions =
    lastComment?.suggestedReplies?.length &&
    lastComment.role === 'agent' &&
    thread.status === 'open';

  return (
    <div
      className={`thread-card${isResolved ? ' thread-resolved' : ''}${isHighlighted ? ' highlighted' : ''}`}
      data-thread-id={thread.id}
      data-line={line}
      onClick={onClick}
    >
      {/* Anchor preview */}
      {thread.anchor.svgSnippet ? (
        <div
          className="thread-anchor thread-anchor-svg"
          dangerouslySetInnerHTML={{
            __html: `<span class="thread-status ${thread.status}">${thread.status}</span> ${thread.anchor.svgSnippet}`,
          }}
        />
      ) : (
        <div className="thread-anchor">
          <span className={`thread-status ${thread.status}`}>{thread.status}</span>{' '}
          {truncateText(thread.anchor.selectedText, 80)}
        </div>
      )}

      {line === -1 && (
        <div className="orphaned-warning">Anchor text not found in document</div>
      )}

      {/* Comments */}
      {ordered.map((c, i) => (
        <div key={c.id} className="thread-comment" id={`comment-${c.id}`}>
          {c.inReplyTo && byId[c.inReplyTo] && i > 0 && ordered[i - 1].id !== c.inReplyTo && (
            <div className="comment-reply-marker">
              in reply to @{byId[c.inReplyTo].author}
            </div>
          )}
          <div className="comment-header">
            <span className="comment-author">{c.author}</span>
            {c.role && <span className={`comment-role ${c.role}`}>{c.role}</span>}
            <span className="comment-time">{formatTime(c.createdAt)}</span>
          </div>
          <div
            className="comment-body"
            dangerouslySetInnerHTML={{ __html: renderCommentBody(c.body) }}
          />
        </div>
      ))}

      {/* Suggested replies */}
      {showSuggestions && (
        <div className="suggested-replies">
          {lastComment!.suggestedReplies!.map((suggestion) => (
            <button
              key={suggestion}
              className="suggested-reply-pill"
              onClick={(e) => {
                e.stopPropagation();
                handleSuggestedReply(suggestion);
              }}
            >
              {suggestion}
            </button>
          ))}
        </div>
      )}

      {/* Typing indicator */}
      {thread.agentTyping && (
        <div className="thread-typing">
          <span className="agent-dot" />
          <span className="typing-dots">
            <span>.</span>
            <span>.</span>
            <span>.</span>
          </span>
        </div>
      )}

      {/* Actions */}
      {!replyOpen && (
        <div className="thread-actions">
          <button
            onClick={(e) => {
              e.stopPropagation();
              onEnsureAgent();
              onToggleReply();
            }}
          >
            Reply
          </button>
          {thread.status === 'open' ? (
            <button className="resolve-btn" onClick={handleResolve}>
              Resolve
            </button>
          ) : (
            <button className="reopen-btn" onClick={handleReopen}>
              Reopen
            </button>
          )}
        </div>
      )}

      {/* Reply form */}
      {replyOpen && (
        <ReplyForm
          threadId={thread.id}
          project={project}
          filePath={filePath}
          onSubmit={() => {
            onToggleReply();
            onRefresh();
          }}
          onCancel={onToggleReply}
        />
      )}
    </div>
  );
}

// ── ReplyForm ──

interface ReplyFormProps {
  threadId: string;
  project: string;
  filePath: string;
  onSubmit: () => void;
  onCancel: () => void;
}

function ReplyForm({ threadId, project, filePath, onSubmit, onCancel }: ReplyFormProps) {
  const [body, setBody] = useState('');
  const [author, setAuthor] = useState(getSavedAuthor());
  const [submitting, setSubmitting] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  const handleSubmit = async () => {
    if (!body.trim() || !author.trim()) return;
    setSubmitting(true);
    saveAuthor(author.trim());
    try {
      await api.replyToThread(threadId, {
        project,
        path: filePath,
        author: author.trim(),
        role: 'human',
        body: body.trim(),
      });
      onSubmit();
    } catch (err) {
      console.error('Failed to reply:', err);
      setSubmitting(false);
    }
  };

  return (
    <div className="reply-form" onClick={(e) => e.stopPropagation()}>
      <textarea
        ref={textareaRef}
        placeholder="Write a reply..."
        value={body}
        onChange={(e) => setBody(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Escape') onCancel();
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) handleSubmit();
        }}
      />
      <input
        type="text"
        className="author-field"
        placeholder="Your name"
        value={author}
        onChange={(e) => setAuthor(e.target.value)}
      />
      <div className="form-actions">
        <button className="btn-cancel-form" onClick={onCancel}>
          Cancel
        </button>
        <button
          className="btn-submit"
          disabled={submitting || !body.trim() || !author.trim()}
          onClick={handleSubmit}
        >
          Reply
        </button>
      </div>
    </div>
  );
}
