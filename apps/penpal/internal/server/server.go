package server

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/loganj/penpal/internal/activity"
	"github.com/loganj/penpal/internal/agents"
	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
	"github.com/loganj/penpal/internal/config"
	"github.com/loganj/penpal/internal/discovery"
	"github.com/loganj/penpal/internal/watcher"
)

type Server struct {
	cache       *cache.Cache
	watcher     *watcher.Watcher
	comments    *comments.Store
	agents      *agents.Manager
	activity    *activity.Tracker
	mcpHandler  http.Handler
	mux         *http.ServeMux
	loadOnce    sync.Once
	readyCh     chan struct{} // closed when initial population is complete
	readyOnce   sync.Once
	frontendDir string // if set, serve React SPA from this directory at /app/
	cfg         *config.Config
	cfgPath     string
	cfgMu       sync.Mutex // protects cfg mutations

	pendingNav   string    // URL path set by /api/open, consumed by /api/navigate
	pendingNavAt time.Time // when it was set
	navMu        sync.Mutex

	installCfg *installConfig // injectable for testing; nil uses defaults
}

func New(c *cache.Cache, w *watcher.Watcher, cs *comments.Store, mcpHandler http.Handler, am *agents.Manager, act *activity.Tracker, cfg *config.Config, cfgPath string) *Server {
	// Auto-detect frontend/dist directory for SPA serving
	var frontendDir string
	if dir := "frontend/dist"; dirExists(dir) {
		frontendDir = dir
	}

	s := &Server{
		cache:       c,
		watcher:     w,
		comments:    cs,
		agents:      am,
		activity:    act,
		mcpHandler:  mcpHandler,
		mux:         http.NewServeMux(),
		readyCh:     make(chan struct{}),
		frontendDir: frontendDir,
		cfg:         cfg,
		cfgPath:     cfgPath,
	}

	if am != nil {
		am.SetOnChange(func(projectName string) {
			s.watcher.Broadcast(watcher.Event{Type: watcher.EventAgentsChanged, Project: projectName})
		})
		am.SetClaudeBin(s.resolveClaudePath)
	}

	cs.SetOnWorking(func(project string) {
		w.Broadcast(watcher.Event{Type: watcher.EventCommentsChanged, Project: project})
	})

	s.routes()
	return s
}

// E-PENPAL-CORS: CORS allows only specific origins; E-PENPAL-LAZY-INIT: first request triggers discovery.
func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	s.ensureLoaded()
	// Allow cross-origin requests from Tauri desktop app (tauri://localhost)
	// and local development servers.
	origin := r.Header.Get("Origin")
	if origin != "" && isLocalOrigin(origin) {
		w.Header().Set("Access-Control-Allow-Origin", origin)
		w.Header().Set("Access-Control-Allow-Methods", "GET, POST, PATCH, DELETE, OPTIONS")
		w.Header().Set("Access-Control-Allow-Headers", "Content-Type")
		if r.Method == http.MethodOptions {
			w.WriteHeader(http.StatusNoContent)
			return
		}
	}
	s.mux.ServeHTTP(w, r)
}

// isLocalOrigin returns true for origins that should be allowed CORS access.
// E-PENPAL-CORS: allows tauri://, https://tauri.*, http://localhost*, http://127.0.0.1*.
func isLocalOrigin(origin string) bool {
	return strings.HasPrefix(origin, "tauri://") ||
		strings.HasPrefix(origin, "https://tauri.") ||
		strings.HasPrefix(origin, "http://localhost") ||
		strings.HasPrefix(origin, "http://127.0.0.1")
}

// discoverAllProjects discovers projects from all configured workspaces and standalone projects.
func (s *Server) discoverAllProjects() []discovery.Project {
	var allProjects []discovery.Project
	for _, ws := range s.cfg.Workspaces {
		projects, err := discovery.DiscoverWorkspace(ws.Path, ws.DisplayName())
		if err != nil {
			log.Printf("Warning: could not discover workspace %s: %v", ws.Path, err)
			continue
		}
		// Merge any configured sources for workspace projects
		for i := range projects {
			if extras, ok := s.cfg.ProjectSources[projects[i].Path]; ok {
				projects[i].Sources = append(projects[i].Sources, discovery.SourceConfigsToFileSources(projects[i].Path, extras)...)
			}
		}
		allProjects = append(allProjects, projects...)
	}
	// Standalone projects are intentionally not subject to worktree de-duping
	// (which runs inside DiscoverWorkspace). If a user explicitly adds a project,
	// it should always appear regardless of worktree relationships.
	for _, pc := range s.cfg.Projects {
		p, err := discovery.LoadStandaloneProject(pc.Path, pc)
		if err != nil {
			log.Printf("Warning: could not load standalone project %s: %v", pc.Path, err)
			continue
		}
		allProjects = append(allProjects, p)
	}

	// Auto-include ~/.claude/plans/ if it has markdown files.
	// If a project for that path already exists (user added it manually), inject the
	// tree source so all files show — not just any individually-added ones.
	if claudePlans, ok := discovery.DiscoverClaudePlans(); ok {
		existingIdx := -1
		for i, p := range allProjects {
			if filepath.Clean(p.Path) == filepath.Clean(claudePlans.Path) {
				existingIdx = i
				break
			}
		}
		if existingIdx == -1 {
			allProjects = append(allProjects, claudePlans)
		} else {
			// Project exists but may lack a tree source covering the directory.
			hasTreeSource := false
			for _, src := range allProjects[existingIdx].Sources {
				if src.Type == "tree" && filepath.Clean(src.RootPath) == filepath.Clean(claudePlans.Path) {
					hasTreeSource = true
					break
				}
			}
			if !hasTreeSource {
				allProjects[existingIdx].Sources = append(allProjects[existingIdx].Sources, claudePlans.Sources...)
			}
		}
	}

	return allProjects
}

// workspacePaths extracts workspace directory paths from the config.
func (s *Server) workspacePaths() []string {
	paths := make([]string, len(s.cfg.Workspaces))
	for i, ws := range s.cfg.Workspaces {
		paths[i] = ws.Path
	}
	return paths
}

// ensureLoaded does fast project discovery on first request, then populates in background.
// E-PENPAL-LAZY-INIT: first HTTP request triggers sync.Once discovery.
func (s *Server) ensureLoaded() {
	s.loadOnce.Do(func() {
		projects := s.discoverAllProjects()
		log.Printf("Found %d projects across %d workspace(s)", len(projects), len(s.cfg.Workspaces))

		s.cache.SetProjects(projects)

		wsPaths := s.workspacePaths()
		discoverFn := func() ([]discovery.Project, error) {
			return s.discoverAllProjects(), nil
		}
		if err := s.watcher.Start(wsPaths, discoverFn); err != nil {
			log.Printf("Warning: file watcher failed to start: %v", err)
		}

		// Populate file lists and enrichment in background so the first
		// request isn't blocked. Pages render immediately with whatever
		// data is available; SSE pushes an update when population completes.
		go s.populateProjects()
	})
}

// handleReady blocks until the server's initial population is complete.
// Tauri polls this endpoint before showing the webview.
// E-PENPAL-LAZY-INIT: GET /api/ready blocks until populateProjects completes.
func (s *Server) handleReady(w http.ResponseWriter, _ *http.Request) {
	<-s.readyCh
	w.Header().Set("Content-Type", "application/json")
	w.Write([]byte(`{"ready":true}`))
}

// populateProjects scans file lists and fills in git info in the background.
// E-PENPAL-GIT-ENRICH: background git enrichment with SSE push.
func (s *Server) populateProjects() {
	s.cache.RefreshAllProjects()
	s.seedRecentActivity()
	log.Printf("Background file scan complete")
	s.readyOnce.Do(func() { close(s.readyCh) }) // signal that essential data is ready
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventProjectsChanged})

	// Notify any open project pages that their file lists are now ready.
	// Without this, a page loaded before the scan completes would show
	// "No files yet." indefinitely since EventProjectsChanged only triggers
	// a project-existence check, not a file list refresh.
	// A single broadcast with no project field signals all project pages to
	// refresh; this avoids flooding subscriber channels (buffered to 10) when
	// there are many projects.
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventFilesChanged})

	// Now enrich with git info
	projects := s.cache.Projects()
	var wg sync.WaitGroup
	sem := make(chan struct{}, 8) // bound concurrency to avoid fork-bombing

	for _, p := range projects {
		wg.Add(1)
		go func(p discovery.Project) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()

			if p.Name != "(root)" {
				git := discovery.GetGitInfo(p.Path)
				s.cache.EnrichProject(p.QualifiedName(), git)
			}
		}(p)
	}

	wg.Wait()
	log.Printf("Background enrichment complete (git info)")
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventProjectsChanged})
}

// seedRecentActivity populates the activity tracker with ModTimes from the
// most recently modified files in the cache. This ensures /recent has data
// immediately on startup, even for files modified before the server started.
// E-PENPAL-ACTIVITY: seeds file modification events at startup.
func (s *Server) seedRecentActivity() {
	files := s.cache.AllFiles(50)
	for _, f := range files {
		s.activity.RecordAt(activity.FileModified, f.Project, f.FullPath, f.ModTime)
	}
}

// E-PENPAL-API-ROUTES: registers all REST endpoints, SPA, SSE, and MCP.
func (s *Server) routes() {
	// Main mux (:8080) — API, SSE, MCP, and React SPA
	s.mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/" {
			http.NotFound(w, r)
			return
		}
		http.Redirect(w, r, "/app/", http.StatusFound)
	})
	s.mux.HandleFunc("/events", s.handleEvents)
	// API endpoints for dynamic updates
	s.mux.HandleFunc("/api/projects", s.handleAPIProjects)
	s.mux.HandleFunc("/api/project/", s.handleAPIProjectFiles)
	s.mux.HandleFunc("/api/recent", s.handleAPIRecent)
	s.mux.HandleFunc("/api/in-review", s.handleAPIInReview)
	s.mux.HandleFunc("/api/search", s.handleAPISearch)
	s.mux.HandleFunc("/api/copy-file", s.handleCopyFile)
	s.mux.HandleFunc("/api/project-info", s.handleProjectInfo)
	s.mux.HandleFunc("/api/delete-project", s.handleDeleteProject)
	s.mux.HandleFunc("/api/delete-file", s.handleDeleteFile)
	// Workspace and project management
	s.mux.HandleFunc("/api/workspaces", s.handleAPIWorkspaces)
	s.mux.HandleFunc("/api/sources", s.handleAPISources)
	s.mux.HandleFunc("/api/open", s.handleAPIOpen)
	s.mux.HandleFunc("/api/navigate", s.handleAPINavigate)
	// Comment and review API endpoints
	s.mux.HandleFunc("/api/threads", s.handleAPIThreads)
	s.mux.HandleFunc("/api/threads/", s.handleAPIThreadAction)
	s.mux.HandleFunc("/api/reviews", s.handleAPIListReviews)
	// Agent management endpoints
	s.mux.HandleFunc("/api/focus", s.handleFocus)
	s.mux.HandleFunc("/api/agents", s.handleAgentStatus)
	s.mux.HandleFunc("/api/agents/start", s.handleAgentStart)
	s.mux.HandleFunc("/api/agents/stop", s.handleAgentStop)
	// Raw file content
	s.mux.HandleFunc("/api/raw", s.handleRawFile)
	// Activity tracking
	s.mux.HandleFunc("/api/view", s.handleRecordView)
	// Publish to Blockcell
	s.mux.HandleFunc("/api/publish", s.handlePublish)
	s.mux.HandleFunc("/api/publish-state", s.handlePublishState)
	// Readiness check — blocks until initial population is complete
	s.mux.HandleFunc("/api/ready", s.handleReady)
	// Install tools (CLI symlink + Claude Code plugin)
	s.mux.HandleFunc("/api/install-tools", s.handleInstallTools)
	s.mux.HandleFunc("/api/claude-path", s.handleClaudePath)
	// React SPA at /app/ (served from frontend/dist/ when it exists)
	s.mux.Handle("/app/", newSPAHandler(s.frontendDir, "/app"))
	s.mux.Handle("/app", http.RedirectHandler("/app/", http.StatusMovedPermanently))
	// MCP (Model Context Protocol) endpoint
	if s.mcpHandler != nil {
		s.mux.Handle("/mcp", s.mcpHandler)
		s.mux.Handle("/mcp/", s.mcpHandler)
	}

}

// handleFocus tells the watcher what to deep-watch based on the active tab in
// a given window.
//
//	POST /api/focus?window=W&project=X            — watch project sources (ProjectPage)
//	POST /api/focus?window=W&project=X&path=Y     — watch file directory only (FilePage)
//	DELETE /api/focus?window=W                     — clear deep watches for one window
//	DELETE /api/focus                              — clear all deep watches (legacy)
//
// E-PENPAL-FOCUS: windowFocuses map drives dynamic watches.
func (s *Server) handleFocus(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodPost:
		windowID := r.URL.Query().Get("window")
		project := r.URL.Query().Get("project")
		if project == "" {
			http.Error(w, "missing project parameter", http.StatusBadRequest)
			return
		}
		filePath := r.URL.Query().Get("path")
		worktree := r.URL.Query().Get("worktree")
		if filePath != "" {
			if windowID != "" {
				s.watcher.SetWindowFocusFile(windowID, project, filePath, worktree)
			} else {
				s.watcher.FocusFile(project, filePath, worktree)
			}
		} else {
			if windowID != "" {
				s.watcher.SetWindowFocusProject(windowID, project)
			} else {
				s.watcher.FocusProject(project)
			}
		}
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"ok":true}`))
	case http.MethodDelete:
		windowID := r.URL.Query().Get("window")
		if windowID != "" {
			s.watcher.ClearWindowFocus(windowID)
		} else {
			s.watcher.ClearFocus()
		}
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"ok":true}`))
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func formatAge(t time.Time) string {
	d := time.Since(t)
	switch {
	case d < time.Hour:
		mins := int(d.Minutes())
		if mins <= 1 {
			return "just now"
		}
		return fmt.Sprintf("%dm ago", mins)
	case d < 24*time.Hour:
		return fmt.Sprintf("%dh ago", int(d.Hours()))
	case d < 48*time.Hour:
		return "yesterday"
	case d < 7*24*time.Hour:
		return fmt.Sprintf("%dd ago", int(d.Hours()/24))
	default:
		return t.Format("Jan 2")
	}
}

func formatAgeMarker(t time.Time) string {
	if t.IsZero() {
		return ""
	}
	d := time.Since(t)
	switch {
	case d < time.Hour:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	case d < 24*time.Hour:
		return fmt.Sprintf("%dh", int(d.Hours()))
	default:
		return fmt.Sprintf("%dd", int(d.Hours()/24))
	}
}

// computeProjectAge returns an age marker string for the project.
// Age is the minimum of: most recent unstaged change, most recent thoughts
// file, and the project directory mod time. For projects older than 24h,
// unpushed commit age is also considered at 24h granularity.
func computeProjectAge(p discovery.Project) string {
	best := time.Time{}

	if p.LastModified.After(best) {
		best = p.LastModified
	}
	if p.Git != nil && p.Git.UnstagedModTime.After(best) {
		best = p.Git.UnstagedModTime
	}
	if info, err := os.Stat(p.Path); err == nil && info.ModTime().After(best) {
		best = info.ModTime()
	}

	// For projects older than 24h, also consider unpushed commits at day granularity
	if !best.IsZero() && time.Since(best) > 24*time.Hour {
		if p.Git != nil && !p.Git.UnpushedCommitTime.IsZero() {
			daysSince := int(time.Since(p.Git.UnpushedCommitTime).Hours()) / 24
			quantized := time.Now().Add(-time.Duration(daysSince) * 24 * time.Hour)
			if quantized.After(best) {
				best = quantized
			}
		}
	}

	return formatAgeMarker(best)
}

type ProjectFile struct {
	Name       string
	Title      string // H1 heading from markdown file
	Path       string
	Dir        string // source-relative directory (empty for root files)
	ShowDir    bool   // true on first file of each new directory
	Source     string // source name (e.g., "thoughts", "docs")
	SourceType string // "thoughts", "tree", or "files"
	ModTime    time.Time
	Age        string
	FileType   string // "research", "plan", or "other"
}

// FileGroupView is the top-level display unit in the project file list.
// Each group is rendered as its own section with a header and file list.
type FileGroupView struct {
	Name       string // display name ("thoughts", "Context", "auth-feature", etc.)
	Source     string // source name for management operations (e.g., "rp1")
	SourceType string // "tree" or "files"
	Auto       bool
	BadgeText  string // source type badge text (e.g., "RPI", "RP1")
	BadgeColor string // CSS color for badge text
	BadgeBg    string // CSS background for badge
	Files      []ProjectFile
}

// sortAndMarkDirs sorts files by directory then name, and sets ShowDir=true
// on the first file of each non-empty directory.
func sortAndMarkDirs(files []ProjectFile) {
	sort.Slice(files, func(i, j int) bool {
		if files[i].Dir != files[j].Dir {
			// Empty dir (root files) sorts first
			if files[i].Dir == "" {
				return true
			}
			if files[j].Dir == "" {
				return false
			}
			return files[i].Dir < files[j].Dir
		}
		return files[i].Name < files[j].Name
	})
	prevDir := ""
	for i := range files {
		if files[i].Dir != "" && files[i].Dir != prevDir {
			files[i].ShowDir = true
		}
		prevDir = files[i].Dir
	}
}

// buildFileGroups produces a flat list of display groups for a project.
// Sources with GroupFiles produce one group per group; sources without
// produce a single group named after the source.
func buildFileGroups(project *discovery.Project, cachedFiles []cache.FileInfo) []FileGroupView {
	// Index files by source
	filesBySource := make(map[string][]cache.FileInfo)
	for _, f := range cachedFiles {
		filesBySource[f.Source] = append(filesBySource[f.Source], f)
	}

	var groups []FileGroupView
	seen := make(map[string]bool)

	for _, src := range project.Sources {
		if seen[src.Name] {
			continue
		}
		seen[src.Name] = true

		srcFiles := filesBySource[src.Name]
		if len(srcFiles) == 0 {
			continue
		}

		st := discovery.GetSourceType(src.SourceTypeName)

		// Badge info from registered source type
		var badgeText, badgeColor, badgeBg string
		if st != nil {
			badgeText = st.DisplayName
			badgeColor = st.BadgeColor
			badgeBg = st.BadgeBg
		}

		if st != nil && st.GroupFiles != nil {
			// Build path lookup and get source-relative paths
			paths := make([]string, len(srcFiles))
			fileByPath := make(map[string]cache.FileInfo)
			for i, f := range srcFiles {
				paths[i] = f.Path
				fileByPath[f.Path] = f
			}

			for _, g := range st.GroupFiles(paths) {
				gv := FileGroupView{
					Name:       g.Name,
					Source:     src.Name,
					SourceType: src.Type,
					Auto:       src.Auto,
					BadgeText:  badgeText,
					BadgeColor: badgeColor,
					BadgeBg:    badgeBg,
				}
				for _, p := range g.Paths {
					if f, ok := fileByPath[p]; ok {
						gv.Files = append(gv.Files, ProjectFile{
							Name:       f.Name,
							Title:      f.Title,
							Path:       f.FullPath,
							Source:     f.Source,
							SourceType: f.SourceType,
							ModTime:    f.ModTime,
							Age:        formatAge(f.ModTime),
							FileType:   f.FileType,
						})
					}
				}
				groups = append(groups, gv)
			}
		} else {
			// No grouping — single group named after the source
			gv := FileGroupView{
				Name:       src.Name,
				Source:     src.Name,
				SourceType: src.Type,
				Auto:       src.Auto,
				BadgeText:  badgeText,
				BadgeColor: badgeColor,
				BadgeBg:    badgeBg,
			}
			for _, f := range srcFiles {
				pf := ProjectFile{
					Name:       f.Name,
					Title:      f.Title,
					Path:       f.FullPath,
					Source:     f.Source,
					SourceType: f.SourceType,
					ModTime:    f.ModTime,
					Age:        formatAge(f.ModTime),
					FileType:   f.FileType,
				}
				if st != nil && st.ShowDirHeadings {
					pf.Dir = filepath.Dir(f.Path)
					if pf.Dir == "." {
						pf.Dir = ""
					}
				}
				gv.Files = append(gv.Files, pf)
			}
			if st != nil && st.ShowDirHeadings {
				sortAndMarkDirs(gv.Files)
			}
			groups = append(groups, gv)
		}
	}

	// Also include any sources that appear in files but not in project.Sources
	for _, f := range cachedFiles {
		if seen[f.Source] {
			continue
		}
		seen[f.Source] = true
		srcFiles := filesBySource[f.Source]
		gv := FileGroupView{
			Name:       f.Source,
			Source:     f.Source,
			SourceType: f.SourceType,
		}
		if st := discovery.GetSourceType(f.Source); st != nil {
			gv.BadgeText = st.DisplayName
			gv.BadgeColor = st.BadgeColor
			gv.BadgeBg = st.BadgeBg
		}
		for _, sf := range srcFiles {
			gv.Files = append(gv.Files, ProjectFile{
				Name:       sf.Name,
				Title:      sf.Title,
				Path:       sf.FullPath,
				Source:     sf.Source,
				SourceType: sf.SourceType,
				ModTime:    sf.ModTime,
				Age:        formatAge(sf.ModTime),
				FileType:   sf.FileType,
			})
		}
		groups = append(groups, gv)
	}

	return groups
}

// handleEvents is the SSE endpoint for live updates.
// E-PENPAL-SSE: GET /events long-lived SSE stream with event types.
func (s *Server) handleEvents(w http.ResponseWriter, r *http.Request) {
	// Set headers for SSE
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")
	w.Header().Set("Access-Control-Allow-Origin", "*")

	flusher, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "SSE not supported", http.StatusInternalServerError)
		return
	}

	// Subscribe to events
	events := s.watcher.Subscribe()
	defer s.watcher.Unsubscribe(events)

	// Send initial connected event
	fmt.Fprintf(w, "event: connected\ndata: {}\n\n")
	flusher.Flush()

	// Listen for events or client disconnect
	for {
		select {
		case <-r.Context().Done():
			return
		case evt, ok := <-events:
			if !ok {
				return
			}
			data, _ := json.Marshal(evt)
			if _, err := fmt.Fprintf(w, "event: change\ndata: %s\n\n", data); err != nil {
				return
			}
			flusher.Flush()
		}
	}
}

// API handlers for dynamic updates

type APIBadge struct {
	Text  string `json:"text"`
	Color string `json:"color"`
	Bg    string `json:"bg"`
}

type APIWorktree struct {
	Name   string `json:"name"`
	Path   string `json:"path"`
	Branch string `json:"branch"`
	IsMain bool   `json:"isMain"`
}

type APIProject struct {
	Name           string        `json:"name"`
	QualifiedName  string        `json:"qualifiedName"`
	Workspace      string        `json:"workspace"`
	WorkspacePath  string        `json:"workspacePath,omitempty"`
	ProjectPath    string        `json:"projectPath"`
	Origin         string        `json:"origin"`
	Badges         []APIBadge    `json:"badges"`
	Branch         string        `json:"branch,omitempty"`
	Dirty          bool          `json:"dirty,omitempty"`
	FileCount      int           `json:"fileCount"`
	LastModified   string        `json:"lastModified"`
	AgentConnected bool          `json:"agentConnected,omitempty"`
	AgentRunning   bool          `json:"agentRunning,omitempty"`
	Age            string        `json:"age,omitempty"`
	ReviewCount    int           `json:"reviewCount,omitempty"`
	Worktrees      []APIWorktree `json:"worktrees,omitempty"`
}

func (s *Server) handleAPIProjects(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		s.handleListAPIProjects(w, r)
	case http.MethodPost:
		s.handleAddStandaloneProject(w, r)
	case http.MethodDelete:
		s.handleCloseStandaloneProject(w, r)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

// E-PENPAL-API-ROUTES: GET /api/projects endpoint.
func (s *Server) handleListAPIProjects(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")

	projects := s.cache.ProjectsSortedByModTime()
	result := make([]APIProject, len(projects))
	for i, p := range projects {
		qn := p.QualifiedName()
		apiBadges := make([]APIBadge, 0)
		for _, b := range p.Badges() {
			apiBadges = append(apiBadges, APIBadge{Text: b.Text, Color: b.Color, Bg: b.Bg})
		}
		result[i] = APIProject{
			Name:           p.Name,
			QualifiedName:  qn,
			Workspace:      p.WorkspaceName,
			WorkspacePath:  p.WorkspacePath,
			ProjectPath:    p.Path,
			Origin:         p.Origin,
			Badges:         apiBadges,
			FileCount:      p.FileCount,
			LastModified:   p.LastModified.Format(time.RFC3339),
			Age:            computeProjectAge(p),
			AgentConnected: s.agents != nil && s.agents.Status(qn) != nil && s.agents.Status(qn).Running,
			AgentRunning:   s.agents != nil && s.agents.Status(qn) != nil && s.agents.Status(qn).Running,
		}
		if p.Git != nil {
			result[i].Branch = p.Git.Branch
			result[i].Dirty = p.Git.Dirty
		}
		// Include worktrees
		if len(p.Worktrees) > 0 {
			apiWTs := make([]APIWorktree, len(p.Worktrees))
			for j, wt := range p.Worktrees {
				apiWTs[j] = APIWorktree{
					Name:   wt.Name,
					Path:   wt.Path,
					Branch: wt.Branch,
					IsMain: wt.IsMain,
				}
			}
			result[i].Worktrees = apiWTs
		}
		// Count files in review for this project
		if reviews, err := s.comments.ListFilesInReview(qn); err == nil {
			result[i].ReviewCount = len(reviews)
		}
	}
	json.NewEncoder(w).Encode(result)
}

type APIFile struct {
	Name         string `json:"name"`
	Title        string `json:"title,omitempty"`
	Path         string `json:"path"`
	Dir          string `json:"dir,omitempty"`
	Source       string `json:"source,omitempty"`
	SourceType   string `json:"sourceType,omitempty"`
	SourceAuto   bool   `json:"sourceAuto,omitempty"`
	Project      string `json:"project,omitempty"`
	Workspace    string `json:"workspace,omitempty"`
	Age          string `json:"age"`
	FileType     string `json:"fileType,omitempty"`
	ActivityType string `json:"activityType,omitempty"`
	ActivityAge  string `json:"activityAge,omitempty"`
}

// APIFileGroupView is the top-level display unit returned by the project files API.
type APIFileGroupView struct {
	Name       string    `json:"name"`
	Source     string    `json:"source"`
	SourceType string    `json:"sourceType"`
	Auto       bool      `json:"auto"`
	BadgeText  string    `json:"badgeText,omitempty"`
	BadgeColor string    `json:"badgeColor,omitempty"`
	BadgeBg    string    `json:"badgeBg,omitempty"`
	Files      []APIFile `json:"files"`
}

// E-PENPAL-API-ROUTES: GET /api/project/{qualifiedName} returns grouped file list.
func (s *Server) handleAPIProjectFiles(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")

	// Parse /api/project/{qualifiedName}
	qualifiedName := strings.TrimPrefix(r.URL.Path, "/api/project/")
	qualifiedName = strings.TrimSuffix(qualifiedName, "/")
	if qualifiedName == "" {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}

	project := s.cache.FindProject(qualifiedName)
	if project == nil {
		json.NewEncoder(w).Encode([]APIFileGroupView{})
		return
	}

	worktree := r.URL.Query().Get("worktree")

	var cachedFiles []cache.FileInfo
	if worktree != "" {
		wtPath := s.cache.WorktreePath(qualifiedName, worktree)
		if wtPath == "" {
			json.NewEncoder(w).Encode([]APIFileGroupView{})
			return
		}
		cachedFiles = cache.ScanProjectSourcesForWorktree(project, wtPath)
	} else {
		// Ensure file titles are populated before serving
		s.cache.EnrichTitles(qualifiedName)
		cachedFiles = s.cache.ProjectFiles(qualifiedName)
	}
	fileGroups := buildFileGroups(project, cachedFiles)

	result := make([]APIFileGroupView, 0, len(fileGroups))
	for _, g := range fileGroups {
		apiGroup := APIFileGroupView{
			Name:       g.Name,
			Source:     g.Source,
			SourceType: g.SourceType,
			Auto:       g.Auto,
			BadgeText:  g.BadgeText,
			BadgeColor: g.BadgeColor,
			BadgeBg:    g.BadgeBg,
			Files:      make([]APIFile, 0, len(g.Files)),
		}
		for _, f := range g.Files {
			apiGroup.Files = append(apiGroup.Files, APIFile{
				Name:       f.Name,
				Title:      f.Title,
				Path:       f.Path,
				Dir:        f.Dir,
				Source:     f.Source,
				SourceType: f.SourceType,
				SourceAuto: g.Auto,
				Age:        f.Age,
				FileType:   f.FileType,
			})
		}
		result = append(result, apiGroup)
	}
	json.NewEncoder(w).Encode(result)
}

// E-PENPAL-API-ROUTES: GET /api/recent returns recent files.
func (s *Server) handleAPIRecent(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")

	files := s.mergeRecentFiles(50)
	result := make([]APIFile, len(files))
	for i, f := range files {
		result[i] = APIFile{
			Name:         f.DisplayName,
			Title:        f.Title,
			Path:         f.FilePath,
			Project:      f.Project,
			Age:          f.Age,
			FileType:     f.FileType,
			ActivityType: f.ActivityType,
			ActivityAge:  f.ActivityAge,
		}
	}
	json.NewEncoder(w).Encode(result)
}

func (s *Server) handleCopyFile(w http.ResponseWriter, r *http.Request) {
	qualifiedName := r.URL.Query().Get("project")
	filePath := r.URL.Query().Get("path")
	if qualifiedName == "" || filePath == "" {
		http.Error(w, "missing project or path", http.StatusBadRequest)
		return
	}

	project := s.cache.FindProject(qualifiedName)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	fullPath := filepath.Join(project.Path, filePath)
	resolved, err := filepath.Abs(fullPath)
	if err != nil || !isSubpath(project.Path, resolved) {
		http.Error(w, "invalid path", http.StatusBadRequest)
		return
	}

	// Verify the file exists
	if _, err := os.Stat(resolved); err != nil {
		http.Error(w, "file not found", http.StatusNotFound)
		return
	}

	cmd := exec.Command("osascript", "-e", fmt.Sprintf(`set the clipboard to (POSIX file %q)`, resolved))
	if out, err := cmd.CombinedOutput(); err != nil {
		log.Printf("osascript error: %v, output: %s", err, out)
		http.Error(w, fmt.Sprintf("failed to copy file: %v", err), http.StatusInternalServerError)
		return
	}

	w.WriteHeader(http.StatusNoContent)
}

type ProjectInfo struct {
	FileCount       int  `json:"fileCount"`
	Dirty           bool `json:"dirty"`
	UnpushedCommits int  `json:"unpushedCommits"`
}

// E-PENPAL-DELETE-PROJECT: GET /api/project-info returns fileCount, dirty, unpushedCommits.
func (s *Server) handleProjectInfo(w http.ResponseWriter, r *http.Request) {
	qualifiedName := r.URL.Query().Get("name")
	if qualifiedName == "" {
		http.Error(w, "missing project name", http.StatusBadRequest)
		return
	}
	project := s.cache.FindProject(qualifiedName)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	info := ProjectInfo{
		FileCount: len(s.cache.ProjectFiles(qualifiedName)),
	}

	// Fresh git status
	if project.Name != "(root)" {
		cmd := exec.Command("git", "-C", project.Path, "status", "--porcelain")
		if out, err := cmd.Output(); err == nil {
			info.Dirty = len(out) > 0
		}
		cmd2 := exec.Command("git", "-C", project.Path, "rev-list", "@{upstream}..HEAD", "--count")
		if out, err := cmd2.Output(); err == nil {
			if n, err := strconv.Atoi(strings.TrimSpace(string(out))); err == nil {
				info.UnpushedCommits = n
			}
		}
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(info)
}

// E-PENPAL-DELETE-PROJECT: POST /api/delete-project with os.RemoveAll.
func (s *Server) handleDeleteProject(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	qualifiedName := r.URL.Query().Get("name")
	if qualifiedName == "" {
		http.Error(w, "missing project name", http.StatusBadRequest)
		return
	}

	project := s.cache.FindProject(qualifiedName)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}
	if project.Name == "(root)" {
		http.Error(w, "cannot delete root project", http.StatusForbidden)
		return
	}

	if err := os.RemoveAll(project.Path); err != nil {
		log.Printf("Failed to delete project %s: %v", qualifiedName, err)
		http.Error(w, fmt.Sprintf("failed to delete: %v", err), http.StatusInternalServerError)
		return
	}

	log.Printf("Deleted project directory: %s", project.Path)

	// Remove project from config
	s.cfgMu.Lock()
	if project.Origin == "standalone" {
		var filtered []config.ProjectConfig
		for _, pc := range s.cfg.Projects {
			if filepath.Clean(pc.Path) != filepath.Clean(project.Path) {
				filtered = append(filtered, pc)
			}
		}
		s.cfg.Projects = filtered
	}
	// Also clean up any source overrides for this project
	delete(s.cfg.ProjectSources, project.Path)
	if err := config.Save(s.cfgPath, s.cfg); err != nil {
		log.Printf("Warning: could not save config after project delete: %v", err)
	}
	s.cfgMu.Unlock()

	s.cache.RemoveProject(qualifiedName)
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventProjectsChanged})
	w.WriteHeader(http.StatusNoContent)
}

// E-PENPAL-DELETE-FILE: POST /api/delete-file with os.Remove, sidecar cleanup, removeEmptyParents.
func (s *Server) handleDeleteFile(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	qualifiedName := r.URL.Query().Get("project")
	filePath := r.URL.Query().Get("path")
	if qualifiedName == "" || filePath == "" {
		http.Error(w, "missing project or path", http.StatusBadRequest)
		return
	}

	project := s.cache.FindProject(qualifiedName)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	fullPath := filepath.Join(project.Path, filePath)
	resolved, err := filepath.Abs(fullPath)
	if err != nil || !isSubpath(project.Path, resolved) {
		http.Error(w, "invalid path", http.StatusBadRequest)
		return
	}
	if _, err := os.Stat(resolved); err != nil {
		http.Error(w, "file not found", http.StatusNotFound)
		return
	}

	if err := os.Remove(resolved); err != nil {
		log.Printf("Failed to delete file %s: %v", fullPath, err)
		http.Error(w, fmt.Sprintf("failed to delete: %v", err), http.StatusInternalServerError)
		return
	}

	log.Printf("Deleted file: %s", fullPath)

	// Clean up empty parent directories, stopping at the project root
	removeEmptyParents(filepath.Dir(fullPath), project.Path)

	// Clean up the comment sidecar file if it exists
	sidecarPath := filepath.Join(project.Path, ".penpal", "comments", filePath+".json")
	if err := os.Remove(sidecarPath); err != nil && !os.IsNotExist(err) {
		log.Printf("Warning: could not remove comment sidecar %s: %v", sidecarPath, err)
	}
	commentsDir := filepath.Join(project.Path, ".penpal", "comments")
	removeEmptyParents(filepath.Dir(sidecarPath), commentsDir)

	// If this was an individually-added file, also remove from config
	s.cfgMu.Lock()
	s.removeFileFromConfig(project, filePath)
	if err := config.Save(s.cfgPath, s.cfg); err != nil {
		log.Printf("Warning: could not save config after file delete: %v", err)
	}
	s.cfgMu.Unlock()

	// Refresh cache so the file disappears from listings
	s.cache.RefreshProject(qualifiedName)
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventFilesChanged, Project: qualifiedName})
	w.WriteHeader(http.StatusNoContent)
}

// removeEmptyParents removes empty directories from dir up to (but not
// including) stopAt. This is used after file deletion to clean up directories
// that are now empty. It stops as soon as it encounters a non-empty directory
// or reaches the stopAt boundary.
func removeEmptyParents(dir, stopAt string) {
	dir = filepath.Clean(dir)
	stopAt = filepath.Clean(stopAt)
	for dir != stopAt && strings.HasPrefix(dir, stopAt+"/") {
		if err := os.Remove(dir); err != nil {
			break // not empty or permission error
		}
		log.Printf("Removed empty directory: %s", dir)
		dir = filepath.Dir(dir)
	}
}

// Helper for templates - kept for backward compat
func (s *Server) projects() []discovery.Project {
	return s.cache.Projects()
}

// dirExists returns true if the given path is an existing directory.
func dirExists(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.IsDir()
}

// sortProjectsByModTime is used by handleIndex
func sortProjectsByModTime(projects []discovery.Project) []discovery.Project {
	sorted := make([]discovery.Project, len(projects))
	copy(sorted, projects)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].LastModified.After(sorted[j].LastModified)
	})
	return sorted
}
