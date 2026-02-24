import { useState, useRef, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';

interface FileMenuProps {
  project: string;
  projectPath: string;
  filePath: string;
  sourceType: string;
  rawMarkdown: string;
}

export default function FileMenu({
  project,
  projectPath,
  filePath,
  sourceType,
  rawMarkdown,
}: FileMenuProps) {
  const [open, setOpen] = useState(false);
  const [showDeleteModal, setShowDeleteModal] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [publishURL, setPublishURL] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const navigate = useNavigate();

  // Load publish state on mount
  useEffect(() => {
    api.getPublishState(project, filePath)
      .then((state) => {
        if (state.url) setPublishURL(state.url);
      })
      .catch(() => {});
  }, [project, filePath]);

  useEffect(() => {
    if (!open) return;
    const handleClick = () => setOpen(false);
    document.addEventListener('click', handleClick);
    return () => document.removeEventListener('click', handleClick);
  }, [open]);

  // Auto-dismiss toast
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 8000);
    return () => clearTimeout(timer);
  }, [toast]);

  const copyMarkdown = () => {
    navigator.clipboard.writeText(rawMarkdown);
    setOpen(false);
  };

  const copyFile = () => {
    setOpen(false);
    api.copyFile(project, filePath).catch((err) => {
      console.error('Copy file failed:', err);
    });
  };

  const copyRelativePath = () => {
    navigator.clipboard.writeText(`@${filePath}`);
    setOpen(false);
  };

  const copyAbsolutePath = () => {
    navigator.clipboard.writeText(`${projectPath}/${filePath}`);
    setOpen(false);
  };

  const handlePublish = async () => {
    setOpen(false);
    try {
      const result = await api.publish(project, filePath);
      if (result.url) {
        setPublishURL(result.url);
        setToast(result.url);
      }
    } catch (err) {
      alert('Publish failed: ' + (err instanceof Error ? err.message : err));
    }
  };

  const copyBlockcellLink = () => {
    if (publishURL) {
      navigator.clipboard.writeText(publishURL);
    }
    setOpen(false);
  };

  const handleDelete = async () => {
    setDeleting(true);
    try {
      await api.deleteFile(project, filePath);
      navigate(`/project/${project}`);
    } catch (err) {
      console.error('Delete failed:', err);
      setDeleting(false);
    }
  };

  const handleRemoveFromPenpal = async () => {
    setOpen(false);
    try {
      await api.removeSource(project, undefined, filePath);
      navigate(`/project/${project}`);
    } catch (err) {
      console.error('Remove failed:', err);
    }
  };

  return (
    <>
      <div className="file-menu-wrap" ref={wrapRef}>
        <button
          className="file-dots"
          onClick={(e) => {
            e.preventDefault();
            e.stopPropagation();
            setOpen(!open);
          }}
        >
          &#8942;
        </button>
        {open && (
          <div className="file-menu" style={{ display: 'block' }}>
            <button onClick={copyMarkdown}>Copy markdown</button>
            <button onClick={copyFile}>Copy file</button>
            <button onClick={copyRelativePath}>Copy relative path</button>
            <button onClick={copyAbsolutePath}>Copy absolute path</button>
            <div className="menu-divider" />
            <button onClick={handlePublish}>Publish to Blockcell</button>
            {publishURL && (
              <button onClick={copyBlockcellLink}>Copy Blockcell link</button>
            )}
            {sourceType === 'file' && (
              <>
                <div className="menu-divider" />
                <button onClick={handleRemoveFromPenpal}>Remove from Penpal</button>
              </>
            )}
            <div className="menu-divider" />
            <button
              className="menu-danger"
              onClick={() => {
                setOpen(false);
                setShowDeleteModal(true);
              }}
            >
              Delete from disk
            </button>
          </div>
        )}
      </div>

      {/* Publish toast */}
      {toast && (
        <div className="publish-toast">
          <strong>Published!</strong>{' '}
          <a href={toast} target="_blank" rel="noreferrer">
            {toast}
          </a>
        </div>
      )}

      {/* Delete confirmation modal */}
      {showDeleteModal && (
        <div
          className="modal-overlay open"
          style={{
            display: 'flex',
            position: 'fixed',
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
            background: 'var(--bg-modal-overlay)',
            zIndex: 200,
            justifyContent: 'center',
            alignItems: 'center',
          }}
          onClick={() => setShowDeleteModal(false)}
        >
          <div
            className="modal"
            style={{
              background: 'var(--bg-surface)',
              borderRadius: 8,
              padding: 24,
              maxWidth: 400,
              boxShadow: 'var(--shadow-modal)',
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <h3 style={{ marginBottom: 8 }}>Delete file?</h3>
            <p style={{ color: 'var(--text-muted)', fontSize: '0.9em', margin: 0 }}>
              This will permanently delete{' '}
              <strong>{filePath.split('/').pop()}</strong> from the filesystem. This
              cannot be undone.
            </p>
            <div
              style={{
                display: 'flex',
                gap: 8,
                justifyContent: 'flex-end',
                marginTop: 16,
              }}
            >
              <button
                className="btn-cancel-form"
                onClick={() => setShowDeleteModal(false)}
              >
                Cancel
              </button>
              <button
                className="btn-submit"
                style={{
                  background: 'var(--accent-danger)',
                  borderColor: 'var(--accent-danger)',
                }}
                disabled={deleting}
                onClick={handleDelete}
              >
                {deleting ? 'Deleting...' : 'Delete from disk'}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
