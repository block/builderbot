package activity

import (
	"encoding/json"
	"os"
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
	mu       sync.RWMutex
	files    map[fileKey]*FileActivity
	onChange func() // called (under no lock) after Record() mutates state
}

func New() *Tracker {
	return &Tracker{
		files: make(map[fileKey]*FileActivity),
	}
}

// SetOnChange registers a callback invoked after every Record() call.
// E-PENPAL-ACTIVITY-PERSIST: hook for debounced save after mutations.
func (t *Tracker) SetOnChange(fn func()) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.onChange = fn
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
	t.files[key] = &FileActivity{
		Type:      activityType,
		Timestamp: time.Now(),
		Project:   project,
		FilePath:  filePath,
		FileName:  filepath.Base(filePath),
	}
	cb := t.onChange
	t.mu.Unlock()

	if cb != nil {
		cb()
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

// maxPersistedEntries caps the number of entries saved to disk.
// Only the most recent entries are kept; older ones are pruned on save.
const maxPersistedEntries = 500

// Save persists activity entries to a JSON file (atomic write).
// Prunes to the most recent maxPersistedEntries to bound file size.
// E-PENPAL-ACTIVITY-PERSIST: atomic save via MkdirAll + tmp + rename.
func (t *Tracker) Save(path string) error {
	t.mu.RLock()
	entries := make([]FileActivity, 0, len(t.files))
	for _, fa := range t.files {
		entries = append(entries, *fa)
	}
	t.mu.RUnlock()

	sort.Slice(entries, func(i, j int) bool {
		return entries[i].Timestamp.After(entries[j].Timestamp)
	})
	if len(entries) > maxPersistedEntries {
		entries = entries[:maxPersistedEntries]
	}

	data, err := json.Marshal(entries)
	if err != nil {
		return err
	}

	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// Load reads activity entries from a JSON file and seeds them via RecordAt
// so that runtime events always take priority over persisted data.
// E-PENPAL-ACTIVITY-PERSIST: load persisted activity on startup.
func (t *Tracker) Load(path string) error {
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}

	var entries []FileActivity
	if err := json.Unmarshal(data, &entries); err != nil {
		return err
	}

	for i := range entries {
		e := &entries[i]
		t.RecordAt(e.Type, e.Project, e.FilePath, e.Timestamp)
	}
	return nil
}
