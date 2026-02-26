package watcher

import (
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"
	"github.com/loganj/penpal/internal/activity"
	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/discovery"
)

// EventType represents the type of change
type EventType string

const (
	EventProjectsChanged EventType = "projects"
	EventFilesChanged    EventType = "files"
	EventCommentsChanged EventType = "comments"
	EventAgentsChanged   EventType = "agents"
	EventNavigate        EventType = "navigate"
)

// Event represents a change notification
type Event struct {
	Type    EventType `json:"type"`
	Project string    `json:"project,omitempty"`
	Path    string    `json:"path,omitempty"`
}

// Watcher watches for filesystem changes and updates the cache
type Watcher struct {
	cache    *cache.Cache
	activity *activity.Tracker
	watcher  *fsnotify.Watcher
	done     chan struct{}
	eventsMu sync.RWMutex
	subs     map[chan Event]struct{}

	// Debounce timers to coalesce rapid changes
	debounce   map[string]*time.Timer
	debounceMu sync.Mutex

	// Multi-workspace support
	workspacePaths []string
	discoverFn     func() ([]discovery.Project, error) // called on workspace change
}

// New creates a new watcher
func New(c *cache.Cache, act *activity.Tracker) (*Watcher, error) {
	fw, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, err
	}

	w := &Watcher{
		cache:    c,
		activity: act,
		watcher:  fw,
		done:     make(chan struct{}),
		subs:     make(map[chan Event]struct{}),
		debounce: make(map[string]*time.Timer),
	}

	return w, nil
}

// Start begins watching for changes across all workspaces and project sources.
func (w *Watcher) Start(workspacePaths []string, discoverFn func() ([]discovery.Project, error)) error {
	w.workspacePaths = workspacePaths
	w.discoverFn = discoverFn

	// Watch all workspace directories for new/removed projects
	for _, ws := range workspacePaths {
		if err := w.watcher.Add(ws); err != nil {
			log.Printf("Warning: could not watch workspace %s: %v", ws, err)
		}
	}

	// Watch each project's sources and .penpal/comments directory
	for _, p := range w.cache.Projects() {
		w.watchProject(p)
	}

	go w.loop()
	return nil
}

// Refresh updates workspace paths and watches any new projects.
// Called after config changes (add/remove workspace or project).
func (w *Watcher) Refresh(workspacePaths []string, projects []discovery.Project) {
	w.workspacePaths = workspacePaths
	for _, ws := range workspacePaths {
		if err := w.watcher.Add(ws); err != nil {
			log.Printf("Warning: could not watch workspace %s: %v", ws, err)
		}
	}
	for _, p := range projects {
		w.watchProject(p)
	}
}

// watchProject sets up file watches for all sources and comments of a project.
func (w *Watcher) watchProject(p discovery.Project) {
	// Watch the project root directory itself so we can detect new
	// auto-detectable source directories (e.g., thoughts/, .rp1/) at runtime.
	if err := w.watcher.Add(p.Path); err != nil {
		log.Printf("Warning: could not watch project root %s: %v", p.Path, err)
	}

	for _, src := range p.Sources {
		if src.RootPath != "" {
			if err := w.watchDir(src.RootPath); err != nil {
				log.Printf("Warning: could not watch %s: %v", src.RootPath, err)
			}
		}
	}
	commentsDir := filepath.Join(p.Path, ".penpal", "comments")
	if info, err := os.Stat(commentsDir); err == nil && info.IsDir() {
		if err := w.watchDir(commentsDir); err != nil {
			log.Printf("Warning: could not watch %s: %v", commentsDir, err)
		}
	}
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

	// Check if this is a change in a workspace directory (new/removed project)
	parentDir := filepath.Dir(path)
	for _, ws := range w.workspacePaths {
		if parentDir == ws {
			w.debounceRefresh("workspace:"+ws, func() {
				if w.discoverFn != nil {
					projects, err := w.discoverFn()
					if err == nil {
						w.cache.RescanWith(projects)
						// Watch any new project sources
						for _, p := range projects {
							w.watchProject(p)
						}
						w.Broadcast(Event{Type: EventProjectsChanged})
					}
				}
			})
			return
		}
	}

	// Check if a new auto-detectable source directory was created/removed
	// under a project root (e.g., someone creates thoughts/ or .rp1/).
	// For Create events, verify it's actually a directory (not a file with
	// the same name). For Remove/Rename the path is already gone so we
	// can't stat it — just trigger the rescan and let DetectSources sort it out.
	if event.Op&(fsnotify.Create|fsnotify.Remove|fsnotify.Rename) != 0 {
		if event.Op&fsnotify.Create != 0 {
			if info, err := os.Stat(path); err != nil || !info.IsDir() {
				goto notAutoDetect
			}
		}
		dirName := filepath.Base(path)
		for _, st := range discovery.AllSourceTypes() {
			if st.AutoDetectDir != "" && st.AutoDetectDir == dirName {
				// Check if parent is a project root
				for _, p := range w.cache.Projects() {
					if parentDir == p.Path {
						w.debounceRefresh("sources:"+p.QualifiedName(), func() {
							if w.discoverFn != nil {
								projects, err := w.discoverFn()
								if err == nil {
									w.cache.RescanWith(projects)
									for _, proj := range projects {
										w.watchProject(proj)
									}
									w.Broadcast(Event{Type: EventProjectsChanged})
								}
							}
						})
						return
					}
				}
			}
		}
	}
notAutoDetect:

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

	// Handle changes in .penpal/comments/ directories
	if strings.Contains(path, "/.penpal/") && strings.HasSuffix(path, ".json") {
		w.debounceRefresh("comments:"+projectName, func() {
			w.Broadcast(Event{Type: EventCommentsChanged, Project: projectName})
		})
		return
	}

	// Only care about .md files for file list updates
	if !strings.HasSuffix(path, ".md") && event.Op&fsnotify.Create == 0 {
		return
	}

	// Record activity for .md file changes before debouncing
	if strings.HasSuffix(path, ".md") && w.activity != nil {
		if project := w.cache.FindProject(projectName); project != nil {
			if relPath, err := filepath.Rel(project.Path, path); err == nil {
				evtType := activity.FileModified
				if event.Op&fsnotify.Create != 0 {
					evtType = activity.FileCreated
				}
				w.activity.Record(evtType, projectName, relPath)
			}
		}
	}

	w.debounceRefresh(projectName, func() {
		w.cache.RefreshProject(projectName)
		w.cache.RefreshProjectGitInfo(projectName)
		w.Broadcast(Event{Type: EventFilesChanged, Project: projectName})
	})
}

// findProjectForPath finds which project a path belongs to by checking
// all source roots and .penpal directories. Returns the qualified name.
func (w *Watcher) findProjectForPath(path string) string {
	for _, p := range w.cache.Projects() {
		// Check all source roots
		for _, src := range p.Sources {
			if src.RootPath != "" && strings.HasPrefix(path, src.RootPath+"/") {
				return p.QualifiedName()
			}
		}
		// Check .penpal directory
		if strings.HasPrefix(path, p.Path+"/.penpal/") {
			return p.QualifiedName()
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
