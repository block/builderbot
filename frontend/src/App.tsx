import { useEffect } from 'react';
import { createBrowserRouter, RouterProvider, useNavigate } from 'react-router-dom';
import Layout from './components/Layout';
import WorkspacePage from './pages/WorkspacePage';
import ProjectPage from './pages/ProjectPage';
import FilePage from './pages/FilePage';
import SearchPage from './pages/SearchPage';
import RecentPage from './pages/RecentPage';
import InReviewPage from './pages/InReviewPage';
import { api } from './api';

function IndexRedirect() {
  const navigate = useNavigate();

  useEffect(() => {
    api.listProjects().then((projects) => {
      // First workspace (matches Go handleIndex: s.cfg.Workspaces[0].DisplayName())
      const workspaceProject = projects.find((p) => p.origin === 'workspace' && p.workspace);
      if (workspaceProject) {
        navigate(`/workspace/${encodeURIComponent(workspaceProject.workspace)}`, { replace: true });
        return;
      }

      // First standalone project
      const standalone = projects.find((p) => p.origin === 'standalone');
      if (standalone) {
        navigate(`/project/${standalone.qualifiedName}`, { replace: true });
        return;
      }

      // Nothing configured
      navigate('/recent', { replace: true });
    }).catch(() => {
      navigate('/recent', { replace: true });
    });
  }, [navigate]);

  return null;
}

const router = createBrowserRouter([
  {
    element: <Layout />,
    children: [
      { path: '/', element: <IndexRedirect /> },
      { path: '/workspace/:name', element: <WorkspacePage /> },
      { path: '/project/*', element: <ProjectPage /> },
      { path: '/file/*', element: <FilePage /> },
      { path: '/search', element: <SearchPage /> },
      { path: '/recent', element: <RecentPage /> },
      { path: '/in-review', element: <InReviewPage /> },
    ],
  },
]);

export default function App() {
  return <RouterProvider router={router} />;
}
