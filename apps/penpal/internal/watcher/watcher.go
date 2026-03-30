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

type focusKind string

const (
	focusKindProject     focusKind = "project"
	focusKindFile        focusKind = "file"
	defaultFocusWindowID           = "__default__"
)

type focusTarget struct {
	Kind     focusKind
	Project  string
	Path     string
	Worktree string
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

	// Focus tracking: union deep watches across focused windows/tabs.
	focusMu        sync.Mutex
	windowFocuses  map[string]focusTarget
	baseWatched    map[string]struct{}
	dynamicWatched map[string]struct{}

	// E-PENPAL-WATCHER: accumulating debounce for incremental file updates.
	// Collects per-file paths and ops during the debounce window, then applies
	// incremental cache mutations instead of full project walks.
	filePending   map[string]map[string]fsnotify.Op // projectName → absPath → accumulated op
	filePendingMu sync.Mutex
	fileTimers    map[string]*time.Timer // projectName → debounce timer
}

// New creates a new watcher
// E-PENPAL-WATCHER: initializes fsnotify bridge with two-tier watch sets and debounce map.
func New(c *cache.Cache, act *activity.Tracker) (*Watcher, error) {
	fw, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, err
	}

	w := &Watcher{
		cache:          c,
		activity:       act,
		watcher:        fw,
		done:           make(chan struct{}),
		subs:           make(map[chan Event]struct{}),
		debounce:       make(map[string]*time.Timer),
		windowFocuses:  make(map[string]focusTarget),
		baseWatched:    make(map[string]struct{}),
		dynamicWatched: make(map[string]struct{}),
		filePending:    make(map[string]map[string]fsnotify.Op),
		fileTimers:     make(map[string]*time.Timer),
	}

	return w, nil
}

// Start begins watching for changes across all workspaces.
// Only workspace directories are watched initially; individual projects
// are deep-watched on demand via FocusProject.
// E-PENPAL-WATCHER: starts two-tier watch (base + dynamic) and event loop.
func (w *Watcher) Start(workspacePaths []string, discoverFn func() ([]discovery.Project, error)) error {
	w.workspacePaths = workspacePaths
	w.discoverFn = discoverFn

	w.focusMu.Lock()
	w.syncBaseWatchesLocked(workspacePaths, w.cache.Projects())
	w.syncDynamicWatchesLocked()
	w.focusMu.Unlock()

	go w.loop()
	return nil
}

// Refresh updates workspace paths and watches any new project roots (shallow).
// Called after config changes (add/remove workspace or project).
func (w *Watcher) Refresh(workspacePaths []string, projects []discovery.Project) {
	w.workspacePaths = workspacePaths
	w.focusMu.Lock()
	w.syncBaseWatchesLocked(workspacePaths, projects)
	w.syncDynamicWatchesLocked()
	w.focusMu.Unlock()
}

// FocusProject watches a project's sources and comments directories using the
// shared legacy focus slot.
func (w *Watcher) FocusProject(name string) {
	w.SetWindowFocusProject(defaultFocusWindowID, name)
}

// SetWindowFocusProject watches a project's sources and comments directories
// for a specific window.
// E-PENPAL-FOCUS: sets project-level focus for a specific window.
func (w *Watcher) SetWindowFocusProject(windowID, name string) {
	w.setWindowFocus(normalizeWindowID(windowID), focusTarget{
		Kind:    focusKindProject,
		Project: name,
	})
}

// FocusFile watches only the directory containing a specific file using the
// shared legacy focus slot.
func (w *Watcher) FocusFile(projectName, filePath, worktree string) {
	w.SetWindowFocusFile(defaultFocusWindowID, projectName, filePath, worktree)
}

// SetWindowFocusFile watches only the directory containing a specific file for
// a specific window.
// E-PENPAL-FOCUS: sets file-level focus for a specific window.
func (w *Watcher) SetWindowFocusFile(windowID, projectName, filePath, worktree string) {
	w.setWindowFocus(normalizeWindowID(windowID), focusTarget{
		Kind:     focusKindFile,
		Project:  projectName,
		Path:     filePath,
		Worktree: worktree,
	})
}

// ClearFocus removes all deep watches for legacy callers.
func (w *Watcher) ClearFocus() {
	w.ClearAllFocus()
}

// ClearWindowFocus removes deep watches for a specific window.
// E-PENPAL-FOCUS: removes focus for a specific window, recomputes union.
func (w *Watcher) ClearWindowFocus(windowID string) {
	windowID = normalizeWindowID(windowID)
	w.focusMu.Lock()
	defer w.focusMu.Unlock()
	if _, ok := w.windowFocuses[windowID]; !ok {
		return
	}
	delete(w.windowFocuses, windowID)
	w.syncDynamicWatchesLocked()
}

// ClearAllFocus removes all deep watches across every window.
func (w *Watcher) ClearAllFocus() {
	w.focusMu.Lock()
	defer w.focusMu.Unlock()
	clear(w.windowFocuses)
	w.syncDynamicWatchesLocked()
}

func (w *Watcher) setWindowFocus(windowID string, target focusTarget) {
	w.focusMu.Lock()
	defer w.focusMu.Unlock()
	if current, ok := w.windowFocuses[windowID]; ok && current == target {
		return
	}
	w.windowFocuses[windowID] = target
	w.syncDynamicWatchesLocked()
}

func normalizeWindowID(windowID string) string {
	if windowID == "" {
		return defaultFocusWindowID
	}
	return windowID
}

// collectProjectWatchDirs returns all directories that should be watched for a
// project-level focus target.
func (w *Watcher) collectProjectWatchDirs(p discovery.Project) map[string]struct{} {
	dirs := make(map[string]struct{})
	for _, src := range p.Sources {
		if src.RootPath != "" {
			stName := src.SourceTypeName
			if stName == "" {
				stName = src.Name
			}
			st := discovery.GetSourceType(stName)
			var sd map[string]bool
			if st != nil {
				sd = st.SkipDirs
			}
			collectWalkDirs(src.RootPath, sd, dirs)
		}
		// Watch directories containing individually listed files.
		// Skip the project root — it's already shallow-watched by Start()
		// and must not be added to focusWatched (removeFocusWatches would
		// inadvertently drop the baseline watch).
		for _, f := range src.Files {
			dir := filepath.Clean(filepath.Dir(f))
			if dir == filepath.Clean(p.Path) {
				continue
			}
			if info, err := os.Stat(dir); err == nil && info.IsDir() {
				dirs[dir] = struct{}{}
			}
		}
	}
	commentsDir := filepath.Join(p.Path, ".penpal", "comments")
	if info, err := os.Stat(commentsDir); err == nil && info.IsDir() {
		collectWalkDirs(commentsDir, nil, dirs)
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
				collectWalkDirs(wtSourceDir, st.SkipDirs, dirs)
			}
		}
		for _, src := range p.Sources {
			if src.RootPath == "" {
				continue
			}
			stName := src.SourceTypeName
			if stName == "" {
				stName = src.Name
			}
			srcSt := discovery.GetSourceType(stName)
			var sd map[string]bool
			if srcSt != nil {
				sd = srcSt.SkipDirs
			}
			rel, err := filepath.Rel(p.Path, src.RootPath)
			if err != nil {
				continue
			}
			wtSourceDir := filepath.Join(wt.Path, rel)
			if info, err := os.Stat(wtSourceDir); err == nil && info.IsDir() {
				collectWalkDirs(wtSourceDir, sd, dirs)
			}
		}
		wtCommentsDir := filepath.Join(wt.Path, ".penpal", "comments")
		if info, err := os.Stat(wtCommentsDir); err == nil && info.IsDir() {
			collectWalkDirs(wtCommentsDir, nil, dirs)
		}
	}
	return dirs
}

// collectFileWatchDirs returns directories that should be watched for a file
// focus target.
func (w *Watcher) collectFileWatchDirs(target focusTarget) map[string]struct{} {
	dirs := make(map[string]struct{})
	project := w.cache.FindProject(target.Project)
	if project == nil {
		return dirs
	}

	basePath := project.Path
	if target.Worktree != "" {
		for _, wt := range project.Worktrees {
			if wt.Name == target.Worktree {
				basePath = wt.Path
				break
			}
		}
	}

	fileDir := filepath.Clean(filepath.Dir(filepath.Join(basePath, target.Path)))
	if info, err := os.Stat(fileDir); err == nil && info.IsDir() {
		dirs[fileDir] = struct{}{}
	}
	return dirs
}

// collectWalkDirs recursively collects a directory and all subdirectories.
// skipDirs is a set of directory names to skip (e.g., ".git", "node_modules").
func collectWalkDirs(dir string, skipDirs map[string]bool, out map[string]struct{}) {
	filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil
		}
		if info.IsDir() {
			if skipDirs != nil && skipDirs[info.Name()] {
				return filepath.SkipDir
			}
			out[filepath.Clean(path)] = struct{}{}
		}
		return nil
	})
}

func (w *Watcher) syncBaseWatchesLocked(workspacePaths []string, projects []discovery.Project) {
	desired := make(map[string]struct{}, len(workspacePaths)+len(projects))
	for _, ws := range workspacePaths {
		desired[filepath.Clean(ws)] = struct{}{}
	}
	for _, p := range projects {
		desired[filepath.Clean(p.Path)] = struct{}{}
	}

	for path := range w.baseWatched {
		if _, ok := desired[path]; ok {
			continue
		}
		delete(w.baseWatched, path)
		if _, keep := w.dynamicWatched[path]; keep {
			continue
		}
		if err := w.watcher.Remove(path); err != nil {
			log.Printf("Warning: could not unwatch base path %s: %v", path, err)
		}
	}

	for path := range desired {
		if _, ok := w.baseWatched[path]; ok {
			continue
		}
		if _, alreadyDynamic := w.dynamicWatched[path]; !alreadyDynamic {
			if err := w.watcher.Add(path); err != nil {
				log.Printf("Warning: could not watch base path %s: %v", path, err)
				continue
			}
		}
		w.baseWatched[path] = struct{}{}
	}
}

func (w *Watcher) syncDynamicWatchesLocked() {
	desired := make(map[string]struct{})
	for _, target := range w.windowFocuses {
		switch target.Kind {
		case focusKindProject:
			project := w.cache.FindProject(target.Project)
			if project == nil {
				continue
			}
			for path := range w.collectProjectWatchDirs(*project) {
				desired[path] = struct{}{}
			}
		case focusKindFile:
			for path := range w.collectFileWatchDirs(target) {
				desired[path] = struct{}{}
			}
		}
	}

	for path := range w.dynamicWatched {
		if _, ok := desired[path]; ok {
			continue
		}
		delete(w.dynamicWatched, path)
		if _, keep := w.baseWatched[path]; keep {
			continue
		}
		if err := w.watcher.Remove(path); err != nil {
			log.Printf("Warning: could not unwatch focused path %s: %v", path, err)
		}
	}

	for path := range desired {
		if _, ok := w.dynamicWatched[path]; ok {
			continue
		}
		if _, alreadyBase := w.baseWatched[path]; !alreadyBase {
			if err := w.watcher.Add(path); err != nil {
				log.Printf("Warning: could not watch focused path %s: %v", path, err)
				continue
			}
		}
		w.dynamicWatched[path] = struct{}{}
	}
}

func (w *Watcher) hasProjectFocusLocked(projectName string) bool {
	for _, target := range w.windowFocuses {
		if target.Kind == focusKindProject && target.Project == projectName {
			return true
		}
	}
	return false
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

// E-PENPAL-WATCHER: routes fsnotify events to project/workspace refresh or SSE broadcast.
func (w *Watcher) handleEvent(event fsnotify.Event) {
	path := filepath.Clean(event.Name)

	// Check if this is a change in a workspace directory (new/removed project)
	parentDir := filepath.Clean(filepath.Dir(path))
	for _, ws := range w.workspacePaths {
		if parentDir == filepath.Clean(ws) {
			w.debounceRefresh("workspace:"+ws, func() {
				if w.discoverFn != nil {
					projects, err := w.discoverFn()
					if err == nil {
						w.cache.RescanWith(projects)
						w.focusMu.Lock()
						w.syncBaseWatchesLocked(w.workspacePaths, projects)
						w.syncDynamicWatchesLocked()
						w.focusMu.Unlock()
						w.Broadcast(Event{Type: EventProjectsChanged})
					}
				}
			})
			return
		}
	}

	// Check if a new auto-detectable source was created/removed under a
	// project root (e.g., someone creates thoughts/, .rp1/, or ANCHORS.md).
	// For Create events, stat to distinguish dirs from files.
	// For Remove/Rename the path is already gone so we can't stat it —
	// just trigger the rescan and let DetectSources sort it out.
	if event.Op&(fsnotify.Create|fsnotify.Remove|fsnotify.Rename) != 0 {
		baseName := filepath.Base(path)
		isNewDir := false
		if event.Op&fsnotify.Create != 0 {
			if info, err := os.Stat(path); err == nil {
				isNewDir = info.IsDir()
			}
		}
		for _, st := range discovery.AllSourceTypes() {
			matched := false
			if event.Op&fsnotify.Create != 0 {
				matched = (isNewDir && st.AutoDetectDir != "" && st.AutoDetectDir == baseName) ||
					(!isNewDir && st.AutoDetectFile != "" && st.AutoDetectFile == baseName)
			} else {
				// Remove/Rename: path is gone, match either mechanism
				matched = (st.AutoDetectDir != "" && st.AutoDetectDir == baseName) ||
					(st.AutoDetectFile != "" && st.AutoDetectFile == baseName)
			}
			if matched {
				for _, p := range w.cache.Projects() {
					if parentDir == filepath.Clean(p.Path) {
						w.debounceRefresh("sources:"+p.QualifiedName(), func() {
							if w.discoverFn != nil {
								projects, err := w.discoverFn()
								if err == nil {
									w.cache.RescanWith(projects)
									w.focusMu.Lock()
									w.syncBaseWatchesLocked(w.workspacePaths, projects)
									w.syncDynamicWatchesLocked()
									w.focusMu.Unlock()
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

	// Find which project this path belongs to
	projectName := w.findProjectForPath(path)
	if projectName == "" {
		return
	}

	// Keep deep watches in sync when a focused directory tree changes.
	if event.Op&(fsnotify.Create|fsnotify.Remove|fsnotify.Rename) != 0 {
		w.focusMu.Lock()
		shouldSync := false
		if event.Op&fsnotify.Create != 0 && w.hasProjectFocusLocked(projectName) {
			if info, err := os.Stat(path); err == nil && info.IsDir() {
				shouldSync = true
			}
		}
		if !shouldSync {
			_, shouldSync = w.dynamicWatched[path]
		}
		if shouldSync {
			w.syncDynamicWatchesLocked()
		}
		w.focusMu.Unlock()
	}

	// Handle changes in .penpal/comments/ directories
	if strings.Contains(path, "/.penpal/") && strings.HasSuffix(path, ".json") {
		w.debounceRefresh("comments:"+projectName, func() {
			w.Broadcast(Event{Type: EventCommentsChanged, Project: projectName})
		})
		return
	}

	// E-PENPAL-WATCHER: only .md file events trigger cache updates. Non-.md
	// Create events (e.g., scanner temp files, backup artifacts) are ignored.
	// However, directory Create/Rename events may contain .md files that need
	// to be discovered, so we walk those directories before returning.
	if !strings.HasSuffix(path, ".md") {
		if event.Op&(fsnotify.Create|fsnotify.Rename) != 0 {
			if info, err := os.Stat(path); err == nil && info.IsDir() {
				filepath.Walk(path, func(p string, fi os.FileInfo, err error) error {
					if err != nil {
						return nil
					}
					if !fi.IsDir() && strings.HasSuffix(p, ".md") {
						w.debounceFileEvent(projectName, p, fsnotify.Create)
					}
					return nil
				})
			}
		}
		return
	}

	// Record activity for .md file changes before debouncing
	if w.activity != nil {
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

	// E-PENPAL-WATCHER: accumulate file event for incremental cache update.
	w.debounceFileEvent(projectName, path, event.Op)
}

// findProjectForPath finds which project a path belongs to by checking
// all source roots, individual file paths, .penpal directories, and worktree directories.
// Returns the qualified name.
func (w *Watcher) findProjectForPath(path string) string {
	for _, p := range w.cache.Projects() {
		// Check all source roots
		for _, src := range p.Sources {
			if src.RootPath != "" && strings.HasPrefix(path, src.RootPath+"/") {
				return p.QualifiedName()
			}
			// Check individual file paths for "files" sources
			for _, f := range src.Files {
				if path == f {
					return p.QualifiedName()
				}
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
// E-PENPAL-WATCHER: 100ms debounce coalescing rapid filesystem events.
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

// debounceFileEvent accumulates a file event for a project and schedules
// an incremental cache update after the debounce window (100ms).
// E-PENPAL-WATCHER: accumulating debounce for incremental file updates.
func (w *Watcher) debounceFileEvent(projectName, absPath string, op fsnotify.Op) {
	w.filePendingMu.Lock()
	defer w.filePendingMu.Unlock()

	if w.filePending[projectName] == nil {
		w.filePending[projectName] = make(map[string]fsnotify.Op)
	}
	w.filePending[projectName][absPath] |= op

	if timer, ok := w.fileTimers[projectName]; ok {
		timer.Stop()
	}

	w.fileTimers[projectName] = time.AfterFunc(100*time.Millisecond, func() {
		w.flushFileEvents(projectName)
	})
}

// flushFileEvents drains accumulated file events for a project and applies
// incremental cache mutations. Falls back to a full RefreshProject if the
// project hasn't been scanned yet.
// E-PENPAL-WATCHER: incremental cache update from accumulated file events.
func (w *Watcher) flushFileEvents(projectName string) {
	w.filePendingMu.Lock()
	events := w.filePending[projectName]
	delete(w.filePending, projectName)
	delete(w.fileTimers, projectName)
	w.filePendingMu.Unlock()

	if len(events) == 0 {
		return
	}

	project := w.cache.FindProject(projectName)
	if project == nil {
		return
	}

	// Apply incremental updates regardless of scan state. For unscanned
	// projects this adds files to a partial cache; the lazy scan on first
	// access will reconcile with a full walk.
	for absPath, op := range events {
		if op&(fsnotify.Remove|fsnotify.Rename) != 0 {
			relPath, err := filepath.Rel(project.Path, absPath)
			if err == nil {
				w.cache.RemoveFile(projectName, relPath)
			}
		}
		if op&(fsnotify.Create|fsnotify.Write) != 0 {
			w.cache.UpsertFile(projectName, project, absPath)
		}
	}

	w.cache.RefreshProjectGitInfo(projectName)
	w.Broadcast(Event{Type: EventFilesChanged, Project: projectName})
}
