import { useEffect, useRef, useState } from 'react';
import { Link, NavLink, useNavigate } from 'react-router-dom';
import type { APIProject } from '../types';
import type { ProjectSortOrder } from '../hooks/useProjectSort';
import type { ContextMenuItem } from './ContextMenu';

export interface HomeSidebarProps {
  visibleWorkspaces: string[];
  sortedWorkspaceProjects: Map<string, APIProject[]>;
  standaloneProjects: APIProject[];
  reviewCount: number;
  expandedWorkspaces: Set<string>;
  expandedWorktreeProjects: Set<string>;
  onToggleWorkspace: (ws: string) => void;
  onToggleWorktreeProject: (qn: string) => void;
  onShowContextMenu: (e: React.MouseEvent, items: ContextMenuItem[]) => void;
  onRemoveWorkspace: (ws: string) => void;
  onCloseStandaloneProject: (p: APIProject) => void;
  onShowAddModal: () => void;
  sortOrder: ProjectSortOrder;
  onSetSortOrder: (order: ProjectSortOrder) => void;
  showEmpty: boolean;
  onSetShowEmpty: (show: boolean) => void;
}

// E-PENPAL-HOME-SIDEBAR: home view sidebar tree with workspaces, standalone projects, global nav links.
// E-PENPAL-FE-HOME-LABEL: show "Home" label next to house icon on home screen.
// E-PENPAL-VIEW-OPTIONS: view options popover with sort order and show-empty toggle.
export default function HomeSidebar({
  visibleWorkspaces,
  sortedWorkspaceProjects,
  standaloneProjects,
  reviewCount,
  expandedWorkspaces,
  expandedWorktreeProjects,
  onToggleWorkspace,
  onToggleWorktreeProject,
  onShowContextMenu,
  onRemoveWorkspace,
  onCloseStandaloneProject,
  onShowAddModal,
  sortOrder,
  onSetSortOrder,
  showEmpty,
  onSetShowEmpty,
}: HomeSidebarProps) {
  const navigate = useNavigate();

  // E-PENPAL-VIEW-OPTIONS: view options panel state
  const [showViewOptions, setShowViewOptions] = useState(false);
  const viewOptionsPanelRef = useRef<HTMLDivElement>(null);

  // Close view options panel on outside click
  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (viewOptionsPanelRef.current && !viewOptionsPanelRef.current.contains(e.target as Node)) {
        setShowViewOptions(false);
      }
    }
    if (showViewOptions) {
      document.addEventListener('mousedown', handleClick);
      return () => document.removeEventListener('mousedown', handleClick);
    }
  }, [showViewOptions]);

  return (
    <>
      {/* E-PENPAL-FE-HOME-LABEL: show "Home" label next to house icon on home screen */}
      {/* E-PENPAL-VIEW-OPTIONS: view options popover with sort order and show-empty toggle */}
      <div className="sidebar-home-header">
        <span className="home-icon">⌂</span>
        <span className="home-label">Home</span>
        <div className="view-options-wrap" ref={viewOptionsPanelRef}>
          <button
            className="view-options-btn"
            title="View options"
            aria-label="View options"
            aria-haspopup="true"
            aria-expanded={showViewOptions}
            onClick={() => setShowViewOptions((prev) => !prev)}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" width="14" height="14" aria-hidden="true">
              <line x1="4" y1="6" x2="20" y2="6" />
              <circle cx="9" cy="6" r="2" />
              <line x1="4" y1="12" x2="20" y2="12" />
              <circle cx="15" cy="12" r="2" />
              <line x1="4" y1="18" x2="20" y2="18" />
              <circle cx="12" cy="18" r="2" />
            </svg>
          </button>
          {showViewOptions && (
            <div className="view-options-panel">
              <label className="view-options-label">
                Project order
                <select
                  value={sortOrder}
                  onChange={(e) => onSetSortOrder(e.target.value as ProjectSortOrder)}
                >
                  <option value="alpha">A→Z</option>
                  <option value="recent">Most Recent</option>
                </select>
              </label>
              <label className="view-options-label view-options-toggle">
                <input
                  type="checkbox"
                  checked={showEmpty}
                  onChange={(e) => onSetShowEmpty(e.target.checked)}
                />
                Show empty projects
              </label>
            </div>
          )}
        </div>
        <button
          title="Add workspace or project"
          onClick={onShowAddModal}
        >+</button>
      </div>

      {/* Workspaces */}
      {visibleWorkspaces.map(ws => {
        const isExpanded = expandedWorkspaces.has(ws);
        const wsProjects = sortedWorkspaceProjects.get(ws) || [];
        const hasAgent = wsProjects.some(p => p.agentConnected);
        return (
          <div key={ws}>
            <div
              className="tree-item"
              style={{ fontWeight: 500 }}
              onClick={() => onToggleWorkspace(ws)}
              onContextMenu={(e) => onShowContextMenu(e, [
                { label: 'Remove workspace', className: 'menu-danger', onClick: () => onRemoveWorkspace(ws) },
              ])}
            >
              <span className={`chevron${isExpanded ? ' open' : ''}`}>▶</span>
              <span className="label" title={ws}>{ws}</span>
              {hasAgent && <span className="agent-dot" />}
            </div>
            {isExpanded && (
              <div className="tree-children">
                {wsProjects.map(p => {
                  const hasWorktrees = p.worktrees && p.worktrees.length > 1;
                  const isWtExpanded = expandedWorktreeProjects.has(p.qualifiedName);
                  return (
                    <div key={p.qualifiedName}>
                      <div
                        className={`tree-item${p.fileCount === 0 ? ' deemphasized' : ''}`}
                        onClick={() => {
                          if (hasWorktrees) {
                            onToggleWorktreeProject(p.qualifiedName);
                          } else {
                            navigate(`/project/${p.qualifiedName}`);
                          }
                        }}
                      >
                        {hasWorktrees ? (
                          <span className={`chevron${isWtExpanded ? ' open' : ''}`}>▶</span>
                        ) : (
                          <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                        )}
                        <span className="label" title={p.branch ? `${p.name}\nbranch: ${p.branch}` : p.name}>{p.name}</span>
                        {p.agentConnected && <span className="agent-dot" />}
                      </div>
                      {hasWorktrees && isWtExpanded && (
                        <div className="tree-children">
                          {p.worktrees!.map(wt => {
                            const url = wt.isMain
                              ? `/project/${p.qualifiedName}`
                              : `/project/${p.qualifiedName}@${wt.name}`;
                            return (
                              <Link key={wt.name} to={url} className="tree-item worktree-item" title={wt.branch ? `branch: ${wt.branch}` : undefined}>
                                <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                                {!wt.isMain && (
                                  <svg className="worktree-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
                                    <circle cx="6" cy="5" r="2" /><circle cx="18" cy="5" r="2" /><circle cx="18" cy="19" r="2" />
                                    <path d="M8 5h8" /><path d="M8 5v8a6 6 0 0 0 6 6h2" />
                                  </svg>
                                )}
                                <span className="label" title={wt.isMain ? p.name : wt.name}>{wt.isMain ? p.name : wt.name}</span>
                              </Link>
                            );
                          })}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        );
      })}

      {/* Divider between workspaces and standalone projects */}
      {visibleWorkspaces.length > 0 && standaloneProjects.length > 0 && (
        <div className="home-section-divider" />
      )}

      {/* Standalone projects */}
      {standaloneProjects.map(p => (
        <Link
          key={p.qualifiedName}
          to={`/project/${p.qualifiedName}`}
          className={`tree-item${p.fileCount === 0 ? ' deemphasized' : ''}`}
          onContextMenu={(e) => onShowContextMenu(e, [
            { label: 'Close project', className: 'menu-muted', onClick: () => onCloseStandaloneProject(p) },
          ])}
        >
          <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
          <span className="label" title={p.branch ? `${p.name}\nbranch: ${p.branch}` : p.name}>{p.name}</span>
          {p.agentConnected && <span className="agent-dot" />}
        </Link>
      ))}

      {/* Divider before global nav */}
      {(visibleWorkspaces.length > 0 || standaloneProjects.length > 0) && (
        <div className="home-section-divider" />
      )}

      {/* Global In Review */}
      <NavLink
        to="/in-review"
        className={({ isActive }) =>
          `tree-item${isActive ? ' active' : ''}${reviewCount === 0 ? ' deemphasized' : ''}`
        }
      >
        <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
        <span className="label">In Review</span>
        {reviewCount > 0 && <span className="source-count">{reviewCount}</span>}
      </NavLink>

      {/* Global Recent */}
      <NavLink
        to="/recent"
        className={({ isActive }) => `tree-item${isActive ? ' active' : ''}`}
      >
        <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
        <span className="label">Recent</span>
      </NavLink>
    </>
  );
}
