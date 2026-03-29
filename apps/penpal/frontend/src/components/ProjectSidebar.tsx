import { type ReactNode } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import type { APIProject, APIFileGroupView, APIFileInReview } from '../types';
import TableOfContents from './TableOfContents';
import type { Heading } from './TableOfContents';

export interface ProjectSidebarProps {
  activeProject: APIProject;
  activeWorktree: string;
  isFilePage: boolean;
  headings: Heading[];
  projectFiles: APIFileGroupView[];
  projectReviews: Record<string, APIFileInReview>;
  expandedSources: Set<string>;
  expandedDirs: Set<string>;
  selected: Set<string>;
  currentFilePath: string;
  showWorktreeDropdown: boolean;
  worktreeDropdownRef: React.RefObject<HTMLDivElement | null>;
  onSetShowWorktreeDropdown: (show: boolean) => void;
  onToggleSource: (name: string) => void;
  onToggleDir: (key: string) => void;
  onFileClick: (e: React.MouseEvent, filePath: string, allFilePaths: string[]) => void;
  onFileContextMenu: (e: React.MouseEvent, file: { path: string; sourceType?: string }, source: APIFileGroupView) => void;
  onSourceContextMenu: (e: React.MouseEvent, group: APIFileGroupView) => void;
}

// Build a tree structure from flat file list, then compact single-child directory chains
function buildFileTree(files: { path: string; name: string; title?: string; fileType?: string; dir?: string }[]) {
  interface TreeNode {
    name: string;
    path: string;
    isDir: boolean;
    children: TreeNode[];
    file?: typeof files[0];
  }
  const root: TreeNode = { name: '', path: '', isDir: true, children: [] };
  for (const file of files) {
    const parts = file.path.split('/');
    let node = root;
    for (let i = 0; i < parts.length - 1; i++) {
      const dirPath = parts.slice(0, i + 1).join('/');
      let child = node.children.find(c => c.isDir && c.path === dirPath);
      if (!child) {
        child = { name: parts[i], path: dirPath, isDir: true, children: [] };
        node.children.push(child);
      }
      node = child;
    }
    node.children.push({ name: file.name, path: file.path, isDir: false, children: [], file });
  }
  // Compact single-child directory chains: a/ -> b/ -> c/ becomes a/b/c/
  function compact(node: TreeNode): TreeNode {
    node.children = node.children.map(compact);
    if (node.isDir && node.children.length === 1 && node.children[0].isDir) {
      const child = node.children[0];
      return { ...child, name: node.name + '/' + child.name };
    }
    return node;
  }
  return compact(root);
}

// Flatten a file tree into visual (depth-first) order for shift-click ranges.
function flattenTree(node: ReturnType<typeof buildFileTree>): string[] {
  const paths: string[] = [];
  for (const child of node.children) {
    if (child.isDir) {
      paths.push(...flattenTree(child));
    } else {
      paths.push(child.path);
    }
  }
  return paths;
}

// Build file URL for a project file
function fileUrl(activeProject: APIProject, activeWorktree: string, file: { path: string }) {
  const base = `/file/${activeProject.qualifiedName}`;
  const wt = activeWorktree ? `@${activeWorktree}` : '';
  return `${base}${wt}/${file.path}`;
}

// E-PENPAL-PROJECT-RESOLVE, E-PENPAL-BREADCRUMB, E-PENPAL-WORKTREE-DROPDOWN,
// E-PENPAL-SOURCE-SECTIONS, E-PENPAL-FILE-TREE, E-PENPAL-FILE-TREE-ITEM:
// project view sidebar with breadcrumb, worktree dropdown, source file trees.
export default function ProjectSidebar({
  activeProject,
  activeWorktree,
  isFilePage,
  headings,
  projectFiles,
  projectReviews,
  expandedSources,
  expandedDirs,
  selected,
  currentFilePath,
  showWorktreeDropdown,
  worktreeDropdownRef,
  onSetShowWorktreeDropdown,
  onToggleSource,
  onToggleDir,
  onFileClick,
  onFileContextMenu,
  onSourceContextMenu,
}: ProjectSidebarProps) {
  const navigate = useNavigate();

  function renderTreeNode(
    node: ReturnType<typeof buildFileTree>,
    sourceKey: string,
    group: APIFileGroupView,
    allFilePaths: string[],
  ): ReactNode {
    return node.children.map(child => {
      if (child.isDir) {
        const dirKey = `${sourceKey}:${child.path}`;
        const isDirExpanded = expandedDirs.has(dirKey);
        return (
          <div key={dirKey}>
            <div className="tree-item" onClick={() => onToggleDir(dirKey)}>
              <span className={`chevron${isDirExpanded ? ' open' : ''}`}>▶</span>
              <span className="label" title={child.name + '/'}>{child.name}/</span>
            </div>
            {isDirExpanded && (
              <div className="tree-children">
                {renderTreeNode(child, sourceKey, group, allFilePaths)}
              </div>
            )}
          </div>
        );
      }
      const url = fileUrl(activeProject, activeWorktree, child);
      const isActive = currentFilePath === child.path;
      const inReview = !!projectReviews[child.path];
      const isSelected = selected.has(child.path);
      return (
        <Link
          key={child.path}
          to={url}
          className={`tree-item${isActive ? ' active' : ''}${isSelected ? ' selected' : ''}`}
          onClick={(e) => onFileClick(e, child.path, allFilePaths)}
          onContextMenu={(e) => onFileContextMenu(e, { path: child.path, sourceType: group.sourceType }, group)}
        >
          <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
          <span className="label" title={child.file?.title || child.name}>{child.file?.title || child.name}</span>
          {child.file?.fileType && child.file.fileType !== 'other' && <span className={`badge-file-type badge-file-type-${child.file.fileType}`}>{child.file.fileType}</span>}
          {inReview && <span className="badge-review">in review</span>}
        </Link>
      );
    });
  }

  return (
    <>
      {/* Breadcrumb bar */}
      <div className="breadcrumb-bar">
        <Link to="/" className="breadcrumb-home" title="Home">⌂</Link>
        <span className="sep">/</span>
        <Link to={`/project/${activeProject.qualifiedName}${activeWorktree ? `@${activeWorktree}` : ''}`} className="current">
          {activeProject.workspace ? `${activeProject.workspace} / ` : ''}{activeProject.name}
        </Link>
        {activeProject.agentConnected && <span className="agent-dot" />}
      </div>
      {/* E-PENPAL-WORKTREE-DROPDOWN: full-width worktree selector row below breadcrumb */}
      {activeProject.worktrees && activeProject.worktrees.length > 1 ? (
        <div className="worktree-selector-row" ref={worktreeDropdownRef} onClick={() => onSetShowWorktreeDropdown(!showWorktreeDropdown)}>
          {(() => {
            const wt = activeProject.worktrees!.find(wt => activeWorktree ? wt.name === activeWorktree : wt.isMain);
            const isMain = !wt || wt.isMain;
            return isMain ? 'main repo' : (
              <>
                <svg className="worktree-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
                  <circle cx="6" cy="5" r="2" /><circle cx="18" cy="5" r="2" /><circle cx="18" cy="19" r="2" />
                  <path d="M8 5h8" /><path d="M8 5v8a6 6 0 0 0 6 6h2" />
                </svg>
                {wt!.name}
              </>
            );
          })()}
          {showWorktreeDropdown && (
            <div className="worktree-dropdown-menu">
              {activeProject.worktrees.map(wt => {
                const isActive = wt.isMain ? !activeWorktree : activeWorktree === wt.name;
                const url = wt.isMain
                  ? `/project/${activeProject.qualifiedName}`
                  : `/project/${activeProject.qualifiedName}@${wt.name}`;
                return (
                  <button
                    key={wt.name}
                    className={isActive ? 'active' : ''}
                    title={wt.branch ? `branch: ${wt.branch}` : undefined}
                    onClick={(e) => {
                      e.stopPropagation();
                      onSetShowWorktreeDropdown(false);
                      navigate(url);
                    }}
                  >
                    {!wt.isMain && (
                      <svg className="worktree-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
                        <circle cx="6" cy="5" r="2" /><circle cx="18" cy="5" r="2" /><circle cx="18" cy="19" r="2" />
                        <path d="M8 5h8" /><path d="M8 5v8a6 6 0 0 0 6 6h2" />
                      </svg>
                    )}
                    {wt.isMain ? 'main repo' : wt.name}
                  </button>
                );
              })}
            </div>
          )}
        </div>
      ) : (
        <div className="worktree-selector-row deemphasized">no worktrees</div>
      )}

      {isFilePage ? (
        /* File view: only show table of contents below breadcrumb */
        headings.length > 0 ? <TableOfContents headings={headings} /> : null
      ) : (
        /* Project view: show source file trees */
        <>
          {/* E-PENPAL-FE-SRC-DISAMBIG: compute badge texts that appear on multiple groups */}
          {(() => {
            const badgeCounts = new Map<string, number>();
            for (const g of projectFiles) {
              if (g.badgeText) {
                badgeCounts.set(g.badgeText, (badgeCounts.get(g.badgeText) || 0) + 1);
              }
            }
            const duplicatedBadges = new Set<string>();
            for (const [badge, count] of badgeCounts) {
              if (count > 1) duplicatedBadges.add(badge);
            }
            return projectFiles.map((group) => {
            const isExpanded = expandedSources.has(group.name);
            const tree = isExpanded ? buildFileTree(group.files) : null;
            const allFilePaths = tree ? flattenTree(tree) : [];

            const isEmpty = !group.files || group.files.length === 0;
            const isVirtual = group.source === '__all_markdown__';
            const displayName = isEmpty && isVirtual ? 'No Markdown Found' : group.name;

            return (
              <div key={group.name}>
                <div
                  className={`source-header${isEmpty && isVirtual ? ' deemphasized' : ''}`}
                  onClick={isEmpty ? undefined : () => onToggleSource(group.name)}
                  onContextMenu={isEmpty ? undefined : (e) => onSourceContextMenu(e, group)}
                >
                  {isEmpty ? (
                    <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                  ) : (
                    <span className={`chevron${isExpanded ? ' open' : ''}`}>▶</span>
                  )}
                  {group.badgeText ? (
                    <span
                      className="badge-source"
                      style={{ background: group.badgeBg, color: group.badgeColor }}
                    >
                      {group.badgeText}
                    </span>
                  ) : (
                    <span>{displayName}</span>
                  )}
                  {/* E-PENPAL-FE-SRC-DISAMBIG: show source path when badge is shared by multiple groups */}
                  {group.badgeText && duplicatedBadges.has(group.badgeText) && (
                    <span className="source-disambig" title={group.name}>{group.name}</span>
                  )}
                  {!isEmpty && <span className="source-count">{group.files.length}</span>}
                </div>
                {isExpanded && tree && (
                  <div className="source-body">
                    {renderTreeNode(tree, group.name, group, allFilePaths)}
                  </div>
                )}
              </div>
            );
          });
          })()}

          {/* Per-project In Review section */}
          {(() => {
            const reviewFiles = Object.keys(projectReviews);
            const isEmpty = reviewFiles.length === 0;
            const isExpanded = expandedSources.has('__in_review__');
            return (
              <div>
                <div
                  className={`source-header${isEmpty ? ' deemphasized' : ''}`}
                  onClick={isEmpty ? undefined : () => onToggleSource('__in_review__')}
                >
                  {isEmpty ? (
                    <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                  ) : (
                    <span className={`chevron${isExpanded ? ' open' : ''}`}>▶</span>
                  )}
                  <span>{isEmpty ? 'Nothing in Review' : 'In Review'}</span>
                  {!isEmpty && <span className="source-count">{reviewFiles.length}</span>}
                </div>
                {isExpanded && !isEmpty && (
                  <div className="source-body">
                    {reviewFiles.map(filePath => {
                      const url = fileUrl(activeProject, activeWorktree, { path: filePath });
                      const name = filePath.split('/').pop() || filePath;
                      const isActive = currentFilePath === filePath;
                      return (
                        <Link key={filePath} to={url} className={`tree-item${isActive ? ' active' : ''}`}>
                          <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                          <span className="label" title={name}>{name}</span>
                        </Link>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })()}

          {/* Per-project Recent section -- currently always empty (TODO: fetch per-project recent files) */}
          <div>
            <div className="source-header deemphasized">
              <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
              <span>Nothing Recent</span>
            </div>
          </div>
        </>
      )}
    </>
  );
}
