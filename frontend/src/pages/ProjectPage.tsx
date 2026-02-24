import { useParams } from 'react-router-dom';

export default function ProjectPage() {
  const { qualifiedName } = useParams<{ qualifiedName: string }>();
  return (
    <div data-testid="project-page">
      <h1>Project: {qualifiedName}</h1>
      <p>File list coming in PR 5.</p>
    </div>
  );
}
