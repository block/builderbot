import { useParams } from 'react-router-dom';

export default function FilePage() {
  const { qualifiedName, '*': filePath } = useParams();
  return (
    <div data-testid="file-page">
      <h1>File Viewer</h1>
      <p>
        Project: {qualifiedName}, Path: {filePath}
      </p>
      <p>Markdown rendering coming in PR 4.</p>
    </div>
  );
}
