import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { api } from '../api';
import { useSSE } from '../hooks/useSSE';
import FileTypeBadge from '../components/FileTypeBadge';
import type { ReviewGroup, SSEEvent } from '../types';

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

export default function InReviewPage() {
  const [groups, setGroups] = useState<ReviewGroup[]>([]);

  const refresh = useCallback(() => {
    api.getInReview().then(setGroups).catch(() => {});
  }, []);

  useEffect(() => { api.clearFocus().catch(() => {}); }, []);
  useEffect(() => { refresh(); }, [refresh]);

  const debouncedRefresh = useCallback(() => debounce(refresh, 200)(), [refresh]);

  useSSE(
    useCallback(
      (event: SSEEvent) => {
        if (event.type === 'comments' || event.type === 'agents') debouncedRefresh();
      },
      [debouncedRefresh],
    ),
  );

  return (
    <div data-testid="in-review-page">
      {groups.length > 0 ? (
        groups.map((group, i) => (
          <div key={i} className="source-section">
            <div className="source-header" style={{ cursor: 'default', flexWrap: 'wrap' }}>
              {group.badgeText && (
                <span
                  className="source-badge"
                  style={{ color: group.badgeColor, background: group.badgeBg }}
                >
                  {group.badgeText}
                </span>
              )}
              <span className="source-breadcrumb">
                {group.workspace && (
                  <>
                    <Link to={group.workspaceURL || `/workspace/${encodeURIComponent(group.workspace)}`}>
                      {group.workspace}
                    </Link>
                    <span className="breadcrumb-sep">/</span>
                  </>
                )}
                <Link to={`/project/${group.projectQN}`}>
                  {group.projectName}
                </Link>
                <span className="breadcrumb-sep">/</span>
                <Link
                  to={`/project/${group.projectQN}#source-${group.sourceName}`}
                  className="breadcrumb-source"
                >
                  {group.sourceName}
                </Link>
              </span>
              {(group.workingThreads ?? 0) > 0 && (
                <span style={{ marginLeft: 'auto' }}><WorkingIndicator /></span>
              )}
            </div>
            <ul className="files-list">
              {(group.files || []).map((file) => (
                <li key={file.path} className="file-row">
                  <div className="file-left">
                    <FileTypeBadge type={file.fileType} />
                    <span className="file-name">
                      <Link to={`/file/${file.project}/${file.path}`}>
                        {file.title || file.path}
                      </Link>
                      <span className="file-subtitle">{file.title ? file.path : '\u00A0'}</span>
                    </span>
                  </div>
                  <div className="file-right">
                    <span className="file-age">{file.age}</span>
                  </div>
                </li>
              ))}
            </ul>
          </div>
        ))
      ) : (
        <p className="empty">No files are currently in review.</p>
      )}
    </div>
  );
}
