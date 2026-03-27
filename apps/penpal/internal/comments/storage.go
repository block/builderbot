package comments

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// commentsPath returns the absolute path to the sidecar JSON file for the
// given project and file path. The sidecar lives at:
//
//	{project.Path}/.penpal/comments/{filePath}.json
//
// filePath is relative to the project root (e.g., "thoughts/shared/plans/foo.md").
//
// E-PENPAL-COMMENT-STORAGE: JSON sidecar files at {project}/.penpal/comments/{path}.json.
func (s *Store) commentsPath(projectName, filePath string) (string, error) {
	return s.commentsPathForWorktree(projectName, filePath, "")
}

// commentsPathForWorktree returns the absolute path to the sidecar JSON file,
// scoped to a specific worktree. When worktree is empty, uses the main project path.
// When worktree is specified, uses the worktree's filesystem path.
//
// E-PENPAL-COMMENT-WORKTREE: stores comments within the worktree's filesystem path.
func (s *Store) commentsPathForWorktree(projectName, filePath, worktree string) (string, error) {
	project := s.cache.FindProject(projectName)
	if project == nil {
		return "", fmt.Errorf("project not found: %s", projectName)
	}

	basePath := project.Path
	if worktree != "" {
		wtPath := s.cache.WorktreePath(projectName, worktree)
		if wtPath == "" {
			return "", fmt.Errorf("worktree not found: %s", worktree)
		}
		basePath = wtPath
	}

	commentsDir := filepath.Join(basePath, ".penpal", "comments")
	full := filepath.Join(commentsDir, filePath+".json")

	// Prevent path traversal: resolved path must stay within the comments dir.
	resolved, err := filepath.Abs(full)
	if err != nil {
		return "", fmt.Errorf("invalid path: %w", err)
	}
	absDir, _ := filepath.Abs(commentsDir)
	if !strings.HasPrefix(filepath.Clean(resolved)+string(filepath.Separator),
		filepath.Clean(absDir)+string(filepath.Separator)) {
		return "", fmt.Errorf("path traversal detected: %s", filePath)
	}

	return resolved, nil
}

// Load reads and parses the sidecar JSON for the given project and file.
// If the file does not exist, it returns an empty FileComments (not an error).
// E-PENPAL-COMMENT-STORAGE: reads JSON sidecar file for a project file.
func (s *Store) Load(projectName, filePath string) (*FileComments, error) {
	return s.LoadForWorktree(projectName, filePath, "")
}

// LoadForWorktree reads and parses the sidecar JSON scoped to a worktree.
func (s *Store) LoadForWorktree(projectName, filePath, worktree string) (*FileComments, error) {
	p, err := s.commentsPathForWorktree(projectName, filePath, worktree)
	if err != nil {
		return nil, err
	}

	data, err := os.ReadFile(p)
	if err != nil {
		if os.IsNotExist(err) {
			return &FileComments{}, nil
		}
		return nil, fmt.Errorf("reading comments file: %w", err)
	}

	var fc FileComments
	if err := json.Unmarshal(data, &fc); err != nil {
		return nil, fmt.Errorf("parsing comments file: %w", err)
	}
	return &fc, nil
}

// migrateInReplyTo backfills missing InReplyTo fields by walking each
// thread's comments in array order. comment[0] stays root; comment[N]
// gets InReplyTo = comment[N-1].ID if not already set.
//
// E-PENPAL-INREPLYTO: backfills legacy data missing InReplyTo fields.
func migrateInReplyTo(fc *FileComments) {
	for i, t := range fc.Threads {
		for j := 1; j < len(t.Comments); j++ {
			if fc.Threads[i].Comments[j].InReplyTo == "" {
				fc.Threads[i].Comments[j].InReplyTo = t.Comments[j-1].ID
			}
		}
	}
}

// Save writes the FileComments to the sidecar JSON file atomically.
// It writes to a temporary file first, then renames it into place.
// Directories are created as needed.
// E-PENPAL-COMMENT-STORAGE: atomic writes via temp file + rename.
func (s *Store) Save(projectName, filePath string, fc *FileComments) error {
	return s.SaveForWorktree(projectName, filePath, "", fc)
}

// SaveForWorktree writes comments scoped to a worktree.
// E-PENPAL-COMMENT-WORKTREE: atomic write to worktree-scoped sidecar path.
func (s *Store) SaveForWorktree(projectName, filePath, worktree string, fc *FileComments) error {
	migrateInReplyTo(fc)
	p, err := s.commentsPathForWorktree(projectName, filePath, worktree)
	if err != nil {
		return err
	}

	dir := filepath.Dir(p)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("creating comments directory: %w", err)
	}

	data, err := json.MarshalIndent(fc, "", "  ")
	if err != nil {
		return fmt.Errorf("marshaling comments: %w", err)
	}

	tmp := p + ".tmp"
	if err := os.WriteFile(tmp, data, 0644); err != nil {
		return fmt.Errorf("writing temp file: %w", err)
	}

	if err := os.Rename(tmp, p); err != nil {
		os.Remove(tmp) // best-effort cleanup
		return fmt.Errorf("renaming temp file: %w", err)
	}

	s.NotifyChange()
	return nil
}

// LoadThreads is a convenience method that returns just the threads for a file.
func (s *Store) LoadThreads(projectName, filePath string) ([]Thread, error) {
	fc, err := s.Load(projectName, filePath)
	if err != nil {
		return nil, err
	}
	return fc.Threads, nil
}

// SaveThreads is a convenience method that replaces the threads for a file,
// preserving any existing review state.
func (s *Store) SaveThreads(projectName, filePath string, threads []Thread) error {
	fc, err := s.Load(projectName, filePath)
	if err != nil {
		return err
	}
	fc.Threads = threads
	return s.Save(projectName, filePath, fc)
}
