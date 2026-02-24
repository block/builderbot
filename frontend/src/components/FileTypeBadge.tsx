export default function FileTypeBadge({ type }: { type?: string }) {
  if (!type || type === 'other') return null;
  return <span className={`file-type ${type}`}>{type}</span>;
}
