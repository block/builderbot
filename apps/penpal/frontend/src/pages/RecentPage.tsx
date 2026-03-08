import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { api } from '../api';
import { useSSE } from '../hooks/useSSE';
import FileTypeBadge from '../components/FileTypeBadge';
import type { APIFile, SSEEvent } from '../types';

function debounce<T extends (...args: never[]) => void>(fn: T, ms: number): T {
  let timer: ReturnType<typeof setTimeout>;
  return ((...args: Parameters<T>) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  }) as T;
}

export default function RecentPage() {
  const [files, setFiles] = useState<APIFile[]>([]);

  const refresh = useCallback(() => {
    api.getRecentFiles().then(setFiles).catch(() => {});
  }, []);

  useEffect(() => { api.clearFocus().catch(() => {}); }, []);
  useEffect(() => { refresh(); }, [refresh]);

  const debouncedRefresh = useCallback(() => debounce(refresh, 200)(), [refresh]);

  useSSE(
    useCallback(
      (event: SSEEvent) => {
        if (event.type === 'files' || event.type === 'comments') debouncedRefresh();
      },
      [debouncedRefresh],
    ),
  );

  return (
    <div data-testid="recent-page">
      <h1 style={{ fontWeight: 600, marginBottom: 8 }}>Recent Files</h1>
      <p style={{ color: 'var(--text-subtle)', marginBottom: 20 }}>Recently active files across all projects</p>

      {files.map((f) => (
        <div key={`${f.project}/${f.path}`} className="file-row recent-file">
          <div className="file-left">
            <FileTypeBadge type={f.fileType} />
            <span className="file-name">
              <Link to={`/file/${(f.project || '')}/${f.path}`}>
                {f.title || f.path}
              </Link>
              <span className="file-subtitle">{f.title ? `${f.project}/${f.path}` : f.project}</span>
            </span>
          </div>
          <div className="file-right">
            {f.activityType && (
              <span className={`activity-label activity-${f.activityType}`}>
                {f.activityType} {f.activityAge}
              </span>
            )}
            <span className="file-age">{f.age}</span>
          </div>
        </div>
      ))}

      {files.length === 0 && <p className="empty">No recent files.</p>}
    </div>
  );
}
