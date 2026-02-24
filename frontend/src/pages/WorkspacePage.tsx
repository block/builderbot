import { useParams } from 'react-router-dom';

export default function WorkspacePage() {
  const { name } = useParams<{ name: string }>();
  return (
    <div data-testid="workspace-page">
      <h1>Workspace: {name}</h1>
      <p>Project cards coming in PR 5.</p>
    </div>
  );
}
