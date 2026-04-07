import { type ReactNode } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import type { APIFile, APIFavoriteEntry, APIProject, APIFileGroupView, APIFileInReview } from '../types';
import TableOfContents from './TableOfContents';
import type { Heading } from './TableOfContents';

export interface ProjectSidebarProps {
  activeProject: APIProject;
  activeWorktree: string;
  isFilePage: boolean;
  headings: Heading[];
  projectFiles: APIFileGroupView[];
  favorites: APIFavoriteEntry[];
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
  onToggleFavorite: (path: string, kind: 'file' | 'tree', favorited: boolean) => void;
  onFileClick: (e: React.MouseEvent, filePath: string, allFilePaths: string[]) => void;
  onFileContextMenu: (e: React.MouseEvent, file: { path: string; sourceType?: string }, source: APIFileGroupView) => void;
  onSourceContextMenu: (e: React.MouseEvent, group: APIFileGroupView) => void;
}

type SidebarFile = Pick<APIFile, 'path' | 'name' | 'title' | 'fileType' | 'dir' | 'displayPath'>;

interface TreeNode {
  name: string;
  path: string;
  actualPath?: string;
  isDir: boolean;
  children: TreeNode[];
  file?: SidebarFile;
}

function buildFileTree(files: SidebarFile[]) {
  const root: TreeNode = { name: '', path: '', isDir: true, children: [] };
  for (const file of files) {
    const treePath = file.displayPath || file.path;
    const parts = treePath.split('/').filter(Boolean);
    if (parts.length === 0) continue;
    let node = root;
    for (let i = 0; i < parts.length - 1; i++) {
      const dirPath = parts.slice(0, i + 1).join('/');
      let child = node.children.find(candidate => candidate.isDir && candidate.path === dirPath);
      if (!child) {
        child = { name: parts[i], path: dirPath, isDir: true, children: [] };
        node.children.push(child);
      }
      node = child;
    }
    node.children.push({
      name: parts[parts.length - 1],
      path: treePath,
      actualPath: file.path,
      isDir: false,
      children: [],
      file,
    });
  }
  function compact(node: TreeNode): TreeNode {
    node.children = node.children.map(compact);
    if (node.isDir && node.children.length === 1 && node.children[0].isDir) {
      const child = node.children[0];
      const name = node.name ? `${node.name}/${child.name}` : child.name;
      return { ...child, name };
    }
    return node;
  }
  return compact(root);
}

function flattenTree(node: TreeNode): string[] {
  const paths: string[] = [];
  for (const child of node.children) {
    if (child.isDir) {
      paths.push(...flattenTree(child));
    } else {
      paths.push(child.actualPath || child.path);
    }
  }
  return paths;
}

function fileUrl(activeProject: APIProject, activeWorktree: string, file: { path: string }) {
  const base = `/file/${activeProject.qualifiedName}`;
  const wt = activeWorktree ? `@${activeWorktree}` : '';
  return `${base}${wt}/${file.path}`;
}

export default function ProjectSidebar({
  activeProject,
  activeWorktree,
  isFilePage,
  headings,
  projectFiles,
  favorites,
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
  onToggleFavorite,
  onFileClick,
  onFileContextMenu,
  onSourceContextMenu,
}: ProjectSidebarProps) {
  const navigate = useNavigate();

  const favoriteFilePaths = new Set(favorites.filter(entry => entry.kind === 'file').map(entry => entry.path));
  const favoriteDirPaths = new Set(favorites.filter(entry => entry.kind === 'tree').map(entry => entry.path));
  const favoriteGroup: APIFileGroupView = {
    name: '__favorites__',
    source: '__favorites__',
    sourceType: 'favorites',
    auto: false,
    files: [],
  };
  const allFavoriteFilePaths = Array.from(new Set(
    favorites.flatMap(entry => entry.kind === 'file' ? [entry.path] : entry.files.map(file => file.path)),
  ));

  function handleFileRowClick(e: React.MouseEvent, path: string, allFilePaths: string[]) {
    onFileClick(e, path, allFilePaths);
    if (e.defaultPrevented) return;
    navigate(fileUrl(activeProject, activeWorktree, { path }));
  }

  function renderFavoriteToggle(path: string, kind: 'file' | 'tree', favorited: boolean) {
    return (
      <button
        type="button"
        className={`favorite-toggle${favorited ? ' active' : ''}`}
        title={favorited ? 'Remove from Favorites' : 'Add to Favorites'}
        aria-label={favorited ? 'Remove from Favorites' : 'Add to Favorites'}
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          onToggleFavorite(path, kind, favorited);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
        }}
      >
        {favorited ? '★' : '☆'}
      </button>
    );
  }

  function renderFileRow(
    key: string,
    label: string,
    path: string,
    allFilePaths: string[],
    group: APIFileGroupView,
    file?: SidebarFile,
  ) {
    const isActive = currentFilePath === path;
    const inReview = !!projectReviews[path];
    const isSelected = selected.has(path);
    return (
      <div
        key={key}
        className={`tree-item${isActive ? ' active' : ''}${isSelected ? ' selected' : ''}`}
        onClick={(e) => handleFileRowClick(e, path, allFilePaths)}
        onContextMenu={(e) => onFileContextMenu(e, { path, sourceType: group.sourceType }, group)}
      >
        {renderFavoriteToggle(path, 'file', favoriteFilePaths.has(path))}
        <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
        <span className="label" title={file?.title || label}>{label}</span>
        {file?.fileType && file.fileType !== 'other' && <span className={`badge-file-type badge-file-type-${file.fileType}`}>{file.fileType}</span>}
        {inReview && <span className="badge-review">in review</span>}
      </div>
    );
  }

  function renderTreeNode(
    node: TreeNode,
    sourceKey: string,
    group: APIFileGroupView,
    allFilePaths: string[],
    dirPathPrefix = '',
  ): ReactNode {
    return node.children.map(child => {
      if (child.isDir) {
        const dirPath = dirPathPrefix ? `${dirPathPrefix}/${child.path}` : child.path;
        const dirKey = `${sourceKey}:${dirPath}`;
        const isDirExpanded = expandedDirs.has(dirKey);
        return (
          <div key={dirKey}>
            <div className="tree-item" onClick={() => onToggleDir(dirKey)}>
              {renderFavoriteToggle(dirPath, 'tree', favoriteDirPaths.has(dirPath))}
              <span className={`chevron${isDirExpanded ? ' open' : ''}`}>▶</span>
              <span className="label" title={child.name + '/'}>{child.name}/</span>
            </div>
            {isDirExpanded && (
              <div className="tree-children">
                {renderTreeNode(child, sourceKey, group, allFilePaths, dirPathPrefix)}
              </div>
            )}
          </div>
        );
      }
      const path = child.actualPath || child.path;
      return renderFileRow(
        path,
        child.file?.title || child.name,
        path,
        allFilePaths,
        group,
        child.file,
      );
    });
  }

  function renderFavoriteEntry(entry: APIFavoriteEntry) {
    if (entry.kind === 'file') {
      return renderFileRow(
        entry.id,
        entry.label,
        entry.path,
        allFavoriteFilePaths,
        favoriteGroup,
        entry.files[0],
      );
    }

    const dirKey = `__favorites__:${entry.path}`;
    const isExpanded = expandedDirs.has(dirKey);
    const tree = buildFileTree(entry.files);
    const isEmpty = entry.files.length === 0;

    return (
      <div key={entry.id}>
        <div className={`tree-item${isEmpty ? ' deemphasized' : ''}`} onClick={isEmpty ? undefined : () => onToggleDir(dirKey)}>
          {renderFavoriteToggle(entry.path, 'tree', true)}
          {isEmpty ? (
            <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
          ) : (
            <span className={`chevron${isExpanded ? ' open' : ''}`}>▶</span>
          )}
          <span className="label" title={entry.label + '/'}>{entry.label}/</span>
          <span className="source-count">{entry.files.length}</span>
        </div>
        {isExpanded && !isEmpty && (
          <div className="tree-children">
            {renderTreeNode(tree, '__favorites__', favoriteGroup, allFavoriteFilePaths, entry.path)}
          </div>
        )}
      </div>
    );
  }

  return (
    <>
      <div className="breadcrumb-bar">
        <Link to="/" className="breadcrumb-home" title="Home">⌂</Link>
        <span className="sep">/</span>
        <Link to={`/project/${activeProject.qualifiedName}${activeWorktree ? `@${activeWorktree}` : ''}`} className="current">
          {activeProject.workspace ? `${activeProject.workspace} / ` : ''}{activeProject.name}
        </Link>
        {activeProject.agentConnected && <span className="agent-dot" />}
      </div>
      {activeProject.worktrees && activeProject.worktrees.length > 1 ? (
        <div className="worktree-selector-row" ref={worktreeDropdownRef} onClick={() => onSetShowWorktreeDropdown(!showWorktreeDropdown)}>
          {(() => {
            const wt = activeProject.worktrees!.find(candidate => activeWorktree ? candidate.name === activeWorktree : candidate.isMain);
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
        headings.length > 0 ? <TableOfContents headings={headings} /> : null
      ) : (
        <>
          {(() => {
            const favoritesExpanded = expandedSources.has('__favorites__');
            const favoritesEmpty = favorites.length === 0;
            return (
              <div>
                <div
                  className={`source-header${favoritesEmpty ? ' deemphasized' : ''}`}
                  onClick={favoritesEmpty ? undefined : () => onToggleSource('__favorites__')}
                >
                  {favoritesEmpty ? (
                    <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                  ) : (
                    <span className={`chevron${favoritesExpanded ? ' open' : ''}`}>▶</span>
                  )}
                  <span>{favoritesEmpty ? 'No Favorites' : 'Favorites'}</span>
                  {!favoritesEmpty && <span className="source-count">{favorites.length}</span>}
                </div>
                {favoritesExpanded && !favoritesEmpty && (
                  <div className="source-body">
                    {favorites.map(renderFavoriteEntry)}
                  </div>
                )}
              </div>
            );
          })()}

          {(() => {
            const badgeCounts = new Map<string, number>();
            for (const group of projectFiles) {
              if (group.badgeText) {
                badgeCounts.set(group.badgeText, (badgeCounts.get(group.badgeText) || 0) + 1);
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
