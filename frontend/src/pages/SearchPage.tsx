import { useSearchParams } from 'react-router-dom';

export default function SearchPage() {
  const [params] = useSearchParams();
  const query = params.get('q') || '';
  return (
    <div data-testid="search-page">
      <h1>Search</h1>
      {query && <p>Results for: {query}</p>}
      <p>Search results coming in PR 5.</p>
    </div>
  );
}
