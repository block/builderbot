import { useState } from 'react';
import { api } from '../api';
import type { InstallToolsStatus } from '../types';

interface Props {
  open: boolean;
  isUpdate: boolean;
  onClose: (installed: boolean) => void;
}

export default function InstallToolsModal({ open, isUpdate, onClose }: Props) {
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<InstallToolsStatus | null>(null);

  async function handleInstall() {
    setLoading(true);
    setResult(null);
    try {
      const status = await api.installTools();
      setResult(status);
    } catch {
      setResult({
        cli: { installed: false, error: 'Request failed' },
        plugin: { installed: false, error: 'Request failed' },
      });
    } finally {
      setLoading(false);
    }
  }

  const allInstalled = result?.cli.installed && result?.plugin.installed;

  function handleClose() {
    const didInstall = !!allInstalled;
    setResult(null);
    setLoading(false);
    onClose(didInstall);
  }

  return (
    <div className={`modal-overlay${open ? ' open' : ''}`} onClick={handleClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h3>{isUpdate ? 'Update' : 'Install'} Command Line Tools</h3>
        <p>
          {isUpdate
            ? <>Update the <strong>penpal</strong> CLI and Claude Code plugin to match this build.</>
            : <>Install the <strong>penpal</strong> CLI on your PATH and the Penpal plugin for Claude Code.</>}
        </p>

        {result && (
          <div className="install-results" data-testid="install-results">
            <div className={`install-result ${result.cli.installed ? 'success' : 'error'}`}>
              <span className="install-icon">{result.cli.installed ? '✓' : '✗'}</span>
              <span>
                CLI{' '}
                {result.cli.installed
                  ? `installed at ${result.cli.path}`
                  : result.cli.error}
              </span>
            </div>
            <div className={`install-result ${result.plugin.installed ? 'success' : 'error'}`}>
              <span className="install-icon">{result.plugin.installed ? '✓' : '✗'}</span>
              <span>
                Claude Code plugin{' '}
                {result.plugin.installed ? 'installed' : result.plugin.error}
              </span>
            </div>
          </div>
        )}

        <div className="modal-actions">
          <button className="btn-cancel" onClick={handleClose}>
            {allInstalled ? 'Done' : 'Not Now'}
          </button>
          {!allInstalled && (
            <button
              className="btn-primary"
              onClick={handleInstall}
              disabled={loading}
            >
              {loading ? 'Installing...' : isUpdate ? 'Update' : 'Install'}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
