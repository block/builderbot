package comments

import (
	"context"
	"strings"
	"sync"
	"time"

	"github.com/loganj/birdseye/internal/activity"
	"github.com/loganj/birdseye/internal/cache"
)

// Store manages comment threads and reviews for project files.
// It uses sidecar JSON files stored alongside the thoughts directory.
type Store struct {
	cache      *cache.Cache
	activity   *activity.Tracker
	mu         sync.Mutex           // serializes file writes per-project
	heartbeats map[string]time.Time // key: "project:filePath" -> last agent poll time
	heartMu    sync.RWMutex
	changed    chan struct{} // closed on every Save, then replaced
	changedMu  sync.Mutex
	changeSeq  uint64 // monotonic counter incremented on each change
	typingMu   sync.RWMutex
	typing     map[string]time.Time // key: "project:path:threadId" -> when typing started
	onTyping   func(project string) // called when typing state changes
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
	StartLine    int    `json:"startLine,omitempty"`
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
func NewStore(c *cache.Cache, act *activity.Tracker) *Store {
	return &Store{
		cache:      c,
		activity:   act,
		heartbeats: make(map[string]time.Time),
		changed:    make(chan struct{}),
		typing:     make(map[string]time.Time),
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

// ClearProjectHeartbeats removes all heartbeat entries for a project.
func (s *Store) ClearProjectHeartbeats(projectName string) {
	prefix := projectName + ":"
	s.heartMu.Lock()
	defer s.heartMu.Unlock()
	for key := range s.heartbeats {
		if strings.HasPrefix(key, prefix) {
			delete(s.heartbeats, key)
		}
	}
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

// SetOnTyping sets a callback invoked when typing state changes.
func (s *Store) SetOnTyping(fn func(project string)) {
	s.typingMu.Lock()
	defer s.typingMu.Unlock()
	s.onTyping = fn
}

// SetTyping marks a thread as having an agent actively composing a reply.
func (s *Store) SetTyping(project, path, threadID string) {
	key := project + ":" + path + ":" + threadID
	s.typingMu.Lock()
	s.typing[key] = time.Now()
	fn := s.onTyping
	s.typingMu.Unlock()
	if fn != nil {
		fn(project)
	}
}

// ClearTyping removes the typing indicator for a specific thread.
// Broadcasts a change event if the indicator was actually cleared.
func (s *Store) ClearTyping(project, path, threadID string) {
	key := project + ":" + path + ":" + threadID
	s.typingMu.Lock()
	_, cleared := s.typing[key]
	delete(s.typing, key)
	fn := s.onTyping
	s.typingMu.Unlock()
	if cleared && fn != nil {
		fn(project)
	}
}

// ClearProjectTyping removes all typing indicators for a project.
// Broadcasts a change event if any indicators were actually cleared.
func (s *Store) ClearProjectTyping(project string) {
	prefix := project + ":"
	s.typingMu.Lock()
	cleared := false
	for key := range s.typing {
		if strings.HasPrefix(key, prefix) {
			delete(s.typing, key)
			cleared = true
		}
	}
	fn := s.onTyping
	s.typingMu.Unlock()
	if cleared && fn != nil {
		fn(project)
	}
}

// TypingCount returns the number of threads with active typing indicators
// for the given project and file path. Like IsTyping, indicators auto-expire
// after 60 seconds.
func (s *Store) TypingCount(project, path string) int {
	prefix := project + ":" + path + ":"
	s.typingMu.RLock()
	defer s.typingMu.RUnlock()
	count := 0
	for key, t := range s.typing {
		if strings.HasPrefix(key, prefix) && time.Since(t) < 60*time.Second {
			count++
		}
	}
	return count
}

// IsTyping returns true if an agent is actively composing a reply to the given thread.
// Typing state auto-expires after 60 seconds.
func (s *Store) IsTyping(project, path, threadID string) bool {
	key := project + ":" + path + ":" + threadID
	s.typingMu.RLock()
	defer s.typingMu.RUnlock()
	t, ok := s.typing[key]
	if !ok {
		return false
	}
	return time.Since(t) < 60*time.Second
}

// HasTypingEntry returns true if a typing entry exists for the given thread,
// regardless of when it was set. Use this when the agent process is known to
// be running—explicit cleanup (ClearTyping on reply, ClearProjectTyping on
// exit) handles staleness.
func (s *Store) HasTypingEntry(project, path, threadID string) bool {
	key := project + ":" + path + ":" + threadID
	s.typingMu.RLock()
	defer s.typingMu.RUnlock()
	_, ok := s.typing[key]
	return ok
}

// TypingCountNoExpiry returns the number of threads with typing indicators
// for the given project and file path, without checking the 60-second expiry.
// Use this when the agent process is known to be running.
func (s *Store) TypingCountNoExpiry(project, path string) int {
	prefix := project + ":" + path + ":"
	s.typingMu.RLock()
	defer s.typingMu.RUnlock()
	count := 0
	for key := range s.typing {
		if strings.HasPrefix(key, prefix) {
			count++
		}
	}
	return count
}
