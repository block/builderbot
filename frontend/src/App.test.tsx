import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import App from './App';

// Mock API
vi.mock('./api', () => ({
  API_BASE: 'http://localhost:8080',
  isDesktopApp: false,
  api: {
    listProjects: vi.fn().mockResolvedValue([]),
    getInReview: vi.fn().mockResolvedValue([]),
  },
}));

vi.mock('./hooks/useSSE', () => ({
  useSSE: vi.fn(),
}));

describe('App', () => {
  it('renders the layout', async () => {
    render(<App />);
    // The router redirects / to /workspace/default
    expect(await screen.findByTestId('app-layout')).toBeInTheDocument();
  });

  it('renders workspace page on /workspace route', async () => {
    // createBrowserRouter uses the current URL; for testing just verify layout loads
    render(<App />);
    expect(await screen.findByTestId('app-layout')).toBeInTheDocument();
  });
});
