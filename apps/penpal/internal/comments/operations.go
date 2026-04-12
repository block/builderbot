package comments

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/loganj/penpal/internal/activity"
)

// CreateThread creates a new comment thread on the given file, anchored to
// the specified text selection. The first comment is added to the thread.
// IDs and timestamps are generated automatically.
// E-PENPAL-THREAD-MODEL: creates a Thread with open status, anchor, and initial comment.
func (s *Store) CreateThread(projectName, filePath string, anchor Anchor, comment Comment) (*Thread, error) {
	return s.CreateThreadForWorktree(projectName, filePath, "", anchor, comment)
}

// CreateThreadForWorktree creates a thread scoped to a specific worktree.
// E-PENPAL-THREAD-MUTEX: serializes thread creation via per-project sync.Mutex.
func (s *Store) CreateThreadForWorktree(projectName, filePath, worktree string, anchor Anchor, comment Comment) (*Thread, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	fc, err := s.LoadForWorktree(projectName, filePath, worktree)
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

	if err := s.SaveForWorktree(projectName, filePath, worktree, fc); err != nil {
		return nil, err
	}

	if s.activity != nil {
		s.activity.Record(activity.Comment, projectName, filePath)
	}
	return &thread, nil
}

// AddComment appends a comment to an existing thread. The comment ID and
// timestamp are generated automatically.
// E-PENPAL-INREPLYTO: sets InReplyTo to previous comment's ID.
func (s *Store) AddComment(projectName, filePath, threadID string, comment Comment) (*Thread, error) {
	return s.AddCommentForWorktree(projectName, filePath, "", threadID, comment)
}

// AddCommentForWorktree appends a comment scoped to a specific worktree.
// For agent-role comments, automatically sets InReplyTo and WorkingStartedAt
// from the stored working entry, then clears the working indicator after writing.
// This consolidates working indicator logic so MCP tools and REST handlers
// don't need to manage it themselves.
// E-PENPAL-THREAD-MUTEX: serializes comment addition via per-project sync.Mutex.
// E-PENPAL-WORKING: auto-handles working indicators for agent replies.
func (s *Store) AddCommentForWorktree(projectName, filePath, worktree, threadID string, comment Comment) (*Thread, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	// For agent replies, populate InReplyTo and WorkingStartedAt from stored
	// working entry. Reads are inside mu.Lock() to prevent two concurrent
	// agent replies from reading the same working state.
	if comment.Role == "agent" {
		if comment.InReplyTo == "" {
			if afterID := s.WorkingAfterCommentID(projectName, filePath, threadID); afterID != "" {
				comment.InReplyTo = afterID
			}
		}
		if comment.WorkingStartedAt == nil {
			if startedAt := s.WorkingStartedAt(projectName, filePath, threadID); !startedAt.IsZero() {
				comment.WorkingStartedAt = &startedAt
			}
		}
	}

	fc, err := s.LoadForWorktree(projectName, filePath, worktree)
	if err != nil {
		return nil, err
	}

	for i := range fc.Threads {
		if fc.Threads[i].ID == threadID {
			comment.ID = generateID()
			comment.CreatedAt = time.Now()
			if comment.InReplyTo == "" && len(fc.Threads[i].Comments) > 0 {
				comment.InReplyTo = fc.Threads[i].Comments[len(fc.Threads[i].Comments)-1].ID
			}
			fc.Threads[i].Comments = append(fc.Threads[i].Comments, comment)

			if err := s.SaveForWorktree(projectName, filePath, worktree, fc); err != nil {
				return nil, err
			}
			if s.activity != nil {
				s.activity.Record(activity.Comment, projectName, filePath)
			}
			t := fc.Threads[i]

			// Clear working AFTER writing so SSE broadcasts trigger a
			// fetchThreads that reads the updated file.
			if comment.Role == "agent" {
				s.ClearWorking(projectName, filePath, threadID)
			}

			return &t, nil
		}
	}

	return nil, fmt.Errorf("thread not found: %s", threadID)
}

// markThreadWorking updates the working indicator for a single thread if it is
// open and the last comment is from a human. If a working entry already exists
// for the same comment, only the expiry timer is refreshed.
// E-PENPAL-WORKING: shared helper for MarkThreadsRead and MarkFileThreadsRead.
func (s *Store) markThreadWorking(project, filePath string, t Thread) {
	if t.Status != "open" || len(t.Comments) == 0 || t.Comments[len(t.Comments)-1].Role != "human" {
		return
	}
	lastCommentID := t.Comments[len(t.Comments)-1].ID
	if s.WorkingAfterCommentID(project, filePath, t.ID) == lastCommentID {
		s.RefreshWorkingTimestamp(project, filePath, t.ID)
	} else {
		s.SetWorking(project, filePath, t.ID, lastCommentID)
	}
}

// MarkThreadsRead sets working indicators for all open threads where the last
// comment is from a human. Call this when an agent reads thread listings so
// the UI shows the "working" pulsing dot.
// E-PENPAL-WORKING: consolidates set-working-on-read logic from MCP and REST layers.
func (s *Store) MarkThreadsRead(project string, threads []ThreadWithFile) {
	for _, t := range threads {
		s.markThreadWorking(project, t.FilePath, t.Thread)
	}
}

// MarkFileThreadsRead sets working indicators for open threads on a specific file
// where the last comment is from a human.
// E-PENPAL-WORKING: consolidates set-working-on-read logic for single-file operations.
func (s *Store) MarkFileThreadsRead(project, filePath string, threads []Thread) {
	for _, t := range threads {
		s.markThreadWorking(project, filePath, t)
	}
}

// ResolveThread marks a thread as resolved.
// E-PENPAL-THREAD-MODEL: transitions thread status to "resolved" with ResolvedAt/ResolvedBy.
func (s *Store) ResolveThread(projectName, filePath, threadID, resolvedBy string) error {
	return s.ResolveThreadForWorktree(projectName, filePath, "", threadID, resolvedBy)
}

// ResolveThreadForWorktree marks a thread as resolved, scoped to a worktree.
func (s *Store) ResolveThreadForWorktree(projectName, filePath, worktree, threadID, resolvedBy string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	fc, err := s.LoadForWorktree(projectName, filePath, worktree)
	if err != nil {
		return err
	}

	for i := range fc.Threads {
		if fc.Threads[i].ID == threadID {
			fc.Threads[i].Status = "resolved"
			fc.Threads[i].ResolvedAt = time.Now()
			fc.Threads[i].ResolvedBy = resolvedBy
			if err := s.SaveForWorktree(projectName, filePath, worktree, fc); err != nil {
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
// E-PENPAL-THREAD-MODEL: transitions thread status back to "open", clears ResolvedAt/ResolvedBy.
func (s *Store) ReopenThread(projectName, filePath, threadID string) error {
	return s.ReopenThreadForWorktree(projectName, filePath, "", threadID)
}

// ReopenThreadForWorktree sets a resolved thread back to open, scoped to a worktree.
func (s *Store) ReopenThreadForWorktree(projectName, filePath, worktree, threadID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	fc, err := s.LoadForWorktree(projectName, filePath, worktree)
	if err != nil {
		return err
	}

	for i := range fc.Threads {
		if fc.Threads[i].ID == threadID {
			fc.Threads[i].Status = "open"
			fc.Threads[i].ResolvedAt = time.Time{}
			fc.Threads[i].ResolvedBy = ""
			return s.SaveForWorktree(projectName, filePath, worktree, fc)
		}
	}

	return fmt.Errorf("thread not found: %s", threadID)
}

// ListOpenThreads walks the .penpal/comments/ directory for the given
// project and returns all threads with status "open" across all files.
// Returned file paths are relative to the project root (e.g., "thoughts/shared/plans/foo.md").
// E-PENPAL-COMMENT-STORAGE: reads JSON sidecar files from {project}/.penpal/comments/.
func (s *Store) ListOpenThreads(projectName string) ([]ThreadWithFile, error) {
	return s.ListThreadsByStatus(projectName, "open")
}

// ListThreadsByStatus walks the .penpal/comments/ directory for the given
// project and returns threads matching the given status filter.
// An empty status returns all threads regardless of status.
func (s *Store) ListThreadsByStatus(projectName, status string) ([]ThreadWithFile, error) {
	return s.ListThreadsByStatusForWorktree(projectName, status, "")
}

// ListThreadsByStatusForWorktree walks the comments directory scoped to a worktree.
// E-PENPAL-COMMENT-WORKTREE: reads threads from worktree-scoped comment storage.
func (s *Store) ListThreadsByStatusForWorktree(projectName, status, worktree string) ([]ThreadWithFile, error) {
	project := s.cache.FindProject(projectName)
	if project == nil {
		return nil, fmt.Errorf("project not found: %s", projectName)
	}

	basePath := project.Path
	if worktree != "" {
		wtPath := s.cache.WorktreePath(projectName, worktree)
		if wtPath == "" {
			return nil, fmt.Errorf("worktree not found: %s", worktree)
		}
		basePath = wtPath
	}

	commentsDir := filepath.Join(basePath, ".penpal", "comments")
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
			if status == "" || t.Status == status {
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

// WaitResult holds the outcome of a wait-for-changes operation.
// E-PENPAL-CHANGE-SEQ: result of WaitAndEnrich combining wait + enrich + refresh.
type WaitResult struct {
	Changed bool             `json:"changed"`
	Seq     uint64           `json:"seq"`
	Files   []WaitResultFile `json:"files"`
}

// WaitResultFile describes a file in review with optional pending thread detail.
type WaitResultFile struct {
	FilePath    string   `json:"filePath"`
	OpenThreads int      `json:"openThreads"`
	Threads     []Thread `json:"threads,omitempty"`
}

// WaitAndEnrich blocks until comments change (or context cancels), then returns
// enriched files with pending threads and refreshed working indicators.
// This consolidates the duplicated wait-enrich-refresh logic from the MCP
// penpal_wait_for_changes tool and the REST handleAgentWait handler.
// E-PENPAL-CHANGE-SEQ: uses WaitForChangeSince to block until changeSeq advances.
// E-PENPAL-MCP-WORKING: refreshes working timestamps during wait cycles to prevent expiry.
func (s *Store) WaitAndEnrich(ctx context.Context, project, worktree string, sinceSeq uint64) (*WaitResult, error) {
	seq, waitErr := s.WaitForChangeSince(ctx, sinceSeq)
	changed := waitErr == nil

	files, err := s.ListFilesInReviewForWorktree(project, worktree)
	if err != nil {
		return nil, err
	}

	// Single loop: refresh working indicators for all files, and collect
	// pending threads when changes were detected.
	var pendingByFile map[string][]Thread
	if changed {
		pendingByFile = make(map[string][]Thread)
	}
	for _, f := range files {
		fc, loadErr := s.LoadForWorktree(project, f.FilePath, worktree)
		if loadErr != nil {
			continue
		}
		s.MarkFileThreadsRead(project, f.FilePath, fc.Threads)
		if changed {
			for _, t := range fc.Threads {
				if t.Status == "open" && len(t.Comments) > 0 && t.Comments[len(t.Comments)-1].Role == "human" {
					pendingByFile[f.FilePath] = append(pendingByFile[f.FilePath], t)
				}
			}
		}
	}

	resultFiles := make([]WaitResultFile, len(files))
	for i, f := range files {
		resultFiles[i] = WaitResultFile{
			FilePath:    f.FilePath,
			OpenThreads: f.OpenThreads,
		}
		if changed {
			resultFiles[i].Threads = pendingByFile[f.FilePath]
		}
	}

	return &WaitResult{Changed: changed, Seq: seq, Files: resultFiles}, nil
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

// ListFilesInReview walks the .penpal/comments/ directory for the given
// project and returns all files that have at least one open comment thread.
// Returned file paths are relative to the project root (e.g., "thoughts/shared/plans/foo.md").
// E-PENPAL-COMMENT-STORAGE: scans JSON sidecar files for open threads.
func (s *Store) ListFilesInReview(projectName string) ([]FileInReview, error) {
	return s.ListFilesInReviewForWorktree(projectName, "")
}

// ListFilesInReviewForWorktree lists files in review scoped to a worktree.
// E-PENPAL-COMMENT-WORKTREE: scans worktree-scoped sidecar files for open threads.
func (s *Store) ListFilesInReviewForWorktree(projectName, worktree string) ([]FileInReview, error) {
	project := s.cache.FindProject(projectName)
	if project == nil {
		return nil, fmt.Errorf("project not found: %s", projectName)
	}

	basePath := project.Path
	if worktree != "" {
		wtPath := s.cache.WorktreePath(projectName, worktree)
		if wtPath == "" {
			return nil, fmt.Errorf("worktree not found: %s", worktree)
		}
		basePath = wtPath
	}

	commentsDir := filepath.Join(basePath, ".penpal", "comments")
	// For source file existence checks, use the worktree path if available
	sourceBasePath := basePath
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

		// Skip sidecars whose source file no longer exists on disk.
		sourceFile := filepath.Join(sourceBasePath, filePath)
		if _, err := os.Stat(sourceFile); os.IsNotExist(err) {
			return nil
		}

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
