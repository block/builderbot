import { useEffect, useRef } from 'react';

// E-PENPAL-CONTEXT-MENU: shared right-click context menu component.
export interface ContextMenuItem {
  label: string;
  className?: string;
  onClick: () => void;
}

interface ContextMenuProps {
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
}

export default function ContextMenu({ x, y, items, onClose }: ContextMenuProps) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    }
    function handleKey(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('mousedown', handleClick);
    document.addEventListener('keydown', handleKey);
    return () => {
      document.removeEventListener('mousedown', handleClick);
      document.removeEventListener('keydown', handleKey);
    };
  }, [onClose]);

  // Adjust position to keep menu on screen
  useEffect(() => {
    if (!ref.current) return;
    const rect = ref.current.getBoundingClientRect();
    if (rect.right > window.innerWidth) {
      ref.current.style.left = `${x - rect.width}px`;
    }
    if (rect.bottom > window.innerHeight) {
      ref.current.style.top = `${y - rect.height}px`;
    }
  }, [x, y]);

  return (
    <div
      ref={ref}
      className="context-menu"
      style={{ position: 'fixed', left: x, top: y, zIndex: 1000 }}
    >
      {items.map((item, i) =>
        item.label === '---' ? (
          <div key={i} className="menu-divider" />
        ) : (
          <button
            key={item.label}
            className={item.className}
            onClick={() => { item.onClick(); onClose(); }}
          >
            {item.label}
          </button>
        ),
      )}
    </div>
  );
}
