import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import InstallToolsModal from './InstallToolsModal';
import { api } from '../api';

vi.mock('../api', () => ({
  API_BASE: 'http://localhost:8080',
  isDesktopApp: false,
  api: {
    installTools: vi.fn(),
    checkInstallStatus: vi.fn(),
    listProjects: vi.fn().mockResolvedValue([]),
    getInReview: vi.fn().mockResolvedValue([]),
  },
}));

beforeEach(() => {
  vi.clearAllMocks();
});

describe('InstallToolsModal', () => {
  it('renders with Install button for fresh install', () => {
    render(<InstallToolsModal open={true} isUpdate={false} onClose={() => {}} />);

    expect(screen.getByText('Install Command Line Tools')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Install' })).toBeInTheDocument();
    expect(screen.getByText('Not Now')).toBeInTheDocument();
    expect(screen.getByText(/Install the/)).toBeInTheDocument();
  });

  it('renders with Update button when tools already exist', () => {
    render(<InstallToolsModal open={true} isUpdate={true} onClose={() => {}} />);

    expect(screen.getByText('Update Command Line Tools')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Update' })).toBeInTheDocument();
    expect(screen.getByText(/Update the/)).toBeInTheDocument();
  });

  it('does not render content when closed', () => {
    render(<InstallToolsModal open={false} isUpdate={false} onClose={() => {}} />);

    const overlay = document.querySelector('.modal-overlay');
    expect(overlay).not.toHaveClass('open');
  });

  it('calls onClose(false) when dismiss button is clicked without installing', () => {
    const onClose = vi.fn();
    render(<InstallToolsModal open={true} isUpdate={false} onClose={onClose} />);

    fireEvent.click(screen.getByText('Not Now'));
    expect(onClose).toHaveBeenCalledWith(false);
  });

  it('calls installTools API when install is clicked', async () => {
    vi.mocked(api.installTools).mockResolvedValue({
      cli: { installed: true, path: '/usr/local/bin/penpal' },
      plugin: { installed: true },
    });

    render(<InstallToolsModal open={true} isUpdate={false} onClose={() => {}} />);
    fireEvent.click(screen.getByText('Install'));

    await waitFor(() => {
      expect(api.installTools).toHaveBeenCalledTimes(1);
    });
  });

  it('shows success state and calls onClose(true) when done', async () => {
    vi.mocked(api.installTools).mockResolvedValue({
      cli: { installed: true, path: '/usr/local/bin/penpal' },
      plugin: { installed: true },
    });

    const onClose = vi.fn();
    render(<InstallToolsModal open={true} isUpdate={false} onClose={onClose} />);
    fireEvent.click(screen.getByText('Install'));

    await waitFor(() => {
      expect(screen.getByTestId('install-results')).toBeInTheDocument();
    });

    expect(screen.getByText(/installed at/)).toBeInTheDocument();
    expect(screen.getByText('Done')).toBeInTheDocument();
    // Install button should be gone after success
    expect(screen.queryByText('Install')).not.toBeInTheDocument();

    fireEvent.click(screen.getByText('Done'));
    expect(onClose).toHaveBeenCalledWith(true);
  });

  it('shows error state on failure', async () => {
    vi.mocked(api.installTools).mockResolvedValue({
      cli: { installed: false, error: 'permission denied' },
      plugin: { installed: true },
    });

    render(<InstallToolsModal open={true} isUpdate={false} onClose={() => {}} />);
    fireEvent.click(screen.getByText('Install'));

    await waitFor(() => {
      expect(screen.getByTestId('install-results')).toBeInTheDocument();
    });

    expect(screen.getByText(/permission denied/)).toBeInTheDocument();
  });
});
