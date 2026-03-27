package comments

import (
	"context"
	"strings"
	"sync"
	"time"

	"github.com/loganj/penpal/internal/activity"
	"github.com/loganj/penpal/internal/cache"
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
	workingMu  sync.RWMutex
	working    map[string]workingEntry // key: "project:path:threadId" -> working state
	onWorking  func(project string)    // called when working state changes
}

// FileComments holds all comment threads for a single file.
type FileComments struct {
	Threads []Thread `json:"threads"`
}

// Thread represents a comment thread anchored to a specific piece of text.
// E-PENPAL-THREAD-MODEL: Thread has ID/Status/Anchor/Comments/CreatedAt/ResolvedAt/ResolvedBy.
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
// E-PENPAL-ANCHOR-STRUCT: Anchor with SelectedText, Before, After, HeadingPath, StartLine, OccurrenceIndex, SvgSnippet/SvgRect.
type Anchor struct {
	SelectedText string   `json:"selectedText"`
	Before       string   `json:"before,omitempty"`
	After        string   `json:"after,omitempty"`
	HeadingPath  string   `json:"headingPath,omitempty"`
	StartLine    int      `json:"startLine,omitempty"`
	SvgSnippet   string   `json:"svgSnippet,omitempty"`
	SvgRect      *SvgRect `json:"svgRect,omitempty"`
}

// SvgRect describes a rectangle selection within an SVG diagram.
type SvgRect struct {
	X      float64 `json:"x"`
	Y      float64 `json:"y"`
	Width  float64 `json:"width"`
	Height float64 `json:"height"`
}

// Comment is a single message within a thread.
// E-PENPAL-THREAD-MODEL: Comment has ID/Author/Role/Body/CreatedAt/SuggestedReplies/InReplyTo/WorkingStartedAt.
type Comment struct {
	ID               string     `json:"id"`
	Author           string     `json:"author"`
	Role             string     `json:"role"` // "human" | "agent"
	Body             string     `json:"body"`
	CreatedAt        time.Time  `json:"createdAt"`
	SuggestedReplies []string   `json:"suggestedReplies,omitempty"`
	InReplyTo        string     `json:"inReplyTo,omitempty"`
	WorkingStartedAt *time.Time `json:"workingStartedAt,omitempty"`
}

// workingEntry tracks the state of an agent actively working on a thread.
// startedAt is immutable (set once, used for WorkingStartedAt ordering).
// lastRefreshed is updated by RefreshWorkingTimestamp to extend the 60s expiry.
// E-PENPAL-WORKING: stores when work started and which comment the agent is responding to.
type workingEntry struct {
	startedAt      time.Time // when the agent first started working (used for ordering)
	lastRefreshed  time.Time // when the entry was last refreshed (used for 60s expiry)
	afterCommentID string
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
		working:    make(map[string]workingEntry),
	}
}

// NotifyChange wakes all goroutines blocked in WaitForChange.
// E-PENPAL-CHANGE-SEQ: increments global monotonic sequence number and broadcasts.
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
//
// E-PENPAL-CHANGE-SEQ: WaitForChangeSince blocks until changeSeq advances or context cancels.
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
// E-PENPAL-HEARTBEAT: records agent activity in in-memory heartbeats map.
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
// E-PENPAL-HEARTBEAT: returns true if heartbeat is <60s old.
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

// SetOnWorking sets a callback invoked when working state changes.
func (s *Store) SetOnWorking(fn func(project string)) {
	s.workingMu.Lock()
	defer s.workingMu.Unlock()
	s.onWorking = fn
}

// SetWorking marks a thread as having an agent actively working on it.
// afterCommentID is the ID of the last comment the agent saw when it started working.
// Always updates both startedAt and afterCommentID — use RefreshWorkingTimestamp
// when only the expiry timer should be extended without changing the anchor position.
// E-PENPAL-WORKING: updates in-memory working map and triggers SSE event.
func (s *Store) SetWorking(project, path, threadID, afterCommentID string) {
	key := project + ":" + path + ":" + threadID
	s.workingMu.Lock()
	now := time.Now()
	s.working[key] = workingEntry{startedAt: now, lastRefreshed: now, afterCommentID: afterCommentID}
	fn := s.onWorking
	s.workingMu.Unlock()
	if fn != nil {
		fn(project)
	}
}

// RefreshWorkingTimestamp extends the expiry timer for a working entry without
// changing startedAt or afterCommentID. Use this in long-poll refresh paths
// where the agent hasn't actually re-read the thread.
// E-PENPAL-WORKING: refreshes lastRefreshed while preserving startedAt and afterCommentID.
func (s *Store) RefreshWorkingTimestamp(project, path, threadID string) {
	key := project + ":" + path + ":" + threadID
	s.workingMu.Lock()
	if existing, ok := s.working[key]; ok {
		existing.lastRefreshed = time.Now()
		s.working[key] = existing
	}
	s.workingMu.Unlock()
}

// ClearWorking removes the working indicator for a specific thread.
// Broadcasts a change event if the indicator was actually cleared.
// E-PENPAL-WORKING: clears working entry and triggers SSE event.
func (s *Store) ClearWorking(project, path, threadID string) {
	key := project + ":" + path + ":" + threadID
	s.workingMu.Lock()
	_, cleared := s.working[key]
	delete(s.working, key)
	fn := s.onWorking
	s.workingMu.Unlock()
	if cleared && fn != nil {
		fn(project)
	}
}

// ClearProjectWorking removes all working indicators for a project.
// Broadcasts a change event if any indicators were actually cleared.
func (s *Store) ClearProjectWorking(project string) {
	prefix := project + ":"
	s.workingMu.Lock()
	cleared := false
	for key := range s.working {
		if strings.HasPrefix(key, prefix) {
			delete(s.working, key)
			cleared = true
		}
	}
	fn := s.onWorking
	s.workingMu.Unlock()
	if cleared && fn != nil {
		fn(project)
	}
}

// WorkingCount returns the number of threads with active working indicators
// for the given project and file path. Indicators auto-expire after 60 seconds.
// E-PENPAL-WORKING: counts non-expired working entries with 60s expiry.
func (s *Store) WorkingCount(project, path string) int {
	prefix := project + ":" + path + ":"
	s.workingMu.RLock()
	defer s.workingMu.RUnlock()
	count := 0
	for key, entry := range s.working {
		if strings.HasPrefix(key, prefix) && time.Since(entry.lastRefreshed) < 60*time.Second {
			count++
		}
	}
	return count
}

// IsWorking returns true if an agent is actively working on the given thread.
// Working state auto-expires after 60 seconds.
// E-PENPAL-WORKING: checks working entry with 60s expiry.
func (s *Store) IsWorking(project, path, threadID string) bool {
	key := project + ":" + path + ":" + threadID
	s.workingMu.RLock()
	defer s.workingMu.RUnlock()
	entry, ok := s.working[key]
	if !ok {
		return false
	}
	return time.Since(entry.lastRefreshed) < 60*time.Second
}

// HasWorkingEntry returns true if a working entry exists for the given thread,
// regardless of when it was set. Use this when the agent process is known to
// be running—explicit cleanup (ClearWorking on reply, ClearProjectWorking on
// exit) handles staleness.
func (s *Store) HasWorkingEntry(project, path, threadID string) bool {
	key := project + ":" + path + ":" + threadID
	s.workingMu.RLock()
	defer s.workingMu.RUnlock()
	_, ok := s.working[key]
	return ok
}

// WorkingAfterCommentID returns the afterCommentID for a working thread,
// or empty string if no working entry exists.
// E-PENPAL-WORKING: retrieves the afterCommentID from the working entry.
func (s *Store) WorkingAfterCommentID(project, path, threadID string) string {
	key := project + ":" + path + ":" + threadID
	s.workingMu.RLock()
	defer s.workingMu.RUnlock()
	entry, ok := s.working[key]
	if !ok {
		return ""
	}
	return entry.afterCommentID
}

// WorkingStartedAt returns the startedAt time for a working thread,
// or zero time if no working entry exists.
// E-PENPAL-WORKING: retrieves the startedAt time from the working entry.
func (s *Store) WorkingStartedAt(project, path, threadID string) time.Time {
	key := project + ":" + path + ":" + threadID
	s.workingMu.RLock()
	defer s.workingMu.RUnlock()
	entry, ok := s.working[key]
	if !ok {
		return time.Time{}
	}
	return entry.startedAt
}

// WorkingCountNoExpiry returns the number of threads with working indicators
// for the given project and file path, without checking the 60-second expiry.
// Use this when the agent process is known to be running.
func (s *Store) WorkingCountNoExpiry(project, path string) int {
	prefix := project + ":" + path + ":"
	s.workingMu.RLock()
	defer s.workingMu.RUnlock()
	count := 0
	for key := range s.working {
		if strings.HasPrefix(key, prefix) {
			count++
		}
	}
	return count
}
