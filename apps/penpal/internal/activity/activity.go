package activity

import (
	"path/filepath"
	"sort"
	"sync"
	"time"
)

type EventType string

const (
	FileViewed    EventType = "viewed"
	FileModified  EventType = "modified"
	FileCreated   EventType = "created"
	Comment       EventType = "comment"
	FilePublished EventType = "published"
)

// FileActivity represents the latest activity on a single file.
type FileActivity struct {
	Type      EventType `json:"type"`
	Timestamp time.Time `json:"timestamp"`
	Project   string    `json:"project"`
	FilePath  string    `json:"filePath"` // project-relative
	FileName  string    `json:"fileName"` // basename for display
}

// fileKey uniquely identifies a file across projects.
type fileKey struct {
	Project  string
	FilePath string
}

type Tracker struct {
	mu    sync.RWMutex
	files map[fileKey]*FileActivity
}

func New() *Tracker {
	return &Tracker{
		files: make(map[fileKey]*FileActivity),
	}
}

// RecordAt records activity with a specific timestamp. It does NOT overwrite
// existing entries — this is used to seed historical data from filesystem
// ModTimes so that runtime-observed events always take priority.
// E-PENPAL-ACTIVITY: seed-only recording that preserves existing events.
func (t *Tracker) RecordAt(activityType EventType, project, filePath string, timestamp time.Time) {
	key := fileKey{Project: project, FilePath: filePath}
	t.mu.Lock()
	defer t.mu.Unlock()

	if _, exists := t.files[key]; exists {
		return
	}
	t.files[key] = &FileActivity{
		Type:      activityType,
		Timestamp: timestamp,
		Project:   project,
		FilePath:  filePath,
		FileName:  filepath.Base(filePath),
	}
}

// Record updates the latest activity for a file. Always overwrites
// the previous activity — we only track the most recent one.
// E-PENPAL-ACTIVITY: one event per file with overwrite semantics.
func (t *Tracker) Record(activityType EventType, project, filePath string) {
	key := fileKey{Project: project, FilePath: filePath}
	t.mu.Lock()
	defer t.mu.Unlock()

	t.files[key] = &FileActivity{
		Type:      activityType,
		Timestamp: time.Now(),
		Project:   project,
		FilePath:  filePath,
		FileName:  filepath.Base(filePath),
	}
}

// RecentFiles returns up to `limit` files sorted by most recent activity.
// E-PENPAL-ACTIVITY: returns recent activity sorted by timestamp.
func (t *Tracker) RecentFiles(limit int) []FileActivity {
	t.mu.RLock()
	defer t.mu.RUnlock()

	all := make([]FileActivity, 0, len(t.files))
	for _, fa := range t.files {
		all = append(all, *fa)
	}
	sort.Slice(all, func(i, j int) bool {
		return all[i].Timestamp.After(all[j].Timestamp)
	})
	if limit > 0 && len(all) > limit {
		all = all[:limit]
	}
	return all
}

// Lookup returns the latest activity for a specific file, or nil if none tracked.
// E-PENPAL-ACTIVITY: returns a copy of the latest activity for a file.
func (t *Tracker) Lookup(project, filePath string) *FileActivity {
	t.mu.RLock()
	defer t.mu.RUnlock()

	fa := t.files[fileKey{Project: project, FilePath: filePath}]
	if fa == nil {
		return nil
	}
	copy := *fa
	return &copy
}
