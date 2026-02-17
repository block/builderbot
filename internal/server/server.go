package server

import (
	"encoding/json"
	"fmt"
	"html/template"
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

	"github.com/loganj/birdseye/internal/activity"
	"github.com/loganj/birdseye/internal/agents"
	"github.com/loganj/birdseye/internal/cache"
	"github.com/loganj/birdseye/internal/comments"
	"github.com/loganj/birdseye/internal/config"
	"github.com/loganj/birdseye/internal/discovery"
	"github.com/loganj/birdseye/internal/watcher"
	"github.com/loganj/birdseye/templates"
)

type Server struct {
	cache       *cache.Cache
	watcher     *watcher.Watcher
	comments    *comments.Store
	agents      *agents.Manager
	activity    *activity.Tracker
	mcpHandler  http.Handler
	mux         *http.ServeMux
	tmpl        *template.Template
	layoutTmpl  *template.Template // base layout for sidebar pages
	loadOnce    sync.Once
	templateDir string // if set, reload templates from disk on each request
	cfg         *config.Config
	cfgPath     string
	cfgMu       sync.Mutex // protects cfg mutations
}

func New(c *cache.Cache, w *watcher.Watcher, cs *comments.Store, mcpHandler http.Handler, am *agents.Manager, act *activity.Tracker, templateDir string, cfg *config.Config, cfgPath string) *Server {
	s := &Server{
		cache:       c,
		watcher:     w,
		comments:    cs,
		agents:      am,
		activity:    act,
		mcpHandler:  mcpHandler,
		mux:         http.NewServeMux(),
		templateDir: templateDir,
		cfg:         cfg,
		cfgPath:     cfgPath,
	}

	// Parse templates from embedded filesystem
	s.tmpl = template.Must(template.ParseFS(templates.FS, "*.html"))
	// Parse layout template separately for clone-per-page rendering
	s.layoutTmpl = template.Must(template.New("").ParseFS(templates.FS, "_layout.html"))

	if am != nil {
		am.SetOnChange(func(projectName string) {
			s.watcher.Broadcast(watcher.Event{Type: watcher.EventAgentsChanged, Project: projectName})
		})
	}

	cs.SetOnTyping(func(project string) {
		w.Broadcast(watcher.Event{Type: watcher.EventCommentsChanged, Project: project})
	})

	s.routes()
	return s
}

func (s *Server) getTemplate() *template.Template {
	if s.templateDir != "" {
		t, err := template.ParseGlob(filepath.Join(s.templateDir, "*.html"))
		if err != nil {
			log.Printf("Error reloading templates: %v", err)
			return s.tmpl
		}
		return t
	}
	return s.tmpl
}

// getPageTemplate returns a template set with the layout + a specific page template.
// Each page gets its own clone so block definitions (title, content, etc.) don't conflict.
func (s *Server) getPageTemplate(pageName string) *template.Template {
	if s.templateDir != "" {
		layoutPath := filepath.Join(s.templateDir, "_layout.html")
		pagePath := filepath.Join(s.templateDir, pageName)
		t, err := template.ParseFiles(layoutPath, pagePath)
		if err != nil {
			log.Printf("Error loading page template %s: %v", pageName, err)
			return s.tmpl
		}
		return t
	}
	t, err := template.Must(s.layoutTmpl.Clone()).ParseFS(templates.FS, pageName)
	if err != nil {
		log.Printf("Error cloning page template %s: %v", pageName, err)
		return s.tmpl
	}
	return t
}

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	s.ensureLoaded()
	s.mux.ServeHTTP(w, r)
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
	for _, pc := range s.cfg.Projects {
		p, err := discovery.LoadStandaloneProject(pc.Path, pc)
		if err != nil {
			log.Printf("Warning: could not load standalone project %s: %v", pc.Path, err)
			continue
		}
		allProjects = append(allProjects, p)
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

// populateProjects scans file lists and fills in git info in the background.
func (s *Server) populateProjects() {
	s.cache.RefreshAllProjects()
	log.Printf("Background file scan complete")
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventProjectsChanged})

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

func (s *Server) routes() {
	s.mux.HandleFunc("/", s.handleIndex)
	s.mux.HandleFunc("/workspace/", s.handleWorkspace)
	s.mux.HandleFunc("/project/", s.handleProject)
	s.mux.HandleFunc("/file/", s.handleFile)
	s.mux.HandleFunc("/search", s.handleSearch)
	s.mux.HandleFunc("/recent", s.handleRecent)
	s.mux.HandleFunc("/in-review", s.handleInReview)
	s.mux.HandleFunc("/events", s.handleEvents)
	// API endpoints for dynamic updates
	s.mux.HandleFunc("/api/projects", s.handleAPIProjects)
	s.mux.HandleFunc("/api/project/", s.handleAPIProjectFiles)
	s.mux.HandleFunc("/api/recent", s.handleAPIRecent)
	s.mux.HandleFunc("/api/in-review", s.handleAPIInReview)
	s.mux.HandleFunc("/api/copy-file", s.handleCopyFile)
	s.mux.HandleFunc("/api/project-info", s.handleProjectInfo)
	s.mux.HandleFunc("/api/delete-project", s.handleDeleteProject)
	s.mux.HandleFunc("/api/delete-file", s.handleDeleteFile)
	// Workspace and project management
	s.mux.HandleFunc("/api/workspaces", s.handleAPIWorkspaces)
	s.mux.HandleFunc("/api/sources", s.handleAPISources)
	s.mux.HandleFunc("/api/open", s.handleAPIOpen)
	// Comment and review API endpoints
	s.mux.HandleFunc("/api/threads", s.handleAPIThreads)
	s.mux.HandleFunc("/api/threads/", s.handleAPIThreadAction)
	s.mux.HandleFunc("/api/reviews", s.handleAPIListReviews)
	// Agent management endpoints
	s.mux.HandleFunc("/api/agents", s.handleAgentStatus)
	s.mux.HandleFunc("/api/agents/start", s.handleAgentStart)
	s.mux.HandleFunc("/api/agents/stop", s.handleAgentStop)
	// Raw file content
	s.mux.HandleFunc("/api/raw", s.handleRawFile)
	// Publish to Blockcell
	s.mux.HandleFunc("/api/publish", s.handlePublish)
	s.mux.HandleFunc("/api/publish-state", s.handlePublishState)
	// Static assets embedded in the templates package
	s.mux.HandleFunc("/static/", s.handleStatic)
	// MCP (Model Context Protocol) endpoint
	if s.mcpHandler != nil {
		s.mux.Handle("/mcp", s.mcpHandler)
		s.mux.Handle("/mcp/", s.mcpHandler)
	}
}

// handleStatic serves embedded static assets (JS, CSS) from the templates package.
func (s *Server) handleStatic(w http.ResponseWriter, r *http.Request) {
	name := strings.TrimPrefix(r.URL.Path, "/static/")
	if name == "" {
		http.NotFound(w, r)
		return
	}

	// Sanitize: only allow a bare filename (no slashes, no traversal).
	if strings.ContainsAny(name, "/\\") || name == "." || name == ".." || strings.Contains(name, "..") {
		http.NotFound(w, r)
		return
	}

	// In dev mode, serve from disk for live reload
	if s.templateDir != "" {
		http.ServeFile(w, r, filepath.Join(s.templateDir, name))
		return
	}

	data, err := templates.FS.ReadFile(name)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	if strings.HasSuffix(name, ".js") {
		w.Header().Set("Content-Type", "application/javascript")
	}
	w.Header().Set("Cache-Control", "public, max-age=3600")
	w.Write(data)
}

// NavData provides the sidebar with workspace/project links on every page.
type NavData struct {
	Workspaces    []NavWorkspace
	Standalone    []NavProject
	ActiveProject *NavProject // active workspace project (shown indented under workspace)
	ActiveQN      string      // qualified name of the active project (for standalone highlighting)
	ActiveWS      string      // workspace path of the active project (for workspace highlighting)
	ActiveWSName  string      // display name of the active workspace (for URL construction)
	InProject     bool        // true when viewing a project or file page (triggers focused sidebar)
	SearchQuery   string      // pre-fill search box if on search page
	ReviewCount   int         // total files with open comment threads across all projects
	ActivePage    string      // "recent", "in-review", etc. for sidebar link highlighting
}

type NavWorkspace struct {
	Name     string
	Path     string
	HasAgent bool // true if any project in this workspace has an MCP connection
}

type NavProject struct {
	Name          string
	QualifiedName string
	Path          string // filesystem path (for removal API)
	HasAgent      bool
	Badges        []discovery.Badge
	Branch        string
	Dirty         bool
}

// buildNav builds NavData from current config and cache state.
func (s *Server) buildNav(activeQN string) NavData {
	nav := NavData{ActiveQN: activeQN}

	projects := s.cache.ProjectsSortedByModTime()

	// Check which workspaces have active MCP connections
	wsHasAgent := make(map[string]bool)
	for _, p := range projects {
		qn := p.QualifiedName()
		hasAgent := s.agents != nil && s.agents.Status(qn) != nil && s.agents.Status(qn).Running

		if p.Origin == "standalone" {
			np := NavProject{
				Name:          p.Name,
				QualifiedName: qn,
				Path:          p.Path,
				HasAgent:      hasAgent,
				Badges:        p.Badges(),
			}
			if p.Git != nil {
				np.Branch = p.Git.Branch
				np.Dirty = p.Git.Dirty
			}
			nav.Standalone = append(nav.Standalone, np)
			continue
		}

		if hasAgent {
			wsHasAgent[p.WorkspacePath] = true
		}

		// Build active project details for workspace projects
		if qn == activeQN {
			nav.ActiveWS = p.WorkspacePath
			nav.ActiveWSName = p.WorkspaceName
			np := NavProject{
				Name:          p.Name,
				QualifiedName: qn,
				HasAgent:      hasAgent,
				Badges:        p.Badges(),
			}
			if p.Git != nil {
				np.Branch = p.Git.Branch
				np.Dirty = p.Git.Dirty
			}
			nav.ActiveProject = &np
		}
	}

	for _, ws := range s.cfg.Workspaces {
		nav.Workspaces = append(nav.Workspaces, NavWorkspace{
			Name:     ws.DisplayName(),
			Path:     ws.Path,
			HasAgent: wsHasAgent[ws.Path],
		})
	}

	// Count files in review across all projects
	for _, p := range projects {
		if reviews, err := s.comments.ListFilesInReview(p.QualifiedName()); err == nil {
			nav.ReviewCount += len(reviews)
		}
	}

	return nav
}

// PageData wraps nav data and page-specific data for the layout template.
type PageData struct {
	Nav  NavData
	Page interface{}
}

// renderPage wraps page-specific data with NavData and executes the layout.
// It clones the layout template and parses the page template into the clone,
// so each page's block definitions (title, content, etc.) don't conflict.
func (s *Server) renderPage(w http.ResponseWriter, tmplName string, nav NavData, pageData interface{}) {
	data := PageData{Nav: nav, Page: pageData}
	t := s.getPageTemplate(tmplName)
	if err := t.ExecuteTemplate(w, tmplName, data); err != nil {
		log.Printf("Template error (%s): %v", tmplName, err)
	}
}

type IndexFile struct {
	Project  string
	FilePath string
	FileName string
	ModTime  time.Time
	Age      string
}

// WorkspaceGroup groups projects discovered from a single workspace directory.
type WorkspaceGroup struct {
	Name     string
	Path     string
	Projects []discovery.Project
}

func (s *Server) handleIndex(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}

	// Redirect to first workspace or standalone project
	if len(s.cfg.Workspaces) > 0 {
		http.Redirect(w, r, "/workspace/"+s.cfg.Workspaces[0].DisplayName(), http.StatusFound)
		return
	}

	projects := s.cache.ProjectsSortedByModTime()
	for _, p := range projects {
		if p.Origin == "standalone" {
			http.Redirect(w, r, "/project/"+p.QualifiedName(), http.StatusFound)
			return
		}
	}

	// Nothing configured
	http.Redirect(w, r, "/recent", http.StatusFound)
}

func (s *Server) handleWorkspace(w http.ResponseWriter, r *http.Request) {
	wsName := strings.TrimPrefix(r.URL.Path, "/workspace/")
	if wsName == "" {
		http.Redirect(w, r, "/", http.StatusFound)
		return
	}

	// Find workspace config by display name
	var wsConfig *config.Workspace
	for i := range s.cfg.Workspaces {
		if s.cfg.Workspaces[i].DisplayName() == wsName {
			wsConfig = &s.cfg.Workspaces[i]
			break
		}
	}
	if wsConfig == nil {
		http.NotFound(w, r)
		return
	}

	// Get projects for this workspace only
	sortedProjects := s.cache.ProjectsSortedByModTime()

	agentConnected := make(map[string]bool)
	ages := make(map[string]string)
	var wsProjects []discovery.Project

	for _, p := range sortedProjects {
		if p.Origin == "standalone" || p.WorkspaceName != wsConfig.DisplayName() {
			continue
		}
		qn := p.QualifiedName()
		if s.agents != nil && s.agents.Status(qn) != nil && s.agents.Status(qn).Running {
			agentConnected[qn] = true
		}
		ages[qn] = computeProjectAge(p)
		wsProjects = append(wsProjects, p)
	}

	// Active projects first (by mod time), then empty (by name)
	sort.SliceStable(wsProjects, func(i, j int) bool {
		iActive := wsProjects[i].FileCount > 0
		jActive := wsProjects[j].FileCount > 0
		if iActive != jActive {
			return iActive
		}
		if !iActive {
			return wsProjects[i].Name < wsProjects[j].Name
		}
		return false
	})

	wg := WorkspaceGroup{Name: wsConfig.DisplayName(), Path: wsConfig.Path, Projects: wsProjects}

	nav := s.buildNav("")
	nav.ActiveWS = wsConfig.Path
	nav.ActiveWSName = wsConfig.DisplayName()
	pageData := struct {
		Workspaces     []WorkspaceGroup
		Standalone     []discovery.Project
		AgentConnected map[string]bool
		Ages           map[string]string
		WorkspacePath  string
	}{
		Workspaces:     []WorkspaceGroup{wg},
		AgentConnected: agentConnected,
		Ages:           ages,
		WorkspacePath:  wsConfig.Path,
	}
	s.renderPage(w, "index.html", nav, pageData)
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

func (s *Server) handleProject(w http.ResponseWriter, r *http.Request) {
	// Parse /project/{qualifiedName} where qualifiedName is "workspace/project" or "project"
	qualifiedName := strings.TrimPrefix(r.URL.Path, "/project/")
	qualifiedName = strings.TrimSuffix(qualifiedName, "/")
	if qualifiedName == "" {
		http.NotFound(w, r)
		return
	}

	// Find project by qualified name
	project := s.cache.FindProject(qualifiedName)
	if project == nil {
		http.NotFound(w, r)
		return
	}

	cachedFiles := s.cache.ProjectFiles(qualifiedName)
	groups := buildFileGroups(project, cachedFiles)

	nav := s.buildNav(project.QualifiedName())
	nav.InProject = true
	pageData := struct {
		Project *discovery.Project
		Groups  []FileGroupView
	}{
		Project: project,
		Groups:  groups,
	}
	s.renderPage(w, "project.html", nav, pageData)
}

// handleEvents is the SSE endpoint for live updates
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
			fmt.Fprintf(w, "event: change\ndata: %s\n\n", data)
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

type APIProject struct {
	Name           string     `json:"name"`
	QualifiedName  string     `json:"qualifiedName"`
	Workspace      string     `json:"workspace"`
	WorkspacePath  string     `json:"workspacePath,omitempty"`
	ProjectPath    string     `json:"projectPath"`
	Origin         string     `json:"origin"`
	Badges         []APIBadge `json:"badges"`
	Branch         string     `json:"branch,omitempty"`
	Dirty          bool       `json:"dirty,omitempty"`
	FileCount      int        `json:"fileCount"`
	LastModified   string     `json:"lastModified"`
	AgentConnected bool       `json:"agentConnected,omitempty"`
	AgentRunning   bool       `json:"agentRunning,omitempty"`
	Age            string     `json:"age,omitempty"`
	ReviewCount    int        `json:"reviewCount,omitempty"`
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
		// Count files in review for this project
		if reviews, err := s.comments.ListFilesInReview(qn); err == nil {
			result[i].ReviewCount = len(reviews)
		}
	}
	json.NewEncoder(w).Encode(result)
}

type APIFile struct {
	Name         string `json:"name"`
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

	cachedFiles := s.cache.ProjectFiles(qualifiedName)
	fileGroups := buildFileGroups(project, cachedFiles)

	var result []APIFileGroupView
	for _, g := range fileGroups {
		apiGroup := APIFileGroupView{
			Name:       g.Name,
			Source:     g.Source,
			SourceType: g.SourceType,
			Auto:       g.Auto,
			BadgeText:  g.BadgeText,
			BadgeColor: g.BadgeColor,
			BadgeBg:    g.BadgeBg,
		}
		for _, f := range g.Files {
			apiGroup.Files = append(apiGroup.Files, APIFile{
				Name:       f.Name,
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

func (s *Server) handleAPIRecent(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")

	files := s.mergeRecentFiles(50)
	result := make([]APIFile, len(files))
	for i, f := range files {
		result[i] = APIFile{
			Name:         f.DisplayName,
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

	// Verify the file exists and is under the project directory
	if _, err := os.Stat(fullPath); err != nil {
		http.Error(w, "file not found", http.StatusNotFound)
		return
	}

	cmd := exec.Command("osascript", "-e", fmt.Sprintf(`set the clipboard to (POSIX file %q)`, fullPath))
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

	// Verify the file exists and is under the project directory
	if !strings.HasPrefix(fullPath, project.Path+"/") {
		http.Error(w, "invalid path", http.StatusBadRequest)
		return
	}
	if _, err := os.Stat(fullPath); err != nil {
		http.Error(w, "file not found", http.StatusNotFound)
		return
	}

	if err := os.Remove(fullPath); err != nil {
		log.Printf("Failed to delete file %s: %v", fullPath, err)
		http.Error(w, fmt.Sprintf("failed to delete: %v", err), http.StatusInternalServerError)
		return
	}

	log.Printf("Deleted file: %s", fullPath)

	// Clean up empty parent directories, stopping at the project root
	removeEmptyParents(filepath.Dir(fullPath), project.Path)

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

// sortProjectsByModTime is used by handleIndex
func sortProjectsByModTime(projects []discovery.Project) []discovery.Project {
	sorted := make([]discovery.Project, len(projects))
	copy(sorted, projects)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].LastModified.After(sorted[j].LastModified)
	})
	return sorted
}
