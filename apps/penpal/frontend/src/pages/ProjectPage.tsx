import { useLocation } from 'react-router-dom';
import { parseProjectWorktree } from '../utils/worktree';

// E-PENPAL-PROJECT-WELCOME: welcome screen when viewing project with no file open.
// File browsing is handled by the sidebar in Layout.tsx.
export default function ProjectPage() {
  const location = useLocation();
  const qnRaw = location.pathname.replace(/^\/project\//, '');
  const { project: qn } = parseProjectWorktree(qnRaw);
  const projectName = qn.split('/').pop() || qn;

  return (
    <div data-testid="project-page" className="welcome-screen">
      <h2>{projectName}</h2>
      <p>Expand a source in the sidebar to browse files.</p>
    </div>
  );
}
