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

// Record updates the latest activity for a file. Always overwrites
// the previous activity — we only track the most recent one.
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
