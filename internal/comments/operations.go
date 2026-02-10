package comments

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/loganj/birdseye/internal/activity"
)

// CreateThread creates a new comment thread on the given file, anchored to
// the specified text selection. The first comment is added to the thread.
// IDs and timestamps are generated automatically.
func (s *Store) CreateThread(projectName, filePath string, anchor Anchor, comment Comment) (*Thread, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	fc, err := s.Load(projectName, filePath)
	if err != nil {
		return nil, err
	}

	now := time.Now()
	comment.ID = generateID()
	comment.CreatedAt = now

	thread := Thread{
		ID:        generateID(),
		Status:    "open",
		Anchor:    anchor,
		Comments:  []Comment{comment},
		CreatedAt: now,
	}

	fc.Threads = append(fc.Threads, thread)

	if err := s.Save(projectName, filePath, fc); err != nil {
		return nil, err
	}

	if s.activity != nil {
		s.activity.Record(activity.Comment, projectName, filePath)
	}
	return &thread, nil
}

// AddComment appends a comment to an existing thread. The comment ID and
// timestamp are generated automatically.
func (s *Store) AddComment(projectName, filePath, threadID string, comment Comment) (*Thread, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	fc, err := s.Load(projectName, filePath)
	if err != nil {
		return nil, err
	}

	for i := range fc.Threads {
		if fc.Threads[i].ID == threadID {
			comment.ID = generateID()
			comment.CreatedAt = time.Now()
			fc.Threads[i].Comments = append(fc.Threads[i].Comments, comment)

			if err := s.Save(projectName, filePath, fc); err != nil {
				return nil, err
			}
			if s.activity != nil {
				s.activity.Record(activity.Comment, projectName, filePath)
			}
			t := fc.Threads[i]
			return &t, nil
		}
	}

	return nil, fmt.Errorf("thread not found: %s", threadID)
}

// ResolveThread marks a thread as resolved.
func (s *Store) ResolveThread(projectName, filePath, threadID, resolvedBy string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	fc, err := s.Load(projectName, filePath)
	if err != nil {
		return err
	}

	for i := range fc.Threads {
		if fc.Threads[i].ID == threadID {
			fc.Threads[i].Status = "resolved"
			fc.Threads[i].ResolvedAt = time.Now()
			fc.Threads[i].ResolvedBy = resolvedBy
			if err := s.Save(projectName, filePath, fc); err != nil {
				return err
			}
			if s.activity != nil {
				s.activity.Record(activity.Comment, projectName, filePath)
			}
			return nil
		}
	}

	return fmt.Errorf("thread not found: %s", threadID)
}

// ReopenThread sets a resolved thread back to open.
func (s *Store) ReopenThread(projectName, filePath, threadID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	fc, err := s.Load(projectName, filePath)
	if err != nil {
		return err
	}

	for i := range fc.Threads {
		if fc.Threads[i].ID == threadID {
			fc.Threads[i].Status = "open"
			fc.Threads[i].ResolvedAt = time.Time{}
			fc.Threads[i].ResolvedBy = ""
			return s.Save(projectName, filePath, fc)
		}
	}

	return fmt.Errorf("thread not found: %s", threadID)
}

// ListOpenThreads walks the .birdseye/comments/ directory for the given
// project and returns all threads with status "open" across all files.
// Returned file paths are relative to the project root (e.g., "thoughts/shared/plans/foo.md").
func (s *Store) ListOpenThreads(projectName string) ([]ThreadWithFile, error) {
	project := s.cache.FindProject(projectName)
	if project == nil {
		return nil, fmt.Errorf("project not found: %s", projectName)
	}

	commentsDir := filepath.Join(project.Path, ".birdseye", "comments")
	var results []ThreadWithFile

	err := filepath.Walk(commentsDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			if os.IsNotExist(err) {
				return nil
			}
			return err
		}
		if info.IsDir() || !strings.HasSuffix(path, ".json") {
			return nil
		}

		data, err := os.ReadFile(path)
		if err != nil {
			return nil
		}

		var fc FileComments
		if err := json.Unmarshal(data, &fc); err != nil {
			return nil
		}

		// Derive the original file path from the sidecar path
		rel, err := filepath.Rel(commentsDir, path)
		if err != nil {
			return nil
		}
		filePath := strings.TrimSuffix(rel, ".json")

		for _, t := range fc.Threads {
			if t.Status == "open" {
				results = append(results, ThreadWithFile{
					Thread:   t,
					FilePath: filePath,
				})
			}
		}
		return nil
	})

	if err != nil && !os.IsNotExist(err) {
		return nil, err
	}

	return results, nil
}

// HasPendingHumanComments returns true if any open thread in the project
// has a human as the last commenter (i.e., awaiting agent response).
func (s *Store) HasPendingHumanComments(projectName string) bool {
	threads, err := s.ListOpenThreads(projectName)
	if err != nil || len(threads) == 0 {
		return false
	}
	for _, t := range threads {
		if len(t.Comments) > 0 && t.Comments[len(t.Comments)-1].Role == "human" {
			return true
		}
	}
	return false
}

// ListFilesInReview walks the .birdseye/comments/ directory for the given
// project and returns all files that have at least one open comment thread.
// Returned file paths are relative to the project root (e.g., "thoughts/shared/plans/foo.md").
func (s *Store) ListFilesInReview(projectName string) ([]FileInReview, error) {
	project := s.cache.FindProject(projectName)
	if project == nil {
		return nil, fmt.Errorf("project not found: %s", projectName)
	}

	commentsDir := filepath.Join(project.Path, ".birdseye", "comments")
	var results []FileInReview

	err := filepath.Walk(commentsDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			if os.IsNotExist(err) {
				return nil
			}
			return err
		}
		if info.IsDir() || !strings.HasSuffix(path, ".json") {
			return nil
		}

		data, err := os.ReadFile(path)
		if err != nil {
			return nil
		}

		var fc FileComments
		if err := json.Unmarshal(data, &fc); err != nil {
			return nil
		}

		rel, err := filepath.Rel(commentsDir, path)
		if err != nil {
			return nil
		}
		filePath := strings.TrimSuffix(rel, ".json")

		openCount := 0
		for _, t := range fc.Threads {
			if t.Status == "open" {
				openCount++
			}
		}

		if openCount == 0 {
			return nil
		}

		results = append(results, FileInReview{
			FilePath:    filePath,
			OpenThreads: openCount,
		})
		return nil
	})

	if err != nil && !os.IsNotExist(err) {
		return nil, err
	}

	return results, nil
}
