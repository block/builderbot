import { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { useOutletContext, useLocation } from 'react-router-dom';
import { api } from '../api';
import { useSSE } from '../hooks/useSSE';
import MarkdownViewer from '../components/MarkdownViewer';
import CommentsPanel from '../components/CommentsPanel';
import SelectionToolbar, {
  addCommentHighlights,
  removePendingHighlight,
} from '../components/SelectionToolbar';
import FileTypeBadge from '../components/FileTypeBadge';
import FileMenu from '../components/FileMenu';
import { renderMermaidBlocks } from '../components/MermaidRenderer';
import type { Heading } from '../components/TableOfContents';
import type { LayoutContext } from '../components/Layout';
import type { ThreadResponse, Anchor, AgentStatus } from '../types';

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
  const agentPollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Resolve project QN and file path from URL by matching against known projects.
  // The URL is /file/{qualifiedName}/{filePath} where qualifiedName may contain slashes
  // (e.g. "Development/birdseye"), so we can't rely on a single :param.
  const { project, path } = useMemo(() => {
    const rest = location.pathname.replace(/^\/file\//, '');
    // Try matching against known projects (longest match first)
    const sorted = [...projects].sort((a, b) => b.qualifiedName.length - a.qualifiedName.length);
    for (const p of sorted) {
      if (rest === p.qualifiedName || rest.startsWith(p.qualifiedName + '/')) {
        return {
          project: p.qualifiedName,
          path: rest.slice(p.qualifiedName.length + 1),
        };
      }
    }
    // Fallback: assume first two segments are the QN
    const segments = rest.split('/');
    return {
      project: segments.slice(0, 2).join('/'),
      path: segments.slice(2).join('/'),
    };
  }, [location.pathname, projects]);

  // Fetch raw file content
  const fetchContent = useCallback(async () => {
    if (!project || !path) return;
    try {
      const content = await api.getRawFile(project, path);
      setRawMarkdown(content);
      setError(null);
    } catch (err) {
      setError('Failed to load file');
      console.error(err);
    } finally {
      setLoading(false);
    }
  }, [project, path]);

  // Fetch threads
  const fetchThreads = useCallback(async () => {
    if (!project || !path) return;
    try {
      const data = await api.getThreads(project, path);
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
  }, [project, path]);

  // Fetch agent status
  const fetchAgentStatus = useCallback(async () => {
    if (!project) return;
    try {
      const status = await api.getAgentStatus(project);
      setAgentStatus(status);
      if (status.running && !agentPollRef.current) {
        agentPollRef.current = setInterval(async () => {
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
      }
    } catch {
      // ignore
    }
  }, [project]);

  // Fetch file metadata
  useEffect(() => {
    if (!project || !path) return;
    // Get file metadata from project files list
    api.getProjectFiles(project).then((groups) => {
      for (const group of groups) {
        for (const file of group.files) {
          if (file.path === path) {
            setFileType(file.fileType || '');
            setDisplayName(file.name);
            setSourceType(group.sourceType);
            return;
          }
        }
      }
      // Fallback
      setDisplayName(path.split('/').pop() || path);
    }).catch(() => {
      setDisplayName(path.split('/').pop() || path);
    });

    // Get project path
    api.listProjects().then((projects) => {
      const p = projects.find((pr) => pr.qualifiedName === project);
      if (p) setProjectPath(p.projectPath);
    }).catch(() => {});
  }, [project, path]);

  // Initial data load
  useEffect(() => {
    fetchContent();
    fetchThreads();
    fetchAgentStatus();
    return () => {
      if (agentPollRef.current) clearInterval(agentPollRef.current);
    };
  }, [fetchContent, fetchThreads, fetchAgentStatus]);

  // Render mermaid after content updates
  useEffect(() => {
    if (!contentRef.current || !rawMarkdown) return;
    // Small delay to let React finish rendering
    const timer = setTimeout(() => {
      if (contentRef.current) {
        renderMermaidBlocks(contentRef.current);
      }
    }, 100);
    return () => clearTimeout(timer);
  }, [rawMarkdown]);

  // Apply comment highlights after threads/content update
  useEffect(() => {
    if (!contentRef.current || threads.length === 0) return;
    const timer = setTimeout(() => {
      if (contentRef.current) {
        addCommentHighlights(threads, anchorLines, contentRef.current);
      }
    }, 200);
    return () => clearTimeout(timer);
  }, [threads, anchorLines, rawMarkdown]);

  // SSE: refresh on relevant events
  useSSE(
    useCallback(
      (event) => {
        if (event.project && event.project !== project) return;
        if (event.type === 'comments') fetchThreads();
        if (event.type === 'files') fetchContent();
        if (event.type === 'agents') fetchAgentStatus();
      },
      [project, fetchThreads, fetchContent, fetchAgentStatus],
    ),
  );

  const handleComment = useCallback((anchor: Anchor, selectedText: string) => {
    setPendingAnchor(anchor);
    setPendingText(selectedText);
  }, []);

  const handleCancelNewThread = useCallback(() => {
    setPendingAnchor(null);
    setPendingText('');
    removePendingHighlight();
  }, []);

  const handleThreadFocus = useCallback((threadId: string, line: number) => {
    if (!contentRef.current || line < 1) return;
    const el = contentRef.current.querySelector(`[data-source-line="${line}"]`);
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }
    // Activate highlight
    contentRef.current.querySelectorAll('.comment-highlight.active').forEach((m) => {
      m.classList.remove('active');
    });
    contentRef.current
      .querySelectorAll(`.comment-highlight[data-thread-id="${threadId}"]`)
      .forEach((m) => {
        m.classList.add('active');
      });
    setTimeout(() => {
      contentRef.current
        ?.querySelectorAll(`.comment-highlight[data-thread-id="${threadId}"]`)
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
    <div data-testid="file-page" className="file-page-layout">
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
              ref={contentRef}
            />
            <SelectionToolbar
              contentRef={contentRef}
              rawMarkdown={rawMarkdown}
              onComment={handleComment}
            />
          </div>
        </div>
      </div>

      <CommentsPanel
        threads={threads}
        anchorLines={anchorLines}
        project={project}
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
