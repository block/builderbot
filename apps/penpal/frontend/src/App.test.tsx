import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import App from './App';

// Mock API
vi.mock('./api', () => ({
  API_BASE: 'http://localhost:8080',
  isDesktopApp: false,
  api: {
    listProjects: vi.fn().mockResolvedValue([]),
    getRecentFiles: vi.fn().mockResolvedValue([]),
    getInReview: vi.fn().mockResolvedValue([]),
    clearFocus: vi.fn().mockResolvedValue(undefined),
  },
}));

vi.mock('./hooks/useSSE', () => ({
  useSSE: vi.fn(),
}));

describe('App', () => {
  it('renders the layout with home view', async () => {
    render(<App />);
    expect(await screen.findByTestId('app-layout')).toBeInTheDocument();
  });
});
