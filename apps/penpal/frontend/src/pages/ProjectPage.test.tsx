import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { describe, it, expect } from 'vitest';
import ProjectPage from './ProjectPage';

function renderPage(path = '/project/ws/proj') {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/project/*" element={<ProjectPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

// E-PENPAL-PROJECT-WELCOME: verifies project welcome screen.
describe('ProjectPage', () => {
  it('has project-page testid', () => {
    renderPage();
    expect(screen.getByTestId('project-page')).toBeTruthy();
  });

  it('shows project name extracted from URL', () => {
    renderPage();
    expect(screen.getByText('proj')).toBeTruthy();
  });

  it('shows sidebar guidance text', () => {
    renderPage();
    expect(screen.getByText('Expand a source in the sidebar to browse files.')).toBeTruthy();
  });
});
