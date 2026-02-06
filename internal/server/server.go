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

	"github.com/loganj/birdseye/internal/agents"
	"github.com/loganj/birdseye/internal/cache"
	"github.com/loganj/birdseye/internal/comments"
	"github.com/loganj/birdseye/internal/discovery"
	"github.com/loganj/birdseye/internal/watcher"
	"github.com/loganj/birdseye/templates"
)

type Server struct {
	cache       *cache.Cache
	watcher     *watcher.Watcher
	comments    *comments.Store
	mcpHandler  http.Handler
	mux         *http.ServeMux
	tmpl        *template.Template
	loadOnce    sync.Once
	templateDir string // if set, reload templates from disk on each request
}

func New(c *cache.Cache, w *watcher.Watcher, cs *comments.Store, mcpHandler http.Handler, templateDir string) *Server {
	s := &Server{
		cache:       c,
		watcher:     w,
		comments:    cs,
		mcpHandler:  mcpHandler,
		mux:         http.NewServeMux(),
		templateDir: templateDir,
	}

	// Parse templates from embedded filesystem
	s.tmpl = template.Must(template.ParseFS(templates.FS, "*.html"))

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

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	s.ensureLoaded()
	s.mux.ServeHTTP(w, r)
}

// ensureLoaded does fast project discovery on first request, then populates in background.
func (s *Server) ensureLoaded() {
	s.loadOnce.Do(func() {
		root := s.cache.Root()
		projects, err := discovery.FindProjectsFast(root)
		if err != nil {
			log.Printf("Error discovering projects: %v", err)
			return
		}

		log.Printf("Found %d projects with thoughts/ directories", len(projects))

		s.cache.SetProjects(projects)

		if err := s.watcher.Start(); err != nil {
			log.Printf("Warning: file watcher failed to start: %v", err)
		}

		// Populate file lists and enrichment in background so the first
		// request isn't blocked. Pages render immediately with whatever
		// data is available; SSE pushes an update when population completes.
		go s.populateProjects()
	})
}

// populateProjects scans file lists and fills in git info + summaries in the background.
func (s *Server) populateProjects() {
	s.cache.RefreshAllProjects()
	log.Printf("Background file scan complete")
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventProjectsChanged})

	// Now enrich with git info and summaries
	projects := s.cache.Projects()
	var wg sync.WaitGroup
	sem := make(chan struct{}, 8) // bound concurrency to avoid fork-bombing

	for _, p := range projects {
		wg.Add(1)
		go func(p discovery.Project) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()

			var git *discovery.GitInfo
			if p.Name != "(root)" {
				git = discovery.GetGitInfo(p.Path)
			}

			var summary string
			if p.Name == "(root)" {
				summary = "Cross-project notes and research"
			} else {
				// Use cached file list to avoid re-walking the directory
				cachedFiles := s.cache.ProjectFiles(p.Name)
				if len(cachedFiles) > 0 {
					limit := 5
					if len(cachedFiles) < limit {
						limit = len(cachedFiles)
					}
					filePaths := make([]string, limit)
					for i, f := range cachedFiles[:limit] {
						filePaths[i] = filepath.Join(p.ThoughtsPath, f.Path)
					}
					summary = discovery.GenerateSummaryFromFiles(filePaths)
				}
			}

			s.cache.EnrichProject(p.Name, git, summary)
		}(p)
	}

	wg.Wait()
	log.Printf("Background enrichment complete (git info + summaries)")
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventProjectsChanged})
}

func (s *Server) routes() {
	s.mux.HandleFunc("/", s.handleIndex)
	s.mux.HandleFunc("/project/", s.handleProject)
	s.mux.HandleFunc("/file/", s.handleFile)
	s.mux.HandleFunc("/search", s.handleSearch)
	s.mux.HandleFunc("/recent", s.handleRecent)
	s.mux.HandleFunc("/events", s.handleEvents)
	// API endpoints for dynamic updates
	s.mux.HandleFunc("/api/projects", s.handleAPIProjects)
	s.mux.HandleFunc("/api/project/", s.handleAPIProjectFiles)
	s.mux.HandleFunc("/api/recent", s.handleAPIRecent)
	s.mux.HandleFunc("/api/copy-file", s.handleCopyFile)
	s.mux.HandleFunc("/api/project-info", s.handleProjectInfo)
	s.mux.HandleFunc("/api/delete-project", s.handleDeleteProject)
	// Comment and review API endpoints
	s.mux.HandleFunc("/api/threads", s.handleAPIThreads)
	s.mux.HandleFunc("/api/threads/", s.handleAPIThreadAction)
	s.mux.HandleFunc("/api/reviews", s.handleAPIListReviews)
	// MCP (Model Context Protocol) endpoint
	if s.mcpHandler != nil {
		s.mux.Handle("/mcp", s.mcpHandler)
		s.mux.Handle("/mcp/", s.mcpHandler)
	}
}

type IndexFile struct {
	Project  string
	FilePath string
	FileName string
	ModTime  time.Time
	Age      string
}

func (s *Server) handleIndex(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}

	// Get files from cache
	allFiles := s.cache.AllFiles(100)
	files := make([]IndexFile, len(allFiles))
	for i, f := range allFiles {
		files[i] = IndexFile{
			Project:  f.Project,
			FilePath: f.Path,
			FileName: f.Name,
			ModTime:  f.ModTime,
			Age:      formatAge(f.ModTime),
		}
	}

	// Get projects sorted by last modified
	sortedProjects := s.cache.ProjectsSortedByModTime()

	// Check for active agents
	activeAgents := agents.FindActive()
	agentMap := make(map[string]string)
	for _, p := range sortedProjects {
		if agts, ok := activeAgents[p.Path]; ok && len(agts) > 0 {
			if len(agts) == 1 {
				prompt := agts[0].Prompt
				if prompt == "" {
					prompt = "Agent active"
				}
				agentMap[p.Name] = prompt
			} else {
				agentMap[p.Name] = fmt.Sprintf("%d agents active", len(agts))
			}
		}
	}

	ages := make(map[string]string)
	for _, p := range sortedProjects {
		ages[p.Name] = computeProjectAge(p)
	}

	data := struct {
		Projects []discovery.Project
		Files    []IndexFile
		Agents   map[string]string
		Ages     map[string]string
	}{
		Projects: sortedProjects,
		Files:    files,
		Agents:   agentMap,
		Ages:     ages,
	}
	s.getTemplate().ExecuteTemplate(w, "index.html", data)
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
	Name     string
	Path     string
	ModTime  time.Time
	Age      string
	FileType string // "research", "plan", or "other"
}

func (s *Server) handleProject(w http.ResponseWriter, r *http.Request) {
	// Parse /project/{name}
	projectName := strings.TrimPrefix(r.URL.Path, "/project/")
	projectName = strings.TrimSuffix(projectName, "/")

	// Find project
	project := s.cache.FindProject(projectName)
	if project == nil {
		http.NotFound(w, r)
		return
	}

	// Get files from cache
	cachedFiles := s.cache.ProjectFiles(projectName)
	files := make([]ProjectFile, len(cachedFiles))
	for i, f := range cachedFiles {
		files[i] = ProjectFile{
			Name:     f.Name,
			Path:     f.Path,
			ModTime:  f.ModTime,
			Age:      formatAge(f.ModTime),
			FileType: f.FileType,
		}
	}

	// Check for active agents
	var agentPrompts []string
	activeAgents := agents.FindActive()
	if agts, ok := activeAgents[project.Path]; ok && len(agts) > 0 {
		agentPrompts = make([]string, len(agts))
		for i, a := range agts {
			agentPrompts[i] = a.Prompt
		}
	}

	data := struct {
		Project      *discovery.Project
		Files        []ProjectFile
		AgentPrompts []string
	}{
		Project:      project,
		Files:        files,
		AgentPrompts: agentPrompts,
	}
	s.getTemplate().ExecuteTemplate(w, "project.html", data)
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

type APIProject struct {
	Name         string   `json:"name"`
	Branch       string   `json:"branch,omitempty"`
	Dirty        bool     `json:"dirty,omitempty"`
	FileCount    int      `json:"fileCount"`
	Summary      string   `json:"summary,omitempty"`
	LastModified string   `json:"lastModified"`
	AgentCount   int      `json:"agentCount,omitempty"`
	AgentPrompts []string `json:"agentPrompts,omitempty"`
	Age          string   `json:"age,omitempty"`
	ReviewCount  int      `json:"reviewCount,omitempty"`
}

func (s *Server) handleAPIProjects(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")

	activeAgents := agents.FindActive()

	projects := s.cache.ProjectsSortedByModTime()
	result := make([]APIProject, len(projects))
	for i, p := range projects {
		result[i] = APIProject{
			Name:         p.Name,
			FileCount:    p.FileCount,
			Summary:      p.Summary,
			LastModified: p.LastModified.Format(time.RFC3339),
			Age:          computeProjectAge(p),
		}
		if p.Git != nil {
			result[i].Branch = p.Git.Branch
			result[i].Dirty = p.Git.Dirty
		}
		if agts, ok := activeAgents[p.Path]; ok && len(agts) > 0 {
			result[i].AgentCount = len(agts)
			prompts := make([]string, len(agts))
			for j, a := range agts {
				prompts[j] = a.Prompt
			}
			result[i].AgentPrompts = prompts
		}
		// Count files in review for this project
		if reviews, err := s.comments.ListFilesInReview(p.Name); err == nil {
			result[i].ReviewCount = len(reviews)
		}
	}
	json.NewEncoder(w).Encode(result)
}

type APIFile struct {
	Name     string `json:"name"`
	Path     string `json:"path"`
	Project  string `json:"project,omitempty"`
	Age      string `json:"age"`
	FileType string `json:"fileType,omitempty"`
}

func (s *Server) handleAPIProjectFiles(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")

	projectName := strings.TrimPrefix(r.URL.Path, "/api/project/")
	projectName = strings.TrimSuffix(projectName, "/")

	files := s.cache.ProjectFiles(projectName)
	result := make([]APIFile, len(files))
	for i, f := range files {
		result[i] = APIFile{
			Name:     f.Name,
			Path:     f.Path,
			Age:      formatAge(f.ModTime),
			FileType: f.FileType,
		}
	}
	json.NewEncoder(w).Encode(result)
}

func (s *Server) handleAPIRecent(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")

	files := s.cache.AllFiles(50)
	result := make([]APIFile, len(files))
	for i, f := range files {
		result[i] = APIFile{
			Name:    f.Name,
			Path:    f.Path,
			Project: f.Project,
			Age:     formatAge(f.ModTime),
		}
	}
	json.NewEncoder(w).Encode(result)
}

func (s *Server) handleCopyFile(w http.ResponseWriter, r *http.Request) {
	projectName := r.URL.Query().Get("project")
	filePath := r.URL.Query().Get("path")
	if projectName == "" || filePath == "" {
		http.Error(w, "missing project or path", http.StatusBadRequest)
		return
	}

	project := s.cache.FindProject(projectName)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	fullPath := filepath.Join(project.ThoughtsPath, filePath)

	// Verify the file exists and is under the thoughts directory
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
	projectName := r.URL.Query().Get("name")
	if projectName == "" {
		http.Error(w, "missing project name", http.StatusBadRequest)
		return
	}
	project := s.cache.FindProject(projectName)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	info := ProjectInfo{
		FileCount: len(s.cache.ProjectFiles(projectName)),
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
	projectName := r.URL.Query().Get("name")
	if projectName == "" {
		http.Error(w, "missing project name", http.StatusBadRequest)
		return
	}
	if projectName == "(root)" {
		http.Error(w, "cannot delete root project", http.StatusForbidden)
		return
	}

	project := s.cache.FindProject(projectName)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	if err := os.RemoveAll(project.Path); err != nil {
		log.Printf("Failed to delete project %s: %v", projectName, err)
		http.Error(w, fmt.Sprintf("failed to delete: %v", err), http.StatusInternalServerError)
		return
	}

	log.Printf("Deleted project directory: %s", project.Path)
	s.cache.RemoveProject(projectName)
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventProjectsChanged})
	w.WriteHeader(http.StatusNoContent)
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
