import { useCallback, useEffect, useRef, useState } from 'react';
import { Link, useLocation, useOutletContext } from 'react-router-dom';
import { api } from '../api';
import { useSSE } from '../hooks/useSSE';
import type { LayoutContext } from '../components/Layout';
import type { APIFileGroupView, APIFile, APIFileInReview, AgentStatus, SSEEvent } from '../types';
import FileTypeBadge from '../components/FileTypeBadge';
import { parseProjectWorktree } from '../utils/worktree';

function debounce<T extends (...args: never[]) => void>(fn: T, ms: number): T {
  let timer: ReturnType<typeof setTimeout>;
  return ((...args: Parameters<T>) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  }) as T;
}

function WorkingIndicator() {
  return (
    <span className="file-working">
      <span className="agent-dot" />
      <span className="working-dots"><span>.</span><span>.</span><span>.</span></span>
    </span>
  );
}

// E-PENPAL-IN-REVIEW-SECTION: "In Review" section before source groups with WorkingIndicator.
// E-PENPAL-SOURCE-ACTIONS: source group header three-dot menu with copy/publish/remove/delete.
// E-PENPAL-BATCH-OPS: selection state with batch actions (copy markdown, copy paths, publish, delete).
export default function ProjectPage() {
  const location = useLocation();
  const { setSidebarExtra } = useOutletContext<LayoutContext>();
  const qnRaw = location.pathname.replace(/^\/project\//, '');
  const { project: qn, worktree } = parseProjectWorktree(qnRaw);
  const [groups, setGroups] = useState<APIFileGroupView[]>([]);
  const [reviewData, setReviewData] = useState<Record<string, APIFileInReview>>({});
  const [agentStatus, setAgentStatus] = useState<AgentStatus | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [openFileMenu, setOpenFileMenu] = useState<string | null>(null);
  const [openSourceMenu, setOpenSourceMenu] = useState<string | null>(null);
  const [showAddSource, setShowAddSource] = useState(false);
  const [sourcePathInput, setSourcePathInput] = useState('');
  const [sourceError, setSourceError] = useState('');
  const [addingSource, setAddingSource] = useState(false);
  const [deleteFiles, setDeleteFiles] = useState<{ project: string; path: string }[]>([]);
  const [deleting, setDeleting] = useState(false);
  const [projectDeleted, setProjectDeleted] = useState(false);
  const agentPollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const refreshFiles = useCallback(() => {
    if (!qn) return;
    api.getProjectFiles(qn, worktree || undefined).then(setGroups).catch(() => {});
  }, [qn, worktree]);

  const refreshReviews = useCallback(() => {
    if (!qn) return;
    api.getReviews(qn, worktree || undefined).then((reviews) => {
      const map: Record<string, APIFileInReview> = {};
      reviews.forEach((r) => { map[r.filePath] = r; });
      setReviewData(map);
    }).catch(() => {});
  }, [qn, worktree]);

  const refreshAgent = useCallback(() => {
    if (!qn) return;
    api.getAgentStatus(qn).then((status) => {
      setAgentStatus(status);
      if (status.running && !agentPollRef.current) {
        agentPollRef.current = setInterval(() => {
          api.getAgentStatus(qn).then(setAgentStatus).catch(() => {});
        }, 5000);
      } else if (!status.running && agentPollRef.current) {
        clearInterval(agentPollRef.current);
        agentPollRef.current = null;
      }
    }).catch(() => {});
  }, [qn]);

  const refreshProjectStatus = useCallback(() => {
    api.listProjects().then((projects) => {
      if (!projects.find((p) => p.qualifiedName === qn)) {
        setProjectDeleted(true);
      }
    }).catch(() => {});
  }, [qn]);

  // Tell the server to deep-watch this project's files
  useEffect(() => {
    if (qn) api.focusProject(qn).catch(() => {});
  }, [qn]);

  useEffect(() => {
    refreshFiles();
    refreshReviews();
    refreshAgent();
    refreshProjectStatus();
    return () => {
      if (agentPollRef.current) clearInterval(agentPollRef.current);
    };
  }, [refreshFiles, refreshReviews, refreshAgent, refreshProjectStatus]);

  const debouncedRefreshFiles = useCallback(() => debounce(refreshFiles, 200)(), [refreshFiles]);
  const debouncedRefreshReviews = useCallback(() => debounce(refreshReviews, 200)(), [refreshReviews]);

  useSSE(
    useCallback(
      (event: SSEEvent) => {
        if (event.type === 'files' && event.project === qn) debouncedRefreshFiles();
        if (event.type === 'projects') refreshProjectStatus();
        if (event.type === 'comments' && event.project === qn) debouncedRefreshReviews();
        if (event.type === 'agents') {
          refreshAgent();
          if (event.project === qn) debouncedRefreshReviews();
        }
      },
      [qn, debouncedRefreshFiles, debouncedRefreshReviews, refreshAgent, refreshProjectStatus],
    ),
    useCallback(() => {
      if (qn) api.focusProject(qn).catch(() => {});
      refreshFiles();
      refreshReviews();
      refreshAgent();
      refreshProjectStatus();
    }, [qn, refreshFiles, refreshReviews, refreshAgent, refreshProjectStatus]),
  );

  // Push sidebar card listing sources (with In Review if applicable)
  const hasReviews = Object.keys(reviewData).length > 0;
  useEffect(() => {
    if (groups.length === 0 && !hasReviews) {
      setSidebarExtra(null);
      return;
    }
    setSidebarExtra(
      <div className="sidebar-card">
        <div className="sidebar-card-title">Sources</div>
        <nav className="sidebar-card-nav">
          {hasReviews && (
            <a href="#source-In Review">
              <span
                className="sidebar-source-tag"
                style={{ color: 'var(--file-type-research-color)', background: 'var(--file-type-research-bg)' }}
              >
                Review
              </span>
              In Review
            </a>
          )}
          {groups.map((g) => (
            <a key={g.name} href={`#source-${g.name}`}>
              {g.badgeText && (
                <span
                  className="sidebar-source-tag"
                  style={{ color: g.badgeColor, background: g.badgeBg }}
                >
                  {g.badgeText}
                </span>
              )}
              {g.name}
            </a>
          ))}
        </nav>
      </div>,
    );
    return () => setSidebarExtra(null);
  }, [groups, hasReviews, setSidebarExtra]);

  // Close menus on outside click
  useEffect(() => {
    function close() { setOpenFileMenu(null); setOpenSourceMenu(null); }
    document.addEventListener('click', close);
    return () => document.removeEventListener('click', close);
  }, []);

  // Selection helpers
  function toggleFile(path: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path); else next.add(path);
      return next;
    });
  }

  function toggleSource(group: APIFileGroupView) {
    const paths = (group.files || []).map((f) => f.path);
    setSelected((prev) => {
      const next = new Set(prev);
      const allSelected = paths.every((p) => next.has(p));
      paths.forEach((p) => { if (allSelected) next.delete(p); else next.add(p); });
      return next;
    });
  }

  function isSourceAllSelected(group: APIFileGroupView) {
    return group.files?.length > 0 && group.files.every((f) => selected.has(f.path));
  }

  function isSourcePartial(group: APIFileGroupView) {
    const paths = (group.files || []).map((f) => f.path);
    const count = paths.filter((p) => selected.has(p)).length;
    return count > 0 && count < paths.length;
  }

  function clearSelection() { setSelected(new Set()); }

  // File menu actions
  function getFileData(file: APIFile) {
    return { project: qn, path: file.path, sourceType: file.sourceType || '' };
  }

  function copyRelativePath(file: APIFile, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenFileMenu(null);
    navigator.clipboard.writeText('@' + file.path);
  }

  function copyAbsolutePath(file: APIFile, projectPath: string, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenFileMenu(null);
    navigator.clipboard.writeText(projectPath + '/' + file.path);
  }

  function removeFile(file: APIFile, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenFileMenu(null);
    api.removeSource(qn, undefined, file.path).then(() => debouncedRefreshFiles()).catch((err) => alert(err.message));
  }

  function showDeleteFile(file: APIFile, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenFileMenu(null);
    setDeleteFiles([{ project: qn, path: file.path }]);
  }

  function publishFile(file: APIFile, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenFileMenu(null);
    api.publish(qn, file.path).then((data) => {
      navigator.clipboard.writeText(data.url);
    }).catch((err) => alert(err.message));
  }

  function copyRawMarkdown(file: APIFile, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenFileMenu(null);
    api.getRawFile(qn, file.path).then((text) => navigator.clipboard.writeText(text)).catch(() => {});
  }

  // Source menu actions
  function copySourceRelative(group: APIFileGroupView, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenSourceMenu(null);
    const paths = (group.files || []).map((f) => '@' + f.path);
    navigator.clipboard.writeText(paths.join('\n'));
  }

  function copySourceAbsolute(group: APIFileGroupView, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenSourceMenu(null);
    // We don't have projectPath here easily, so use the file's path
    // For now, copy relative paths (projectPath comes from the project data)
    const paths = (group.files || []).map((f) => f.path);
    navigator.clipboard.writeText(paths.join('\n'));
  }

  function removeSourceFromPenpal(group: APIFileGroupView, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenSourceMenu(null);
    if (group.sourceType === 'files') {
      const files = group.files || [];
      if (!confirm(`Remove ${files.length} file(s) from Penpal?\nNo files will be deleted.`)) return;
      Promise.allSettled(files.map((f) => api.removeSource(qn, undefined, f.path)))
        .then(() => debouncedRefreshFiles());
    } else {
      if (!confirm(`Remove source "${group.name}" from this project?\nNo files will be deleted.`)) return;
      api.removeSource(qn, group.name).then(() => debouncedRefreshFiles()).catch((err) => alert(err.message));
    }
  }

  function publishSource(group: APIFileGroupView, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenSourceMenu(null);
    Promise.allSettled((group.files || []).map((f) => api.publish(qn, f.path)));
  }

  function deleteSourceFromDisk(group: APIFileGroupView, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenSourceMenu(null);
    const files = (group.files || []).map((f) => ({ project: qn, path: f.path }));
    if (files.length > 0) setDeleteFiles(files);
  }

  // Batch operations
  function getSelectedFileData() {
    const allFiles: APIFile[] = [];
    groups.forEach((g) => (g.files || []).forEach((f) => allFiles.push(f)));
    return allFiles.filter((f) => selected.has(f.path));
  }

  function copySelectedRelative() {
    const paths = getSelectedFileData().map((f) => '@' + f.path);
    navigator.clipboard.writeText(paths.join('\n'));
  }

  function copySelectedMarkdown() {
    const files = getSelectedFileData();
    Promise.all(files.map((f) => api.getRawFile(qn, f.path)))
      .then((texts) => navigator.clipboard.writeText(texts.join('\n\n---\n\n')));
  }

  function deleteSelected() {
    const files = getSelectedFileData().map((f) => ({ project: qn, path: f.path }));
    if (files.length > 0) setDeleteFiles(files);
  }

  function publishSelected() {
    const files = getSelectedFileData();
    Promise.allSettled(files.map((f) => api.publish(qn, f.path)));
  }

  // Execute delete
  function executeDelete() {
    setDeleting(true);
    Promise.allSettled(deleteFiles.map((d) => api.deleteFile(d.project, d.path)))
      .then((results) => {
        const failed = results.filter((r) => r.status === 'rejected').length;
        setDeleteFiles([]);
        setDeleting(false);
        clearSelection();
        debouncedRefreshFiles();
        if (failed > 0) alert(`${failed} file(s) failed to delete.`);
      });
  }

  // Add source
  function confirmAddSource() {
    if (!sourcePathInput.trim()) { setSourceError('Please enter a path.'); return; }
    setAddingSource(true);
    setSourceError('');
    api.addSource(qn, sourcePathInput.trim())
      .then(() => {
        setShowAddSource(false);
        setSourcePathInput('');
        debouncedRefreshFiles();
      })
      .catch((err) => setSourceError(err.message))
      .finally(() => setAddingSource(false));
  }

  // Stop agent
  function stopAgent() {
    api.stopAgent(qn).then(refreshAgent);
  }

  // Build in-review section
  const reviewPaths = Object.keys(reviewData);
  const hasAgent = reviewPaths.some((p) => reviewData[p].agentActive);

  function contextBarClass(pct: number) {
    if (pct >= 85) return 'critical';
    if (pct >= 60) return 'warning';
    return '';
  }

  function renderFileRow(file: APIFile) {
    const review = reviewData[file.path];
    const d = getFileData(file);
    return (
      <li
        key={file.path}
        className={`file-row${selected.has(file.path) ? ' selected' : ''}`}
        onClick={() => toggleFile(file.path)}
      >
        <div className="file-left">
          <input
            type="checkbox"
            className="file-checkbox"
            checked={selected.has(file.path)}
            onChange={() => toggleFile(file.path)}
            onClick={(e) => e.stopPropagation()}
          />
          <FileTypeBadge type={file.fileType} />
          {review && (
            <span className={`review-badge${review.agentActive ? ' agent-active' : ''}`}>in review</span>
          )}
          {review && (review.workingThreads ?? 0) > 0 && <WorkingIndicator />}
          <span className="file-name">
            <Link to={`/file/${qnRaw}/${file.path}`} onClick={(e) => e.stopPropagation()}>
              {file.title || file.name}
            </Link>
            <span className="file-subtitle">{file.title ? file.name : '\u00A0'}</span>
          </span>
        </div>
        <div className="file-right">
          <span className="file-age">{file.age}</span>
          <div className="file-menu-wrap" onClick={(e) => e.stopPropagation()}>
            <button className="file-dots" onClick={(e) => { e.stopPropagation(); setOpenFileMenu(openFileMenu === file.path ? null : file.path); }}>
              &#8942;
            </button>
            {openFileMenu === file.path && (
              <div className="file-menu" style={{ display: 'block' }}>
                <button onClick={(e) => copyRawMarkdown(file, e)}>Copy markdown</button>
                <button onClick={(e) => copyRelativePath(file, e)}>Copy relative path</button>
                <button onClick={(e) => copyAbsolutePath(file, '', e)}>Copy absolute path</button>
                <div className="menu-divider" />
                <button onClick={(e) => publishFile(file, e)}>Publish</button>
                {d.sourceType === 'files' && (
                  <>
                    <div className="menu-divider" />
                    <button onClick={(e) => removeFile(file, e)}>Remove from Penpal</button>
                  </>
                )}
                <div className="menu-divider" />
                <button className="menu-danger" onClick={(e) => showDeleteFile(file, e)}>Delete from disk</button>
              </div>
            )}
          </div>
        </div>
      </li>
    );
  }

  if (projectDeleted) {
    return (
      <div data-testid="project-page">
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '10px 16px', background: 'var(--accent-danger-light)', border: '1px solid var(--accent-danger)', borderRadius: 6, marginBottom: 24, fontSize: '0.9em', color: 'var(--accent-danger)', fontWeight: 500 }}>
          This project is being deleted...
        </div>
      </div>
    );
  }

  return (
    <div data-testid="project-page">
      {/* Agent status bar */}
      {agentStatus?.running && (
        <div className="agent-status-bar">
          <span className="agent-dot" />
          <span>Agent active</span>
          <div className="context-bar" title={`${(agentStatus.contextUsed || 0).toLocaleString()} / ${(agentStatus.contextWindow || 0).toLocaleString()} tokens`}>
            <div className={`context-bar-fill ${contextBarClass(agentStatus.contextPercent || 0)}`} style={{ width: `${Math.round(agentStatus.contextPercent || 0)}%` }} />
          </div>
          <span className="context-label">
            {Math.round(agentStatus.contextPercent || 0)}% context
            {agentStatus.totalCostUSD > 0 && ` · $${agentStatus.totalCostUSD.toFixed(2)}`}
          </span>
          <button className="agent-stop-btn" onClick={stopAgent}>Stop</button>
        </div>
      )}

      {/* In-review virtual source */}
      {reviewPaths.length > 0 && (
        <div className="source-section in-review-section">
          <div className="source-header">
            <span className="source-badge" style={{ color: 'var(--file-type-research-color)', background: 'var(--file-type-research-bg)' }}>Review</span>
            <span className="source-header-label">In Review</span>
            {hasAgent && <span style={{ marginLeft: 'auto' }}><WorkingIndicator /></span>}
          </div>
          <ul className="files-list">
            {reviewPaths.map((path) => {
              const file = groups.flatMap((g) => g.files || []).find((f) => f.path === path);
              const name = file?.name || path.split('/').pop() || path;
              const title = file?.title;
              const age = file?.age || '';
              const fileType = file?.fileType;
              return (
                <li key={path} className="file-row">
                  <div className="file-left">
                    <FileTypeBadge type={fileType} />
                    <span className={`review-badge${reviewData[path].agentActive ? ' agent-active' : ''}`}>in review</span>
                    {(reviewData[path].workingThreads ?? 0) > 0 && <WorkingIndicator />}
                    <span className="file-name">
                      <Link to={`/file/${qnRaw}/${path}`}>{title || name}</Link>
                      <span className="file-subtitle">{title ? name : '\u00A0'}</span>
                    </span>
                  </div>
                  <div className="file-right">
                    <span className="file-age">{age}</span>
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {/* Source groups */}
      {groups.length > 0 ? (
        groups.map((group) => {
          const allSel = isSourceAllSelected(group);
          const partial = isSourcePartial(group);
          let prevDir = '';
          return (
            <div
              key={group.name}
              className={`source-section${allSel ? ' all-selected' : ''}${allSel || partial ? ' selected' : ''}`}
              id={`source-${group.name}`}
            >
              <div className="source-header" onClick={() => toggleSource(group)}>
                <input
                  type="checkbox"
                  checked={allSel}
                  ref={(el) => { if (el) el.indeterminate = partial; }}
                  onChange={() => toggleSource(group)}
                  onClick={(e) => e.stopPropagation()}
                  style={{ flexShrink: 0 }}
                />
                {group.badgeText && (
                  <span className="source-badge" style={{ color: group.badgeColor, background: group.badgeBg }}>
                    {group.badgeText}
                  </span>
                )}
                <span className="source-header-label">{group.name}</span>
                {group.auto && <span className="auto-badge">(auto)</span>}
                <button className="source-dots" onClick={(e) => { e.stopPropagation(); setOpenSourceMenu(openSourceMenu === group.name ? null : group.name); }}>
                  &#8942;
                </button>
                {openSourceMenu === group.name && (
                  <div className="dropdown-menu" style={{ position: 'absolute', right: 0, top: '100%' }} onClick={(e) => e.stopPropagation()}>
                    <button onClick={(e) => copySourceRelative(group, e)}>Copy relative paths</button>
                    <button onClick={(e) => copySourceAbsolute(group, e)}>Copy absolute paths</button>
                    <div className="menu-divider" />
                    <button onClick={(e) => publishSource(group, e)}>Publish</button>
                    {!group.auto && (
                      <>
                        <div className="menu-divider" />
                        <button onClick={(e) => removeSourceFromPenpal(group, e)}>Remove from Penpal</button>
                      </>
                    )}
                    <div className="menu-divider" />
                    <button className="menu-danger" onClick={(e) => deleteSourceFromDisk(group, e)}>Delete from disk</button>
                  </div>
                )}
              </div>
              <ul className="files-list">
                {(group.files || []).map((file) => {
                  const showDir = file.dir && file.dir !== prevDir;
                  prevDir = file.dir || '';
                  return (
                    <li key={file.path} style={{ listStyle: 'none' }}>
                      {showDir && <div className="dir-heading">{file.dir}</div>}
                      {renderFileRow(file)}
                    </li>
                  );
                })}
              </ul>
            </div>
          );
        })
      ) : (
        <p className="empty">No files yet.</p>
      )}

      <a className="add-source-btn" onClick={() => { setShowAddSource(true); setSourcePathInput(''); setSourceError(''); }}>
        + Add to project
      </a>

      {/* Add source modal */}
      <div className={`modal-overlay${showAddSource ? ' open' : ''}`} onClick={() => setShowAddSource(false)}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
          <h3>Add to project</h3>
          <p>Path relative to project root &mdash; a directory (e.g., <code>docs</code>) or a markdown file (e.g., <code>README.md</code>)</p>
          <input
            type="text"
            className="modal-input"
            value={sourcePathInput}
            onChange={(e) => setSourcePathInput(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') confirmAddSource(); }}
            placeholder="docs or path/to/file.md"
            autoFocus
          />
          {sourceError && <div className="modal-error visible">{sourceError}</div>}
          <div className="modal-actions">
            <button className="btn-cancel" onClick={() => setShowAddSource(false)}>Cancel</button>
            <button className="btn-primary" onClick={confirmAddSource} disabled={addingSource}>
              {addingSource ? 'Adding...' : 'Add'}
            </button>
          </div>
        </div>
      </div>

      {/* Delete file modal */}
      <div className={`modal-overlay${deleteFiles.length > 0 ? ' open' : ''}`} onClick={() => setDeleteFiles([])}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
          <h3>Delete file{deleteFiles.length !== 1 ? 's' : ''}?</h3>
          <p>
            This will permanently delete{' '}
            <strong>{deleteFiles.length === 1 ? deleteFiles[0]?.path.split('/').pop() : `${deleteFiles.length} files`}</strong>{' '}
            from the filesystem. This cannot be undone.
          </p>
          <div className="modal-actions">
            <button className="btn-cancel" onClick={() => setDeleteFiles([])}>Cancel</button>
            <button className="btn-delete" onClick={executeDelete} disabled={deleting}>
              {deleting ? 'Deleting...' : 'Delete from disk'}
            </button>
          </div>
        </div>
      </div>

      {/* Selection bar */}
      <div className={`selection-bar${selected.size > 0 ? ' visible' : ''}`}>
        <span className="count">{selected.size} file{selected.size !== 1 ? 's' : ''} selected</span>
        <button onClick={copySelectedMarkdown}>Copy markdown</button>
        <button onClick={copySelectedRelative}>Copy paths</button>
        <button onClick={publishSelected}>Publish</button>
        <button className="danger-btn" onClick={deleteSelected}>Delete</button>
        <button className="clear-btn" onClick={clearSelection}>Clear</button>
      </div>
    </div>
  );
}
