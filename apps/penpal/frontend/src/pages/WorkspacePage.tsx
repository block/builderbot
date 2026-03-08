import { useCallback, useEffect, useState } from 'react';
import { Link, useParams, useOutletContext } from 'react-router-dom';
import { api } from '../api';
import { useSSE } from '../hooks/useSSE';
import type { LayoutContext } from '../components/Layout';
import type { APIProject, SSEEvent, ProjectInfo } from '../types';

function debounce<T extends (...args: never[]) => void>(fn: T, ms: number): T {
  let timer: ReturnType<typeof setTimeout>;
  return ((...args: Parameters<T>) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  }) as T;
}

export default function WorkspacePage() {
  const { name } = useParams<{ name: string }>();
  const { setSidebarExtra } = useOutletContext<LayoutContext>();
  const [projects, setProjects] = useState<APIProject[]>([]);
  const [standaloneProjects, setStandaloneProjects] = useState<APIProject[]>([]);
  const [openMenu, setOpenMenu] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<APIProject | null>(null);
  const [deleteInfo, setDeleteInfo] = useState<ProjectInfo | null>(null);
  const [deleting, setDeleting] = useState<Set<string>>(new Set());

  const refreshProjects = useCallback(() => {
    api.listProjects().then((all) => {
      const filtered = all.filter((p) => p.workspace === name && p.origin === 'workspace');
      filtered.sort((a, b) => {
        if ((a.fileCount > 0) !== (b.fileCount > 0)) return b.fileCount > 0 ? 1 : -1;
        if (a.fileCount === 0 && b.fileCount === 0) return a.name.localeCompare(b.name);
        return 0;
      });
      setProjects(filtered);
      setStandaloneProjects(all.filter((p) => p.origin === 'standalone'));
    }).catch(() => {});
  }, [name]);

  // Workspace dirs are already watched; clear any deep project/file focus
  useEffect(() => { api.clearFocus().catch(() => {}); }, []);

  useEffect(() => {
    refreshProjects();
  }, [refreshProjects]);

  const debouncedRefresh = useCallback(
    () => debounce(refreshProjects, 200)(),
    [refreshProjects],
  );

  useSSE(
    useCallback(
      (event: SSEEvent) => {
        if (event.type === 'projects' || event.type === 'files' || event.type === 'comments' || event.type === 'agents') {
          debouncedRefresh();
        }
      },
      [debouncedRefresh],
    ),
  );

  // Push sidebar card listing projects
  useEffect(() => {
    if (projects.length === 0) {
      setSidebarExtra(null);
      return;
    }
    setSidebarExtra(
      <div className="sidebar-card">
        <div className="sidebar-card-title">Projects</div>
        <nav className="sidebar-card-nav">
          {projects.map((p) => (
            <Link
              key={p.qualifiedName}
              to={`/project/${p.qualifiedName}`}
              className={p.fileCount === 0 ? 'deemphasized' : undefined}
            >
              {p.name}
            </Link>
          ))}
        </nav>
      </div>,
    );
    return () => setSidebarExtra(null);
  }, [projects, setSidebarExtra]);

  function handleCopyPath(project: APIProject, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenMenu(null);
    navigator.clipboard.writeText(project.projectPath);
  }

  function handleShowDelete(project: APIProject, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenMenu(null);
    setDeleteInfo(null);
    setDeleteTarget(project);
    api.getProjectInfo(project.qualifiedName).then(setDeleteInfo).catch(() => setDeleteInfo(null));
  }

  function handleCloseProject(project: APIProject, e: React.MouseEvent) {
    e.stopPropagation();
    setOpenMenu(null);
    if (!confirm('Close this project? It will be removed from Penpal but no files will be deleted.')) return;
    api.closeProject(project.projectPath).then(refreshProjects).catch((err) => alert(err.message));
  }

  function confirmDelete() {
    if (!deleteTarget) return;
    const qn = deleteTarget.qualifiedName;
    setDeleteTarget(null);
    setDeleting((prev) => new Set(prev).add(qn));
    api.deleteProject(qn)
      .then(refreshProjects)
      .catch((err) => {
        setDeleting((prev) => { const next = new Set(prev); next.delete(qn); return next; });
        alert('Failed to delete project: ' + err.message);
      });
  }

  function renderProjectCard(p: APIProject) {
    const isStandalone = p.origin === 'standalone';
    return (
      <div
        key={p.qualifiedName}
        className={`project-card${p.fileCount === 0 ? ' deemphasized' : ''}${deleting.has(p.qualifiedName) ? ' deleting' : ''}`}
      >
        <div className="project-card-header">
          <div className="project-card-name">
            <Link to={`/project/${p.qualifiedName}`}>{p.name}</Link>
            {p.badges.map((b) => (
              <span key={b.text} className="source-badge" style={{ color: b.color, backgroundColor: b.bg }}>
                {b.text}
              </span>
            ))}
            {(p.agentConnected || p.agentRunning) && <span className="agent-dot" title="Agent active" />}
            {(p.reviewCount ?? 0) > 0 && <span className="review-count">{p.reviewCount} in review</span>}
            {deleting.has(p.qualifiedName) && <span className="deleting-badge">Deleting...</span>}
          </div>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
            {p.age && <span className="project-age">{p.age}</span>}
            <div className="dropdown-menu-wrap">
              <button className="dropdown-dots" onClick={(e) => { e.stopPropagation(); setOpenMenu(openMenu === p.qualifiedName ? null : p.qualifiedName); }}>
                &#8942;
              </button>
              {openMenu === p.qualifiedName && (
                <div className="dropdown-menu">
                  <button onClick={(e) => handleCopyPath(p, e)}>Copy path</button>
                  {isStandalone ? (
                    <button className="menu-muted" onClick={(e) => handleCloseProject(p, e)}>Close</button>
                  ) : p.name !== '(root)' ? (
                    <button className="menu-danger" onClick={(e) => handleShowDelete(p, e)}>Delete</button>
                  ) : null}
                </div>
              )}
            </div>
          </div>
        </div>
        {p.branch ? (
          <div className="project-card-meta">
            <span className="branch">{p.branch}{p.dirty && <span className="dirty">*</span>}</span>
            {(() => {
              const extra = (p.worktrees ?? []).filter((wt) => !wt.isMain).length;
              return extra > 0 ? (
                <span className="worktree-count">+ {extra} worktree{extra !== 1 ? 's' : ''}</span>
              ) : null;
            })()}
          </div>
        ) : null}
      </div>
    );
  }

  return (
    <div data-testid="workspace-page">
      <div className="projects-grid">
        {projects.map(renderProjectCard)}
      </div>

      {projects.length === 0 && <p className="empty">No projects in this workspace.</p>}

      {standaloneProjects.length > 0 && (
        <>
          <div className="standalone-section-header">Standalone Projects</div>
          <div className="projects-grid">
            {standaloneProjects.map(renderProjectCard)}
          </div>
        </>
      )}

      {/* Delete modal */}
      <div className={`modal-overlay${deleteTarget ? ' open' : ''}`} onClick={() => setDeleteTarget(null)}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
          <h3>Delete {deleteTarget?.name}?</h3>
          <p>This will permanently delete the entire project directory.</p>
          {deleteInfo && (
            <div className="modal-info">
              <div>{deleteInfo.fileCount} thoughts file{deleteInfo.fileCount !== 1 ? 's' : ''}</div>
              {deleteInfo.dirty && <div className="warning">&#9888; Has unstaged changes</div>}
              {deleteInfo.unpushedCommits > 0 && (
                <div className="warning">&#9888; {deleteInfo.unpushedCommits} unpushed commit{deleteInfo.unpushedCommits !== 1 ? 's' : ''}</div>
              )}
            </div>
          )}
          <div className="modal-actions">
            <button className="btn-cancel" onClick={() => setDeleteTarget(null)}>Cancel</button>
            <button className="btn-delete" onClick={confirmDelete}>Delete</button>
          </div>
        </div>
      </div>
    </div>
  );
}
