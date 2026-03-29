import { Link } from 'react-router-dom';

export interface TopbarProps {
  canGoBack: boolean;
  canGoForward: boolean;
  goBack: () => void;
  goForward: () => void;
  searchQuery: string;
  onSearchQueryChange: (query: string) => void;
  onSearchSubmit: (e: React.FormEvent) => void;
  theme: string;
  onToggleTheme: () => void;
  isDesktopApp: boolean;
}

// E-PENPAL-EXTERNAL-LINKS: top bar with logo, back/forward nav, search, and theme toggle.
export default function Topbar({
  canGoBack,
  canGoForward,
  goBack,
  goForward,
  searchQuery,
  onSearchQueryChange,
  onSearchSubmit,
  theme,
  onToggleTheme,
  isDesktopApp,
}: TopbarProps) {
  return (
    <div
      className="topbar"
      {...(isDesktopApp ? { 'data-tauri-drag-region': '' } : {})}
    >
      <button className="topbar-nav" disabled={!canGoBack} onClick={goBack} aria-label="Go back">‹</button>
      <button className="topbar-nav" disabled={!canGoForward} onClick={goForward} aria-label="Go forward">›</button>
      <Link to="/" className="topbar-logo">
        Penpal
      </Link>
      <form className="topbar-search" onSubmit={onSearchSubmit}>
        <input
          type="search"
          name="q"
          placeholder="Search all thoughts..."
          value={searchQuery}
          onChange={(e) => onSearchQueryChange(e.target.value)}
        />
      </form>
      <button className="theme-toggle" onClick={onToggleTheme} aria-label="Toggle dark mode" title="Toggle dark mode">
        {theme === 'dark' ? '☾' : '☀'}
      </button>
    </div>
  );
}
