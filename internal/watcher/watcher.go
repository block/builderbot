package watcher

import (
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"
	"github.com/loganj/birdseye/internal/cache"
)

// EventType represents the type of change
type EventType string

const (
	EventProjectsChanged EventType = "projects"
	EventFilesChanged    EventType = "files"
	EventCommentsChanged EventType = "comments"
)

// Event represents a change notification
type Event struct {
	Type    EventType `json:"type"`
	Project string    `json:"project,omitempty"`
}

// Watcher watches for filesystem changes and updates the cache
type Watcher struct {
	cache    *cache.Cache
	watcher  *fsnotify.Watcher
	done     chan struct{}
	eventsMu sync.RWMutex
	subs     map[chan Event]struct{}

	// Debounce timers to coalesce rapid changes
	debounce   map[string]*time.Timer
	debounceMu sync.Mutex
}

// New creates a new watcher
func New(c *cache.Cache) (*Watcher, error) {
	fw, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, err
	}

	w := &Watcher{
		cache:    c,
		watcher:  fw,
		done:     make(chan struct{}),
		subs:     make(map[chan Event]struct{}),
		debounce: make(map[string]*time.Timer),
	}

	return w, nil
}

// Start begins watching for changes
func (w *Watcher) Start() error {
	root := w.cache.Root()

	// Watch the root directory for new projects
	if err := w.watcher.Add(root); err != nil {
		return err
	}

	// Watch each project's thoughts directory and .birdseye/comments directory
	for _, p := range w.cache.Projects() {
		if err := w.watchDir(p.ThoughtsPath); err != nil {
			log.Printf("Warning: could not watch %s: %v", p.ThoughtsPath, err)
		}
		commentsDir := filepath.Join(p.ThoughtsPath, ".birdseye", "comments")
		if info, err := os.Stat(commentsDir); err == nil && info.IsDir() {
			if err := w.watchDir(commentsDir); err != nil {
				log.Printf("Warning: could not watch %s: %v", commentsDir, err)
			}
		}
	}

	go w.loop()
	return nil
}

// watchDir recursively watches a directory and its subdirectories
func (w *Watcher) watchDir(dir string) error {
	return filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil
		}
		if info.IsDir() {
			if err := w.watcher.Add(path); err != nil {
				log.Printf("Warning: could not watch %s: %v", path, err)
			}
		}
		return nil
	})
}

// Stop stops the watcher and closes all subscriber channels
func (w *Watcher) Stop() {
	close(w.done)
	w.watcher.Close()

	// Close all subscriber channels so SSE handlers exit
	w.eventsMu.Lock()
	for ch := range w.subs {
		close(ch)
		delete(w.subs, ch)
	}
	w.eventsMu.Unlock()
}

// Subscribe returns a channel that receives events
func (w *Watcher) Subscribe() chan Event {
	ch := make(chan Event, 10)
	w.eventsMu.Lock()
	w.subs[ch] = struct{}{}
	w.eventsMu.Unlock()
	return ch
}

// Unsubscribe removes a subscription
func (w *Watcher) Unsubscribe(ch chan Event) {
	w.eventsMu.Lock()
	delete(w.subs, ch)
	close(ch)
	w.eventsMu.Unlock()
}

// Broadcast sends an event to all subscribers
func (w *Watcher) Broadcast(evt Event) {
	w.eventsMu.RLock()
	defer w.eventsMu.RUnlock()
	for ch := range w.subs {
		select {
		case ch <- evt:
		default:
			// Skip if channel is full
		}
	}
}

func (w *Watcher) loop() {
	for {
		select {
		case <-w.done:
			return
		case event, ok := <-w.watcher.Events:
			if !ok {
				return
			}
			w.handleEvent(event)
		case err, ok := <-w.watcher.Errors:
			if !ok {
				return
			}
			log.Printf("Watcher error: %v", err)
		}
	}
}

func (w *Watcher) handleEvent(event fsnotify.Event) {
	path := event.Name
	root := w.cache.Root()

	// Check if this is a change in the root directory (new/removed project)
	if filepath.Dir(path) == root {
		w.debounceRefresh("_root_", func() {
			// Check if a thoughts/ dir exists in this potential project
			thoughtsPath := filepath.Join(path, "thoughts")
			if info, err := os.Stat(thoughtsPath); err == nil && info.IsDir() {
				// New project or project with thoughts/ added
				if err := w.cache.RescanProjects(); err == nil {
					// Watch the new project's thoughts directory
					w.watchDir(thoughtsPath)
					w.Broadcast(Event{Type: EventProjectsChanged})
				}
			} else if event.Op&fsnotify.Remove != 0 {
				// Project removed
				if err := w.cache.RescanProjects(); err == nil {
					w.Broadcast(Event{Type: EventProjectsChanged})
				}
			}
		})
		return
	}

	// Find which project this path belongs to
	projectName := w.findProjectForPath(path)
	if projectName == "" {
		return
	}

	// If a new directory was created, watch it
	if event.Op&fsnotify.Create != 0 {
		if info, err := os.Stat(path); err == nil && info.IsDir() {
			w.watcher.Add(path)
		}
	}

	// Handle changes in .birdseye/comments/ directories
	if strings.Contains(path, "/.birdseye/") && strings.HasSuffix(path, ".json") {
		w.debounceRefresh("comments:"+projectName, func() {
			w.Broadcast(Event{Type: EventCommentsChanged, Project: projectName})
		})
		return
	}

	// Only care about .md files for file list updates
	if !strings.HasSuffix(path, ".md") && event.Op&fsnotify.Create == 0 {
		return
	}

	w.debounceRefresh(projectName, func() {
		w.cache.RefreshProject(projectName)
		w.cache.RefreshProjectGitInfo(projectName)
		w.Broadcast(Event{Type: EventFilesChanged, Project: projectName})
	})
}

// findProjectForPath finds which project a path belongs to
func (w *Watcher) findProjectForPath(path string) string {
	for _, p := range w.cache.Projects() {
		if strings.HasPrefix(path, p.ThoughtsPath) {
			return p.Name
		}
	}
	return ""
}

// debounceRefresh debounces rapid changes to the same project
func (w *Watcher) debounceRefresh(key string, fn func()) {
	w.debounceMu.Lock()
	defer w.debounceMu.Unlock()

	if timer, ok := w.debounce[key]; ok {
		timer.Stop()
	}

	w.debounce[key] = time.AfterFunc(100*time.Millisecond, func() {
		w.debounceMu.Lock()
		delete(w.debounce, key)
		w.debounceMu.Unlock()
		fn()
	})
}
