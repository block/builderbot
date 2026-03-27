import { useState } from 'react';
import { api } from '../api';
import type { InstallToolsStatus } from '../types';

interface Props {
  open: boolean;
  isUpdate: boolean;
  onClose: (installed: boolean) => void;
}

// E-PENPAL-INSTALL-DISMISS: install modal with dismiss keyed to BUILD_ID.
export default function InstallToolsModal({ open, isUpdate, onClose }: Props) {
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<InstallToolsStatus | null>(null);
  const [claudePathInput, setClaudePathInput] = useState('');
  const [claudePathError, setClaudePathError] = useState('');
  const [settingPath, setSettingPath] = useState(false);

  const needsClaudePath = result && !result.claudeBin && !result.plugin.installed;

  async function handleInstall() {
    setLoading(true);
    setResult(null);
    setClaudePathInput('');
    setClaudePathError('');
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

  async function handleSetClaudePath() {
    const path = claudePathInput.trim();
    if (!path) return;
    setSettingPath(true);
    setClaudePathError('');
    try {
      await api.setClaudePath(path);
      // Path accepted — retry the full install
      await handleInstall();
    } catch {
      setClaudePathError('Not a valid claude executable');
    } finally {
      setSettingPath(false);
    }
  }

  const allInstalled = result?.cli.installed && result?.plugin.installed;

  function handleClose() {
    const didInstall = !!allInstalled;
    setResult(null);
    setLoading(false);
    setClaudePathInput('');
    setClaudePathError('');
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

        {needsClaudePath && (
          <div className="claude-path-prompt" data-testid="claude-path-prompt">
            <p style={{ fontSize: '13px', margin: '12px 0 8px' }}>
              Could not find the <code>claude</code> binary. Enter the full path:
            </p>
            <div style={{ display: 'flex', gap: '8px' }}>
              <input
                type="text"
                value={claudePathInput}
                onChange={(e) => setClaudePathInput(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && handleSetClaudePath()}
                placeholder="/path/to/claude"
                style={{
                  flex: 1,
                  padding: '6px 10px',
                  borderRadius: '6px',
                  border: '1px solid var(--border)',
                  background: 'var(--bg-secondary)',
                  color: 'var(--text-primary)',
                  fontSize: '13px',
                  fontFamily: 'monospace',
                }}
                disabled={settingPath}
              />
              <button
                className="btn-primary"
                onClick={handleSetClaudePath}
                disabled={settingPath || !claudePathInput.trim()}
                style={{ whiteSpace: 'nowrap' }}
              >
                {settingPath ? 'Checking...' : 'Set Path'}
              </button>
            </div>
            {claudePathError && (
              <p style={{ color: 'var(--text-error, #e53e3e)', fontSize: '12px', margin: '4px 0 0' }}>
                {claudePathError}
              </p>
            )}
          </div>
        )}

        <div className="modal-actions">
          <button className="btn-cancel" onClick={handleClose}>
            {allInstalled ? 'Done' : 'Not Now'}
          </button>
          {!allInstalled && !needsClaudePath && (
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
