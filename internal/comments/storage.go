package comments

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
)

// commentsPath returns the absolute path to the sidecar JSON file for the
// given project and file path. The sidecar lives at:
//
//	{project.ThoughtsPath}/.birdseye/comments/{filePath}.json
func (s *Store) commentsPath(projectName, filePath string) (string, error) {
	project := s.cache.FindProject(projectName)
	if project == nil {
		return "", fmt.Errorf("project not found: %s", projectName)
	}
	return filepath.Join(project.ThoughtsPath, ".birdseye", "comments", filePath+".json"), nil
}

// Load reads and parses the sidecar JSON for the given project and file.
// If the file does not exist, it returns an empty FileComments (not an error).
func (s *Store) Load(projectName, filePath string) (*FileComments, error) {
	p, err := s.commentsPath(projectName, filePath)
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

// Save writes the FileComments to the sidecar JSON file atomically.
// It writes to a temporary file first, then renames it into place.
// Directories are created as needed.
func (s *Store) Save(projectName, filePath string, fc *FileComments) error {
	p, err := s.commentsPath(projectName, filePath)
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
