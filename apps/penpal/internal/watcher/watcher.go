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
	Type     EventType `json:"type"`
	Project  string    `json:"project,omitempty"`
	Path     string    `json:"path,omitempty"`
	Worktree string    `json:"worktree,omitempty"`
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

	// Focus tracking: only deep-watch what the user is looking at
	focusMu      sync.Mutex
	focusKey     string   // dedup key for the current focus (e.g. "project:X" or "file:X/path")
	focusWatched []string // paths added for the current focus (for cleanup)
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

// Start begins watching for changes across all workspaces.
// Only workspace directories are watched initially; individual projects
// are deep-watched on demand via FocusProject.
func (w *Watcher) Start(workspacePaths []string, discoverFn func() ([]discovery.Project, error)) error {
	w.workspacePaths = workspacePaths
	w.discoverFn = discoverFn

	// Watch all workspace directories for new/removed projects
	for _, ws := range workspacePaths {
		if err := w.watcher.Add(ws); err != nil {
			log.Printf("Warning: could not watch workspace %s: %v", ws, err)
		}
	}

	// Watch each project root (shallow — just for detecting new source dirs)
	for _, p := range w.cache.Projects() {
		if err := w.watcher.Add(p.Path); err != nil {
			log.Printf("Warning: could not watch project root %s: %v", p.Path, err)
		}
	}

	go w.loop()
	return nil
}

// Refresh updates workspace paths and watches any new project roots (shallow).
// Called after config changes (add/remove workspace or project).
func (w *Watcher) Refresh(workspacePaths []string, projects []discovery.Project) {
	w.workspacePaths = workspacePaths
	for _, ws := range workspacePaths {
		if err := w.watcher.Add(ws); err != nil {
			log.Printf("Warning: could not watch workspace %s: %v", ws, err)
		}
	}
	for _, p := range projects {
		if err := w.watcher.Add(p.Path); err != nil {
			log.Printf("Warning: could not watch project root %s: %v", p.Path, err)
		}
	}
}

// FocusProject watches a project's sources and comments directories.
// Use this when the user is on a ProjectPage.
func (w *Watcher) FocusProject(name string) {
	key := "project:" + name
	w.focusMu.Lock()
	defer w.focusMu.Unlock()

	if w.focusKey == key {
		return
	}
	w.removeFocusWatches()
	w.focusKey = key

	project := w.cache.FindProject(name)
	if project == nil {
		return
	}

	w.focusWatched = nil
	w.watchProjectSources(*project)
	log.Printf("Focus: watching project %s sources (%d dirs)", name, len(w.focusWatched))
}

// FocusFile watches only the directory containing a specific file and its
// comments directory. Use this when the user is on a FilePage.
func (w *Watcher) FocusFile(projectName, filePath, worktree string) {
	key := "file:" + projectName + "/" + worktree + "/" + filePath
	w.focusMu.Lock()
	defer w.focusMu.Unlock()

	if w.focusKey == key {
		return
	}
	w.removeFocusWatches()
	w.focusKey = key

	project := w.cache.FindProject(projectName)
	if project == nil {
		return
	}

	w.focusWatched = nil

	// Determine the base path (main project or worktree)
	basePath := project.Path
	if worktree != "" {
		for _, wt := range project.Worktrees {
			if wt.Name == worktree {
				basePath = wt.Path
				break
			}
		}
	}

	// Watch only the directory containing the file (for external edits).
	// Comments are broadcast via the API — no fs watch needed.
	absFile := filepath.Join(basePath, filePath)
	fileDir := filepath.Dir(absFile)
	if info, err := os.Stat(fileDir); err == nil && info.IsDir() {
		if err := w.watcher.Add(fileDir); err == nil {
			w.focusWatched = append(w.focusWatched, fileDir)
		}
	}

	log.Printf("Focus: watching file %s/%s (%d dirs)", projectName, filePath, len(w.focusWatched))
}

// ClearFocus removes all deep watches.
func (w *Watcher) ClearFocus() {
	w.focusMu.Lock()
	defer w.focusMu.Unlock()
	w.removeFocusWatches()
	w.focusKey = ""
}

// watchProjectSources adds watches for all sources and comments of a project.
// Must be called with focusMu held.
func (w *Watcher) watchProjectSources(p discovery.Project) {
	for _, src := range p.Sources {
		if src.RootPath != "" {
			w.walkAndWatch(src.RootPath)
		}
	}
	commentsDir := filepath.Join(p.Path, ".penpal", "comments")
	if info, err := os.Stat(commentsDir); err == nil && info.IsDir() {
		w.walkAndWatch(commentsDir)
	}

	for _, wt := range p.Worktrees {
		if wt.IsMain {
			continue
		}
		for _, st := range discovery.AllSourceTypes() {
			if st.AutoDetectDir == "" {
				continue
			}
			wtSourceDir := filepath.Join(wt.Path, st.AutoDetectDir)
			if info, err := os.Stat(wtSourceDir); err == nil && info.IsDir() {
				w.walkAndWatch(wtSourceDir)
			}
		}
		for _, src := range p.Sources {
			if src.RootPath == "" {
				continue
			}
			rel, err := filepath.Rel(p.Path, src.RootPath)
			if err != nil {
				continue
			}
			wtSourceDir := filepath.Join(wt.Path, rel)
			if info, err := os.Stat(wtSourceDir); err == nil && info.IsDir() {
				w.walkAndWatch(wtSourceDir)
			}
		}
		wtCommentsDir := filepath.Join(wt.Path, ".penpal", "comments")
		if info, err := os.Stat(wtCommentsDir); err == nil && info.IsDir() {
			w.walkAndWatch(wtCommentsDir)
		}
	}
}

// walkAndWatch recursively watches a directory, recording paths for later cleanup.
// Must be called with focusMu held.
func (w *Watcher) walkAndWatch(dir string) {
	filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil
		}
		if info.IsDir() {
			if err := w.watcher.Add(path); err == nil {
				w.focusWatched = append(w.focusWatched, path)
			}
		}
		return nil
	})
}

// removeFocusWatches removes all watches added by addFocusWatch.
// Must be called with focusMu held.
func (w *Watcher) removeFocusWatches() {
	for _, path := range w.focusWatched {
		w.watcher.Remove(path)
	}
	w.focusWatched = nil
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
						// Shallow-watch new project roots
						for _, p := range projects {
							w.watcher.Add(p.Path)
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
										w.watcher.Add(proj.Path)
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

	// If a new directory was created inside a focused project's sources, watch it
	if event.Op&fsnotify.Create != 0 {
		if info, err := os.Stat(path); err == nil && info.IsDir() {
			w.focusMu.Lock()
			if w.focusKey == "project:"+projectName {
				if err := w.watcher.Add(path); err == nil {
					w.focusWatched = append(w.focusWatched, path)
				}
			}
			w.focusMu.Unlock()
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
// all source roots, .penpal directories, and worktree directories.
// Returns the qualified name.
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
		// Check worktree directories
		for _, wt := range p.Worktrees {
			if wt.IsMain {
				continue
			}
			if strings.HasPrefix(path, wt.Path+"/") {
				return p.QualifiedName()
			}
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
