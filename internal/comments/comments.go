package comments

import (
	"context"
	"strings"
	"sync"
	"time"

	"github.com/loganj/birdseye/internal/cache"
)

// Store manages comment threads and reviews for project files.
// It uses sidecar JSON files stored alongside the thoughts directory.
type Store struct {
	cache      *cache.Cache
	mu         sync.Mutex           // serializes file writes per-project
	heartbeats map[string]time.Time // key: "project:filePath" -> last agent poll time
	heartMu    sync.RWMutex
	changed    chan struct{} // closed on every Save, then replaced
	changedMu  sync.Mutex
	changeSeq  uint64 // monotonic counter incremented on each change
}

// FileComments holds all comment threads for a single file.
type FileComments struct {
	Threads []Thread `json:"threads"`
}

// Thread represents a comment thread anchored to a specific piece of text.
type Thread struct {
	ID         string    `json:"id"`
	Status     string    `json:"status"` // "open" | "resolved"
	Anchor     Anchor    `json:"anchor"`
	Comments   []Comment `json:"comments"`
	CreatedAt  time.Time `json:"createdAt"`
	ResolvedAt time.Time `json:"resolvedAt,omitempty"`
	ResolvedBy string    `json:"resolvedBy,omitempty"`
}

// Anchor describes the text selection a thread is attached to.
type Anchor struct {
	SelectedText string `json:"selectedText"`
	Before       string `json:"before,omitempty"`
	After        string `json:"after,omitempty"`
	HeadingPath  string `json:"headingPath,omitempty"`
}

// Comment is a single message within a thread.
type Comment struct {
	ID        string    `json:"id"`
	Author    string    `json:"author"`
	Role      string    `json:"role"` // "human" | "agent"
	Body      string    `json:"body"`
	CreatedAt time.Time `json:"createdAt"`
}

// ThreadWithFile pairs a thread with the file path it belongs to.
type ThreadWithFile struct {
	Thread
	FilePath string `json:"filePath"`
}

// FileInReview describes a file with open comment threads.
type FileInReview struct {
	FilePath    string `json:"filePath"`
	OpenThreads int    `json:"openThreads"`
}

// NewStore creates a new comment Store backed by the given cache.
func NewStore(c *cache.Cache) *Store {
	return &Store{
		cache:      c,
		heartbeats: make(map[string]time.Time),
		changed:    make(chan struct{}),
	}
}

// NotifyChange wakes all goroutines blocked in WaitForChange.
func (s *Store) NotifyChange() {
	s.changedMu.Lock()
	defer s.changedMu.Unlock()
	s.changeSeq++
	close(s.changed)
	s.changed = make(chan struct{})
}

// ChangeSeq returns the current change sequence number.
func (s *Store) ChangeSeq() uint64 {
	s.changedMu.Lock()
	defer s.changedMu.Unlock()
	return s.changeSeq
}

// WaitForChangeSince blocks until the change sequence advances past sinceSeq,
// or until the context is cancelled. Returns the current sequence number.
// If changes already occurred since sinceSeq, returns immediately.
func (s *Store) WaitForChangeSince(ctx context.Context, sinceSeq uint64) (uint64, error) {
	s.changedMu.Lock()
	if s.changeSeq > sinceSeq {
		seq := s.changeSeq
		s.changedMu.Unlock()
		return seq, nil
	}
	ch := s.changed
	s.changedMu.Unlock()

	select {
	case <-ch:
		s.changedMu.Lock()
		seq := s.changeSeq
		s.changedMu.Unlock()
		return seq, nil
	case <-ctx.Done():
		s.changedMu.Lock()
		seq := s.changeSeq
		s.changedMu.Unlock()
		return seq, ctx.Err()
	}
}

// RecordHeartbeat records the current time as the last agent poll for the
// given project and file path.
func (s *Store) RecordHeartbeat(projectName, filePath string) {
	s.heartMu.Lock()
	defer s.heartMu.Unlock()
	if s.heartbeats == nil {
		s.heartbeats = make(map[string]time.Time)
	}
	s.heartbeats[projectName+":"+filePath] = time.Now()
}

// IsAgentActive returns true if an agent has polled for the given file
// within the last 60 seconds.
func (s *Store) IsAgentActive(projectName, filePath string) bool {
	s.heartMu.RLock()
	defer s.heartMu.RUnlock()
	if s.heartbeats == nil {
		return false
	}
	t, ok := s.heartbeats[projectName+":"+filePath]
	if !ok {
		return false
	}
	return time.Since(t) < 60*time.Second
}

// IsProjectActive returns true if any agent has polled for any file (or the
// project itself) within the last 60 seconds.
func (s *Store) IsProjectActive(projectName string) bool {
	s.heartMu.RLock()
	defer s.heartMu.RUnlock()
	prefix := projectName + ":"
	for key, t := range s.heartbeats {
		if strings.HasPrefix(key, prefix) && time.Since(t) < 60*time.Second {
			return true
		}
	}
	return false
}
