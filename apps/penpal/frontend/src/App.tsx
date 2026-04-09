import { createBrowserRouter, RouterProvider } from 'react-router-dom';
import Layout from './components/Layout';
import ProjectPage from './pages/ProjectPage';
import FilePage from './pages/FilePage';
import RecentPage from './pages/RecentPage';
import InReviewPage from './pages/InReviewPage';

// E-PENPAL-HOME-DEFAULT: home view welcome screen.
function HomePage() {
  return (
    <div className="welcome-screen">
      <h2>Penpal</h2>
      <p>Select a project from the sidebar to browse its files.</p>
    </div>
  );
}

// Vite sets import.meta.env.BASE_URL from the `base` config (e.g. '/app/' for
// Go-served builds, '/' for dev/Tauri). Strip trailing slash for router basename.
const basename = import.meta.env.BASE_URL.replace(/\/+$/, '') || '/';

const router = createBrowserRouter([
  {
    element: <Layout />,
    children: [
      { path: '/', element: <HomePage /> },
      { path: '/project/*', element: <ProjectPage /> },
      { path: '/file/*', element: <FilePage /> },
      { path: '/recent', element: <RecentPage /> },
      { path: '/in-review', element: <InReviewPage /> },
    ],
  },
], { basename });

export default function App() {
  return <RouterProvider router={router} />;
}
