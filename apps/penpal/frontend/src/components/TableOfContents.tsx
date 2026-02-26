export interface Heading {
  level: 1 | 2 | 3;
  text: string;
  id: string;
}

interface TableOfContentsProps {
  headings: Heading[];
}

export default function TableOfContents({ headings }: TableOfContentsProps) {
  if (headings.length === 0) return null;

  return (
    <div className="sidebar-card">
      <div className="sidebar-card-title">On this page</div>
      <nav className="sidebar-card-nav">
        {headings.map((h) => (
          <a key={h.id} href={`#${h.id}`} className={`level-${h.level}`}>
            {h.text}
          </a>
        ))}
      </nav>
    </div>
  );
}
