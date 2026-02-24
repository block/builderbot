import { createBrowserRouter, Navigate, RouterProvider } from 'react-router-dom';
import Layout from './components/Layout';
import WorkspacePage from './pages/WorkspacePage';
import ProjectPage from './pages/ProjectPage';
import FilePage from './pages/FilePage';
import SearchPage from './pages/SearchPage';
import RecentPage from './pages/RecentPage';
import InReviewPage from './pages/InReviewPage';

const router = createBrowserRouter([
  {
    element: <Layout />,
    children: [
      { path: '/', element: <Navigate to="/workspace/default" replace /> },
      { path: '/workspace/:name', element: <WorkspacePage /> },
      { path: '/project/:qualifiedName', element: <ProjectPage /> },
      { path: '/file/:qualifiedName/*', element: <FilePage /> },
      { path: '/search', element: <SearchPage /> },
      { path: '/recent', element: <RecentPage /> },
      { path: '/in-review', element: <InReviewPage /> },
    ],
  },
]);

export default function App() {
  return <RouterProvider router={router} />;
}
