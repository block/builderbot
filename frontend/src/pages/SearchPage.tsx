import { useCallback, useEffect, useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { api } from '../api';
import FileTypeBadge from '../components/FileTypeBadge';
import type { SearchResponse } from '../types';

export default function SearchPage() {
  const [params] = useSearchParams();
  const query = params.get('q') || '';
  const [results, setResults] = useState<SearchResponse | null>(null);
  const [loading, setLoading] = useState(false);

  const doSearch = useCallback(() => {
    if (!query) { setResults(null); return; }
    setLoading(true);
    api.search(query)
      .then(setResults)
      .catch(() => setResults(null))
      .finally(() => setLoading(false));
  }, [query]);

  useEffect(() => { doSearch(); }, [doSearch]);

  const hasProjects = (results?.matchingProjects?.length ?? 0) > 0;
  const hasFiles = (results?.projectResults?.length ?? 0) > 0;
  const hasResults = hasProjects || hasFiles;

  return (
    <div data-testid="search-page">
      <h1 style={{ fontWeight: 600, marginBottom: 16 }}>Search</h1>

      {query && !loading && <p className="search-query-label">Results for: {query}</p>}

      {loading && <p className="empty">Searching...</p>}

      {query && results && !loading && (
        <>
          {hasResults && (
            <p className="search-stats">
              {hasProjects && (
                <>{results.matchingProjects!.length} project{results.matchingProjects!.length !== 1 ? 's' : ''}{hasFiles ? ', ' : ''}</>
              )}
              {hasFiles && (
                <>{results.totalFiles} file{results.totalFiles !== 1 ? 's' : ''}{results.totalFiles >= 100 ? ' (limited to 100)' : ''}</>
              )}
            </p>
          )}

          {hasProjects && (
            <>
              <h2 className="search-section-title">Projects</h2>
              <div className="search-projects">
                {results.matchingProjects!.map((p) => (
                  <div key={p.qualifiedName} className="search-project-card">
                    <div style={{ fontWeight: 600, fontSize: '1.05em', marginBottom: 4 }}>
                      <Link to={`/project/${p.qualifiedName}`}>{p.project}</Link>
                    </div>
                  </div>
                ))}
              </div>
            </>
          )}

          {hasFiles && (
            <>
              <h2 className="search-section-title">Thoughts</h2>
              {results.projectResults!.map((pr) => (
                <div key={pr.qualifiedName} className="project-group">
                  <div className="project-group-header">
                    <Link to={`/project/${pr.qualifiedName}`}>{pr.project}</Link>
                  </div>
                  {(pr.files || []).map((f) => (
                    <div key={f.path} className="file-row">
                      <div className="file-left">
                        <FileTypeBadge type={f.fileType} />
                        {f.nameMatch && <span className="match-type">name</span>}
                        <span className="file-name">
                          <Link to={`/file/${pr.qualifiedName}/${f.path}`}>{f.name}</Link>
                        </span>
                      </div>
                    </div>
                  ))}
                </div>
              ))}
            </>
          )}

          {!hasResults && <p className="no-results">No results found for "{query}"</p>}
        </>
      )}
    </div>
  );
}
