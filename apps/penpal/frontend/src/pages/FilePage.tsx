import { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { useOutletContext, useLocation } from 'react-router-dom';
import { api } from '../api';
import { useSSE } from '../hooks/useSSE';
import MarkdownViewer from '../components/MarkdownViewer';
import CommentsPanel from '../components/CommentsPanel';
import SelectionToolbar, {
  applySvgHighlights,
} from '../components/SelectionToolbar';
import MermaidSelection, { removePendingSvgHighlight } from '../components/MermaidSelection';
import FileTypeBadge from '../components/FileTypeBadge';
import FileMenu from '../components/FileMenu';
import { renderMermaidBlocks } from '../components/MermaidRenderer';
import type { Heading } from '../components/TableOfContents';
import type { LayoutContext } from '../components/Layout';
import type { ThreadHighlight } from '../components/rehypeCommentHighlights';
import type { ThreadResponse, Anchor, AgentStatus } from '../types';
import { parseProjectWorktree } from '../utils/worktree';

export default function FilePage() {
  const location = useLocation();
  const { setHeadings: pushHeadings, projects } = useOutletContext<LayoutContext>();
  const [rawMarkdown, setRawMarkdown] = useState('');
  const [threads, setThreads] = useState<ThreadResponse[]>([]);
  const [anchorLines, setAnchorLines] = useState<Record<string, number>>({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [agentStatus, setAgentStatus] = useState<AgentStatus | null>(null);
  const [pendingAnchor, setPendingAnchor] = useState<Anchor | null>(null);
  const [pendingText, setPendingText] = useState('');
  const [fileType, setFileType] = useState('');
  const [displayName, setDisplayName] = useState('');
  const [sourceType, setSourceType] = useState('');
  const [projectPath, setProjectPath] = useState('');
  const contentRef = useRef<HTMLDivElement>(null);
  const mermaidDraggingRef = useRef(false);
  const agentPollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const [chatWidth, setChatWidth] = useState(() => {
    const saved = localStorage.getItem('chatPanelWidth');
    return saved ? parseInt(saved, 10) : 340;
  });
  const isResizing = useRef(false);
  const resizeStartX = useRef(0);
  const resizeStartWidth = useRef(0);
  const chatWidthRef = useRef(chatWidth);

  useEffect(() => {
    chatWidthRef.current = chatWidth;
  }, [chatWidth]);

  const handleResizeMouseDown = useCallback((e: React.MouseEvent) => {
    isResizing.current = true;
    resizeStartX.current = e.clientX;
    resizeStartWidth.current = chatWidthRef.current;
    e.preventDefault();

    const onMouseMove = (ev: MouseEvent) => {
      if (ev.buttons === 0) {
        cleanup();
        return;
      }
      const delta = resizeStartX.current - ev.clientX;
      const newWidth = Math.min(Math.max(resizeStartWidth.current + delta, 200), 700);
      setChatWidth(newWidth);
      chatWidthRef.current = newWidth;
    };
    const onMouseUp = () => {
      cleanup();
    };
    const cleanup = () => {
      isResizing.current = false;
      localStorage.setItem('chatPanelWidth', String(chatWidthRef.current));
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
    };
    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
  }, []);

  // Compute highlights for the rehype plugin (text highlights only, not SVG)
  const threadHighlights = useMemo<ThreadHighlight[]>(() => {
    const highlights: ThreadHighlight[] = threads
      .filter((t) => t.status !== 'resolved' && !t.anchor.svgRect)
      .map((t) => {
        const line = anchorLines[t.id];
        if (!line || line === -1 || !t.anchor.selectedText) return null;
        return {
          threadId: t.id,
          selectedText: t.anchor.selectedText,
          startLine: line,
        };
      })
      .filter((h): h is ThreadHighlight => h !== null);

    // Include pending comment highlight so it's rendered via the rehype plugin
    // (React-managed) rather than direct DOM mutation.
    if (pendingAnchor && pendingAnchor.startLine != null && pendingAnchor.startLine > 0 && pendingAnchor.selectedText && !pendingAnchor.svgRect) {
      highlights.push({
        threadId: 'pending',
        selectedText: pendingAnchor.selectedText,
        startLine: pendingAnchor.startLine,
        occurrenceIndex: pendingAnchor.occurrenceIndex,
        pending: true,
      });
    }

    return highlights;
  }, [threads, anchorLines, pendingAnchor]);

  // Resolve project QN, worktree, and file path from URL by matching against known projects.
  // URL: /file/{qualifiedName}[@worktree]/{filePath}
  // QN may contain slashes (e.g. "Development/birdseye"), so we match longest-first.
  const { project, worktree, path } = useMemo(() => {
    const rest = location.pathname.replace(/^\/file\//, '');
    // Try matching against known projects (longest match first)
    const sorted = [...projects].sort((a, b) => b.qualifiedName.length - a.qualifiedName.length);
    for (const p of sorted) {
      if (rest === p.qualifiedName || rest.startsWith(p.qualifiedName + '/') || rest.startsWith(p.qualifiedName + '@')) {
        const afterQN = rest.slice(p.qualifiedName.length);
        // afterQN could be "@worktree/path", "/path", or ""
        let wt = '';
        let filePath = '';
        if (afterQN.startsWith('@')) {
          const slashIdx = afterQN.indexOf('/');
          if (slashIdx === -1) {
            wt = afterQN.slice(1);
          } else {
            wt = afterQN.slice(1, slashIdx);
            filePath = afterQN.slice(slashIdx + 1);
          }
        } else if (afterQN.startsWith('/')) {
          filePath = afterQN.slice(1);
        }
        return { project: p.qualifiedName, worktree: wt, path: filePath };
      }
    }
    // Fallback: parse with @ support
    const segments = rest.split('/');
    const qnCandidate = segments.slice(0, 2).join('/');
    const { project: proj, worktree: wt } = parseProjectWorktree(qnCandidate);
    return {
      project: proj,
      worktree: wt,
      path: segments.slice(2).join('/'),
    };
  }, [location.pathname, projects]);

  // Fetch raw file content
  const fetchContent = useCallback(async (opts?: { silent?: boolean }) => {
    if (!project || !path) return;
    try {
      const content = await api.getRawFile(project, path, worktree || undefined);
      setRawMarkdown(content);
      setError(null);
    } catch (err) {
      if (!opts?.silent) {
        setError('Failed to load file');
      }
      console.error(err);
    } finally {
      setLoading(false);
    }
  }, [project, path, worktree]);

  // Fetch threads
  const fetchThreads = useCallback(async () => {
    if (!project || !path) return;
    try {
      const data = await api.getThreads(project, path, worktree || undefined);
      setThreads(data);
      // Build anchor lines from thread data
      // The server resolves anchors; here we use startLine from the anchor
      const lines: Record<string, number> = {};
      data.forEach((t) => {
        lines[t.id] = t.anchor.startLine || -1;
      });
      setAnchorLines(lines);
    } catch (err) {
      console.error('Failed to load threads:', err);
    }
  }, [project, path, worktree]);

  // Start polling for agent status updates
  const startAgentPolling = useCallback(() => {
    if (agentPollRef.current) return;
    agentPollRef.current = setInterval(async () => {
      if (!project) return;
      try {
        const s = await api.getAgentStatus(project);
        setAgentStatus(s);
        if (!s.running && agentPollRef.current) {
          clearInterval(agentPollRef.current);
          agentPollRef.current = null;
        }
      } catch {
        // ignore
      }
    }, 5000);
  }, [project]);

  // Fetch agent status and auto-start if needed
  const fetchAgentStatus = useCallback(async () => {
    if (!project) return;
    try {
      const status = await api.getAgentStatus(project);
      setAgentStatus(status);
      if (status.needsAgent && !status.running) {
        // Auto-start, then fetch updated status to show the running dot
        try {
          await api.startAgent(project);
          const updated = await api.getAgentStatus(project);
          setAgentStatus(updated);
          if (updated.running) startAgentPolling();
        } catch {
          // ignore start failure
        }
        return;
      }
      if (status.running) startAgentPolling();
    } catch {
      // ignore
    }
  }, [project, startAgentPolling]);

  const fetchFileMetadata = useCallback(async () => {
    if (!project || !path) return;
    api.getProjectFiles(project, worktree || undefined).then((groups) => {
      for (const group of (groups || [])) {
        for (const file of (group.files || [])) {
          if (file.path === path) {
            setFileType(file.fileType || '');
            setDisplayName(file.title || file.name);
            setSourceType(group.sourceType);
            return;
          }
        }
      }
      setFileType('');
      setSourceType('');
      setDisplayName(path.split('/').pop() || path);
    }).catch(() => {
      setFileType('');
      setSourceType('');
      setDisplayName(path.split('/').pop() || path);
    });

    api.listProjects().then((projects) => {
      const p = projects.find((pr) => pr.qualifiedName === project);
      setProjectPath(p?.projectPath || '');
    }).catch(() => {});
  }, [project, path, worktree]);

  // Fetch file metadata
  useEffect(() => {
    fetchFileMetadata();
  }, [fetchFileMetadata]);

  // Tell the server to watch just this file's directory
  useEffect(() => {
    if (project && path) api.focusFile(project, path, worktree || undefined).catch(() => {});
  }, [project, path, worktree]);

  // Initial data load
  useEffect(() => {
    fetchContent();
    fetchThreads();
    fetchAgentStatus();
    if (project && path) api.recordView(project, path).catch(() => {});
    return () => {
      if (agentPollRef.current) clearInterval(agentPollRef.current);
    };
  }, [fetchContent, fetchThreads, fetchAgentStatus]);

  // Render mermaid after content updates, then apply SVG highlights
  useEffect(() => {
    if (!contentRef.current || !rawMarkdown) return;
    const timer = setTimeout(async () => {
      if (contentRef.current) {
        await renderMermaidBlocks(contentRef.current);
        // Apply SVG highlights after mermaid renders
        if (contentRef.current && threads.length > 0) {
          applySvgHighlights(threads, contentRef.current, rawMarkdown);
        }
      }
    }, 100);
    return () => clearTimeout(timer);
  }, [rawMarkdown, threads, anchorLines]);

  // SSE: refresh on relevant events
  useSSE(
    useCallback(
      (event) => {
        if (event.project && event.project !== project) return;
        if (event.type === 'comments') fetchThreads();
        if (event.type === 'files') {
          fetchContent();
          fetchFileMetadata();
          fetchThreads(); // Re-fetch threads so anchor lines are re-resolved against new content
        }
        if (event.type === 'agents') fetchAgentStatus();
      },
      [project, fetchThreads, fetchContent, fetchFileMetadata, fetchAgentStatus],
    ),
    useCallback(() => {
      if (project && path) api.focusFile(project, path, worktree || undefined).catch(() => {});
      fetchContent({ silent: true });
      fetchFileMetadata();
      fetchAgentStatus();
      fetchThreads();
    }, [project, path, worktree, fetchContent, fetchFileMetadata, fetchAgentStatus, fetchThreads]),
  );

  const handleComment = useCallback((anchor: Anchor, selectedText: string) => {
    setPendingAnchor(anchor);
    setPendingText(selectedText);
  }, []);

  const handleCancelNewThread = useCallback(() => {
    setPendingAnchor(null);
    setPendingText('');
    removePendingSvgHighlight();
  }, []);

  const handleThreadFocus = useCallback((threadId: string, line: number) => {
    if (!contentRef.current || line < 1) return;

    // For SVG highlights, scroll to the highlight rect itself (not the container top)
    const svgHighlight = contentRef.current.querySelector(
      `.penpal-svg-highlight[data-thread-id="${threadId}"]`
    );
    if (svgHighlight) {
      const rect = svgHighlight.getBoundingClientRect();
      const scrollParent = contentRef.current.closest('.file-main-scroll') || window;
      if (scrollParent instanceof HTMLElement) {
        const parentRect = scrollParent.getBoundingClientRect();
        const targetY = scrollParent.scrollTop + rect.top - parentRect.top - parentRect.height / 2 + rect.height / 2;
        scrollParent.scrollTo({ top: targetY, behavior: 'smooth' });
      } else {
        svgHighlight.scrollIntoView({ behavior: 'smooth', block: 'center' });
      }
    } else {
      const el = contentRef.current.querySelector(`[data-source-line="${line}"]`);
      if (el) {
        el.scrollIntoView({ behavior: 'smooth', block: 'center' });
      }
    }

    // Activate text highlights
    contentRef.current.querySelectorAll('.comment-highlight.active').forEach((m) => {
      m.classList.remove('active');
    });
    contentRef.current
      .querySelectorAll(`.comment-highlight[data-thread-id="${threadId}"]`)
      .forEach((m) => {
        m.classList.add('active');
      });
    // Activate SVG highlights
    contentRef.current.querySelectorAll('.penpal-svg-highlight.active').forEach((m) => {
      m.classList.remove('active');
    });
    contentRef.current
      .querySelectorAll(`.penpal-svg-highlight[data-thread-id="${threadId}"]`)
      .forEach((m) => {
        m.classList.add('active');
      });
    setTimeout(() => {
      contentRef.current
        ?.querySelectorAll(`.comment-highlight[data-thread-id="${threadId}"]`)
        .forEach((m) => {
          m.classList.remove('active');
        });
      contentRef.current
        ?.querySelectorAll(`.penpal-svg-highlight[data-thread-id="${threadId}"]`)
        .forEach((m) => {
          m.classList.remove('active');
        });
    }, 3000);
  }, []);

  const handleHeadingsExtracted = useCallback((h: Heading[]) => {
    pushHeadings(h);
  }, [pushHeadings]);

  if (loading) {
    return (
      <div data-testid="file-page" style={{ padding: 40 }}>
        Loading...
      </div>
    );
  }

  if (error) {
    return (
      <div data-testid="file-page" style={{ padding: 40, color: 'var(--accent-danger)' }}>
        {error}
      </div>
    );
  }

  return (
    <div data-testid="file-page" className="file-page-layout" style={{ gridTemplateColumns: `1fr 4px ${chatWidth}px` }}>
      <div className="file-main" id="file-main">
        <div className="file-toolbar">
          <FileTypeBadge type={fileType} />
          <span className="toolbar-filename">{displayName || path.split('/').pop()}</span>
          <FileMenu
            project={project}
            projectPath={projectPath}
            filePath={path}
            sourceType={sourceType}
            rawMarkdown={rawMarkdown}
          />
        </div>

        <div className="file-main-scroll">
          <div style={{ position: 'relative' }}>
            <MarkdownViewer
              content={rawMarkdown}
              rawMarkdown={rawMarkdown}
              onHeadingsExtracted={handleHeadingsExtracted}
              highlights={threadHighlights}
              ref={contentRef}
            />
            <SelectionToolbar
              contentRef={contentRef}
              rawMarkdown={rawMarkdown}
              onComment={handleComment}
            />
            <MermaidSelection
              contentRef={contentRef}
              rawMarkdown={rawMarkdown}
              onComment={handleComment}
              draggingRef={mermaidDraggingRef}
            />
          </div>
        </div>
      </div>

      <div className="chat-resize-handle" onMouseDown={handleResizeMouseDown} />

      <CommentsPanel
        threads={threads}
        anchorLines={anchorLines}
        project={project}
        worktree={worktree}
        filePath={path}
        onRefresh={fetchThreads}
        onThreadFocus={handleThreadFocus}
        agentStatus={agentStatus}
        pendingAnchor={pendingAnchor}
        pendingText={pendingText}
        onCancelNewThread={handleCancelNewThread}
      />
    </div>
  );
}
