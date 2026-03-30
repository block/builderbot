import type { Tab } from '../hooks/useTabs';

export interface TabBarProps {
  tabs: Tab[];
  activeTabId: string;
  onActivateTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  onNewTab: () => void;
}

// E-PENPAL-TABS: tab bar with tab management — activate, close, and new tab.
export default function TabBar({
  tabs,
  activeTabId,
  onActivateTab,
  onCloseTab,
  onNewTab,
}: TabBarProps) {
  return (
    <div className="tab-bar" data-testid="topbar-tabs">
      {tabs.map(tab => (
        <button
          key={tab.id}
          className={`tab-bar-tab${tab.id === activeTabId ? ' active' : ''}`}
          onClick={() => onActivateTab(tab.id)}
          onAuxClick={(e) => { if (e.button === 1) onCloseTab(tab.id); }}
        >
          <span className="tab-title" title={tab.title}>{tab.title}</span>
          {tabs.length > 1 && (
            <span className="tab-close" onClick={(e) => { e.stopPropagation(); onCloseTab(tab.id); }}>×</span>
          )}
        </button>
      ))}
      <button className="tab-bar-new" onClick={onNewTab} aria-label="New tab">+</button>
    </div>
  );
}
