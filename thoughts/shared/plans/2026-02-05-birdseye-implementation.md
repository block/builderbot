# Birdseye Implementation Plan

## Overview

Birdseye is a local web application that provides a bird's eye view of all projects in ~/Development and the `thoughts/` files within them. It helps track agent-generated markdown files (research, plans, etc.) across multiple repositories.

## Current State Analysis

- ~/Development contains 40+ git repositories and worktrees
- 18+ projects have `thoughts/` directories with `shared/{research,plans,guides}/` structure
- Markdown files use YAML frontmatter with metadata (date, tags, status, researcher)
- File naming convention: `YYYY-MM-DD-description.md`
- User needs to easily find, read, and track recently modified files

## Desired End State

A single Go binary that:
1. Scans ~/Development for projects containing `thoughts/` directories
2. Serves a web UI showing all projects, their thoughts files, and recent changes
3. Displays git branch and dirty status for each project
4. Renders markdown files with syntax highlighting
5. Provides search across all thoughts files
6. Runs via `just run` with no environment setup required

### Verification:
- `just build` produces a single binary
- `just run` starts server on localhost:8080
- Dashboard shows all projects with thoughts/ directories
- Can navigate to any project and read its markdown files
- Search finds files by content
- Git branch/status shown for each project

## What We're NOT Doing

- No authentication (local-only tool)
- No file editing (read-only in v1)
- No database (filesystem is source of truth)
- No JavaScript framework (htmx for interactivity)
- No change tracking/history (just current filesystem state)
- No notification system

## Implementation Approach

Build incrementally with a working server at each phase:
1. Basic server + project discovery
2. File listing and navigation
3. Markdown rendering
4. Search functionality
5. Polish (git info, recent files view)

---

## Phase 1: Project Scaffolding and Discovery

### Overview
Set up Go project structure, justfile, and implement project discovery that finds all directories containing `thoughts/`.

### Changes Required:

#### 1. Go module and main entry point
**File**: `main.go`
```go
package main

import (
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"path/filepath"

	"github.com/loganj/birdseye/internal/discovery"
	"github.com/loganj/birdseye/internal/server"
)

func main() {
	port := flag.Int("port", 8080, "port to listen on")
	root := flag.String("root", "", "root directory to scan (default: ~/Development)")
	flag.Parse()

	rootDir := *root
	if rootDir == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			log.Fatal(err)
		}
		rootDir = filepath.Join(home, "Development")
	}

	projects, err := discovery.FindProjects(rootDir)
	if err != nil {
		log.Fatal(err)
	}

	fmt.Printf("Found %d projects with thoughts/ directories\n", len(projects))

	srv := server.New(rootDir, projects)
	addr := fmt.Sprintf(":%d", *port)
	fmt.Printf("Starting server at http://localhost%s\n", addr)
	log.Fatal(http.ListenAndServe(addr, srv))
}
```

#### 2. Project discovery
**File**: `internal/discovery/discovery.go`
```go
package discovery

import (
	"os"
	"path/filepath"
	"sort"
)

type Project struct {
	Name       string
	Path       string
	ThoughtsPath string
}

func FindProjects(root string) ([]Project, error) {
	var projects []Project

	entries, err := os.ReadDir(root)
	if err != nil {
		return nil, err
	}

	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}
		if entry.Name()[0] == '.' {
			continue
		}

		projectPath := filepath.Join(root, entry.Name())
		thoughtsPath := filepath.Join(projectPath, "thoughts")

		info, err := os.Stat(thoughtsPath)
		if err != nil || !info.IsDir() {
			continue
		}

		projects = append(projects, Project{
			Name:         entry.Name(),
			Path:         projectPath,
			ThoughtsPath: thoughtsPath,
		})
	}

	// Also check for thoughts/ directly in root (~/Development/thoughts)
	rootThoughts := filepath.Join(root, "thoughts")
	if info, err := os.Stat(rootThoughts); err == nil && info.IsDir() {
		projects = append(projects, Project{
			Name:         "(root)",
			Path:         root,
			ThoughtsPath: rootThoughts,
		})
	}

	sort.Slice(projects, func(i, j int) bool {
		return projects[i].Name < projects[j].Name
	})

	return projects, nil
}
```

#### 3. Basic HTTP server
**File**: `internal/server/server.go`
```go
package server

import (
	"html/template"
	"net/http"

	"github.com/loganj/birdseye/internal/discovery"
)

type Server struct {
	root     string
	projects []discovery.Project
	mux      *http.ServeMux
	tmpl     *template.Template
}

func New(root string, projects []discovery.Project) *Server {
	s := &Server{
		root:     root,
		projects: projects,
		mux:      http.NewServeMux(),
	}
	s.tmpl = template.Must(template.ParseGlob("templates/*.html"))
	s.routes()
	return s
}

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	s.mux.ServeHTTP(w, r)
}

func (s *Server) routes() {
	s.mux.HandleFunc("/", s.handleIndex)
}

func (s *Server) handleIndex(w http.ResponseWriter, r *http.Request) {
	data := struct {
		Projects []discovery.Project
	}{
		Projects: s.projects,
	}
	s.tmpl.ExecuteTemplate(w, "index.html", data)
}
```

#### 4. Index template
**File**: `templates/index.html`
```html
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Birdseye</title>
    <style>
        * { box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 1200px;
            margin: 0 auto;
            padding: 20px;
            background: #1a1a2e;
            color: #eee;
        }
        h1 { color: #00d4ff; }
        .projects {
            display: grid;
            grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
            gap: 16px;
        }
        .project {
            background: #16213e;
            border-radius: 8px;
            padding: 16px;
            border: 1px solid #0f3460;
        }
        .project:hover {
            border-color: #00d4ff;
        }
        .project h2 {
            margin: 0 0 8px 0;
            font-size: 1.1em;
        }
        .project a {
            color: #00d4ff;
            text-decoration: none;
        }
        .project a:hover {
            text-decoration: underline;
        }
    </style>
</head>
<body>
    <h1>Birdseye</h1>
    <p>{{len .Projects}} projects with thoughts/</p>
    <div class="projects">
        {{range .Projects}}
        <div class="project">
            <h2><a href="/project/{{.Name}}">{{.Name}}</a></h2>
        </div>
        {{end}}
    </div>
</body>
</html>
```

#### 5. Justfile
**File**: `justfile`
```just
# Default recipe
default: run

# Build the binary
build:
    go build -o birdseye .

# Run the server
run: build
    ./birdseye

# Run with live reload (requires entr)
dev:
    find . -name '*.go' -o -name '*.html' | entr -r just run

# Clean build artifacts
clean:
    rm -f birdseye

# Format code
fmt:
    go fmt ./...

# Run tests
test:
    go test ./...
```

#### 6. Go module
**File**: `go.mod`
```
module github.com/loganj/birdseye

go 1.21
```

### Success Criteria:

#### Automated Verification:
- [ ] `just build` compiles without errors
- [ ] `just run` starts server on port 8080
- [ ] `curl http://localhost:8080/` returns HTML with project list

#### Manual Verification:
- [ ] Browser shows dashboard with project cards
- [ ] All expected projects are listed
- [ ] ~/Development/thoughts shows as "(root)"

**Implementation Note**: After completing this phase and all automated verification passes, pause here for manual confirmation before proceeding to the next phase.

---

## Phase 2: File Navigation

### Overview
Add the ability to browse thoughts/ directory contents and navigate into subdirectories.

### Changes Required:

#### 1. Add file listing to discovery
**File**: `internal/discovery/files.go`
```go
package discovery

import (
	"os"
	"path/filepath"
	"sort"
	"time"
)

type ThoughtFile struct {
	Name    string
	Path    string    // relative to thoughts/
	ModTime time.Time
	IsDir   bool
}

func ListThoughts(thoughtsPath, subpath string) ([]ThoughtFile, error) {
	dir := filepath.Join(thoughtsPath, subpath)
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}

	var files []ThoughtFile
	for _, entry := range entries {
		if entry.Name()[0] == '.' {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			continue
		}
		relPath := filepath.Join(subpath, entry.Name())
		files = append(files, ThoughtFile{
			Name:    entry.Name(),
			Path:    relPath,
			ModTime: info.ModTime(),
			IsDir:   entry.IsDir(),
		})
	}

	// Sort: directories first, then by mod time (newest first)
	sort.Slice(files, func(i, j int) bool {
		if files[i].IsDir != files[j].IsDir {
			return files[i].IsDir
		}
		return files[i].ModTime.After(files[j].ModTime)
	})

	return files, nil
}
```

#### 2. Add project route handler
**File**: `internal/server/server.go` (add to routes and handlers)
```go
func (s *Server) routes() {
	s.mux.HandleFunc("/", s.handleIndex)
	s.mux.HandleFunc("/project/", s.handleProject)
}

func (s *Server) handleProject(w http.ResponseWriter, r *http.Request) {
	// Parse /project/{name}[/{subpath}]
	path := strings.TrimPrefix(r.URL.Path, "/project/")
	parts := strings.SplitN(path, "/", 2)
	projectName := parts[0]
	subpath := ""
	if len(parts) > 1 {
		subpath = parts[1]
	}

	// Find project
	var project *discovery.Project
	for i := range s.projects {
		if s.projects[i].Name == projectName {
			project = &s.projects[i]
			break
		}
	}
	if project == nil {
		http.NotFound(w, r)
		return
	}

	files, err := discovery.ListThoughts(project.ThoughtsPath, subpath)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	data := struct {
		Project *discovery.Project
		Subpath string
		Files   []discovery.ThoughtFile
	}{
		Project: project,
		Subpath: subpath,
		Files:   files,
	}
	s.tmpl.ExecuteTemplate(w, "project.html", data)
}
```

#### 3. Project template
**File**: `templates/project.html`
```html
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{{.Project.Name}} - Birdseye</title>
    <style>
        * { box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 1000px;
            margin: 0 auto;
            padding: 20px;
            background: #1a1a2e;
            color: #eee;
        }
        a { color: #00d4ff; text-decoration: none; }
        a:hover { text-decoration: underline; }
        .breadcrumb { margin-bottom: 20px; color: #888; }
        .breadcrumb a { color: #888; }
        .files { list-style: none; padding: 0; }
        .file {
            padding: 12px 16px;
            background: #16213e;
            margin-bottom: 8px;
            border-radius: 6px;
            display: flex;
            justify-content: space-between;
            align-items: center;
        }
        .file:hover { background: #1a2744; }
        .file-name { font-weight: 500; }
        .file-time { color: #888; font-size: 0.85em; }
        .dir::before { content: "📁 "; }
        .md::before { content: "📄 "; }
    </style>
</head>
<body>
    <div class="breadcrumb">
        <a href="/">← All Projects</a>
        {{if .Subpath}}
        / <a href="/project/{{.Project.Name}}">{{.Project.Name}}</a>
        / {{.Subpath}}
        {{else}}
        / {{.Project.Name}}
        {{end}}
    </div>

    <h1>{{.Project.Name}}</h1>

    <ul class="files">
        {{range .Files}}
        <li class="file">
            {{if .IsDir}}
            <span class="file-name dir"><a href="/project/{{$.Project.Name}}/{{.Path}}">{{.Name}}/</a></span>
            {{else}}
            <span class="file-name md"><a href="/file/{{$.Project.Name}}/{{.Path}}">{{.Name}}</a></span>
            {{end}}
            <span class="file-time">{{.ModTime.Format "Jan 2, 2006 3:04 PM"}}</span>
        </li>
        {{end}}
    </ul>
</body>
</html>
```

### Success Criteria:

#### Automated Verification:
- [ ] `just build` compiles without errors
- [ ] `curl http://localhost:8080/project/android-register` returns HTML with file list

#### Manual Verification:
- [ ] Clicking a project from dashboard shows its thoughts/ contents
- [ ] Directories (shared/, research/, plans/) are clickable and navigate correctly
- [ ] Files show modification times
- [ ] Breadcrumb navigation works

**Implementation Note**: Pause for manual confirmation before Phase 3.

---

## Phase 3: Markdown Rendering

### Overview
Add the ability to view markdown files rendered as HTML with syntax highlighting for code blocks.

### Changes Required:

#### 1. Add markdown dependency
**File**: `go.mod` (add dependencies)
```
require (
    github.com/yuin/goldmark v1.6.0
    github.com/yuin/goldmark-highlighting/v2 v2.0.0-20230729083705-37449abec8cc
)
```

#### 2. File content handler
**File**: `internal/server/file.go`
```go
package server

import (
	"bytes"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/loganj/birdseye/internal/discovery"
	"github.com/yuin/goldmark"
	highlighting "github.com/yuin/goldmark-highlighting/v2"
	"github.com/yuin/goldmark/extension"
)

var md = goldmark.New(
	goldmark.WithExtensions(
		extension.GFM,
		highlighting.NewHighlighting(
			highlighting.WithStyle("dracula"),
		),
	),
)

func (s *Server) handleFile(w http.ResponseWriter, r *http.Request) {
	// Parse /file/{project}/{path}
	path := strings.TrimPrefix(r.URL.Path, "/file/")
	parts := strings.SplitN(path, "/", 2)
	if len(parts) < 2 {
		http.NotFound(w, r)
		return
	}
	projectName := parts[0]
	filePath := parts[1]

	// Find project
	var project *discovery.Project
	for i := range s.projects {
		if s.projects[i].Name == projectName {
			project = &s.projects[i]
			break
		}
	}
	if project == nil {
		http.NotFound(w, r)
		return
	}

	fullPath := filepath.Join(project.ThoughtsPath, filePath)
	content, err := os.ReadFile(fullPath)
	if err != nil {
		http.NotFound(w, r)
		return
	}

	var buf bytes.Buffer
	if err := md.Convert(content, &buf); err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	data := struct {
		Project  *discovery.Project
		FilePath string
		FileName string
		Content  string
		Raw      string
	}{
		Project:  project,
		FilePath: filePath,
		FileName: filepath.Base(filePath),
		Content:  buf.String(),
		Raw:      string(content),
	}
	s.tmpl.ExecuteTemplate(w, "file.html", data)
}
```

#### 3. Add route
**File**: `internal/server/server.go`
```go
func (s *Server) routes() {
	s.mux.HandleFunc("/", s.handleIndex)
	s.mux.HandleFunc("/project/", s.handleProject)
	s.mux.HandleFunc("/file/", s.handleFile)
}
```

#### 4. File view template
**File**: `templates/file.html`
```html
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{{.FileName}} - Birdseye</title>
    <style>
        * { box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 900px;
            margin: 0 auto;
            padding: 20px;
            background: #1a1a2e;
            color: #eee;
        }
        a { color: #00d4ff; text-decoration: none; }
        a:hover { text-decoration: underline; }
        .breadcrumb { margin-bottom: 20px; color: #888; }
        .breadcrumb a { color: #888; }
        .content {
            background: #16213e;
            padding: 24px 32px;
            border-radius: 8px;
            line-height: 1.7;
        }
        .content h1, .content h2, .content h3 { color: #00d4ff; }
        .content h1 { border-bottom: 1px solid #0f3460; padding-bottom: 8px; }
        .content code {
            background: #0f3460;
            padding: 2px 6px;
            border-radius: 4px;
            font-size: 0.9em;
        }
        .content pre {
            background: #0d1b2a;
            padding: 16px;
            border-radius: 6px;
            overflow-x: auto;
        }
        .content pre code {
            background: none;
            padding: 0;
        }
        .content ul, .content ol { padding-left: 24px; }
        .content li { margin-bottom: 4px; }
        .content blockquote {
            border-left: 3px solid #00d4ff;
            margin-left: 0;
            padding-left: 16px;
            color: #aaa;
        }
    </style>
</head>
<body>
    <div class="breadcrumb">
        <a href="/">← All Projects</a>
        / <a href="/project/{{.Project.Name}}">{{.Project.Name}}</a>
        / {{.FileName}}
    </div>

    <div class="content">
        {{.Content}}
    </div>
</body>
</html>
```

### Success Criteria:

#### Automated Verification:
- [ ] `go mod tidy` succeeds
- [ ] `just build` compiles without errors
- [ ] `curl http://localhost:8080/file/android-register/shared/plans/...` returns HTML

#### Manual Verification:
- [ ] Clicking a markdown file shows rendered content
- [ ] Code blocks have syntax highlighting
- [ ] Headers, lists, links render correctly
- [ ] YAML frontmatter renders (or is hidden gracefully)

**Implementation Note**: Pause for manual confirmation before Phase 4.

---

## Phase 4: Search

### Overview
Add full-text search across all thoughts files with results showing file name, project, and matching context.

### Changes Required:

#### 1. Search handler
**File**: `internal/server/search.go`
```go
package server

import (
	"bufio"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/loganj/birdseye/internal/discovery"
)

type SearchResult struct {
	Project  string
	FilePath string
	FileName string
	Line     int
	Context  string
}

func (s *Server) handleSearch(w http.ResponseWriter, r *http.Request) {
	query := strings.ToLower(strings.TrimSpace(r.URL.Query().Get("q")))
	if query == "" {
		s.tmpl.ExecuteTemplate(w, "search.html", nil)
		return
	}

	var results []SearchResult
	for _, project := range s.projects {
		filepath.Walk(project.ThoughtsPath, func(path string, info os.FileInfo, err error) error {
			if err != nil || info.IsDir() || !strings.HasSuffix(path, ".md") {
				return nil
			}

			file, err := os.Open(path)
			if err != nil {
				return nil
			}
			defer file.Close()

			relPath, _ := filepath.Rel(project.ThoughtsPath, path)
			scanner := bufio.NewScanner(file)
			lineNum := 0
			for scanner.Scan() {
				lineNum++
				line := scanner.Text()
				if strings.Contains(strings.ToLower(line), query) {
					results = append(results, SearchResult{
						Project:  project.Name,
						FilePath: relPath,
						FileName: filepath.Base(path),
						Line:     lineNum,
						Context:  truncate(line, 120),
					})
					if len(results) >= 100 {
						return filepath.SkipAll
					}
				}
			}
			return nil
		})
	}

	data := struct {
		Query   string
		Results []SearchResult
	}{
		Query:   query,
		Results: results,
	}
	s.tmpl.ExecuteTemplate(w, "search.html", data)
}

func truncate(s string, max int) string {
	if len(s) <= max {
		return s
	}
	return s[:max] + "..."
}
```

#### 2. Add route
```go
s.mux.HandleFunc("/search", s.handleSearch)
```

#### 3. Search template
**File**: `templates/search.html`
```html
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Search - Birdseye</title>
    <style>
        * { box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 900px;
            margin: 0 auto;
            padding: 20px;
            background: #1a1a2e;
            color: #eee;
        }
        a { color: #00d4ff; text-decoration: none; }
        a:hover { text-decoration: underline; }
        .breadcrumb { margin-bottom: 20px; color: #888; }
        .breadcrumb a { color: #888; }
        input[type="search"] {
            width: 100%;
            padding: 12px 16px;
            font-size: 1.1em;
            background: #16213e;
            border: 1px solid #0f3460;
            border-radius: 6px;
            color: #eee;
            margin-bottom: 20px;
        }
        input[type="search"]:focus {
            outline: none;
            border-color: #00d4ff;
        }
        .result {
            background: #16213e;
            padding: 12px 16px;
            margin-bottom: 8px;
            border-radius: 6px;
        }
        .result-header {
            font-size: 0.85em;
            color: #888;
            margin-bottom: 4px;
        }
        .result-context {
            font-family: monospace;
            font-size: 0.9em;
            color: #ccc;
        }
    </style>
</head>
<body>
    <div class="breadcrumb">
        <a href="/">← All Projects</a> / Search
    </div>

    <h1>Search</h1>
    <form method="GET" action="/search">
        <input type="search" name="q" placeholder="Search all thoughts..." value="{{.Query}}" autofocus>
    </form>

    {{if .Results}}
    <p>{{len .Results}} results</p>
    {{range .Results}}
    <div class="result">
        <div class="result-header">
            <a href="/file/{{.Project}}/{{.FilePath}}">{{.Project}}/{{.FilePath}}</a>
            :{{.Line}}
        </div>
        <div class="result-context">{{.Context}}</div>
    </div>
    {{end}}
    {{else if .Query}}
    <p>No results found.</p>
    {{end}}
</body>
</html>
```

#### 4. Add search link to index template
Add to `templates/index.html` after the `<h1>`:
```html
<p><a href="/search">🔍 Search all thoughts</a></p>
```

### Success Criteria:

#### Automated Verification:
- [ ] `just build` compiles without errors
- [ ] `curl "http://localhost:8080/search?q=test"` returns HTML with results

#### Manual Verification:
- [ ] Search page loads with empty query
- [ ] Searching returns relevant results across projects
- [ ] Clicking a result opens the file
- [ ] Performance is acceptable (<1s for typical queries)

**Implementation Note**: Pause for manual confirmation before Phase 5.

---

## Phase 5: Git Integration and Recent Files

### Overview
Add git branch/status to project cards and a global "recent files" view sorted by modification time.

### Changes Required:

#### 1. Git info in discovery
**File**: `internal/discovery/git.go`
```go
package discovery

import (
	"os/exec"
	"path/filepath"
	"strings"
)

type GitInfo struct {
	Branch string
	Dirty  bool
}

func GetGitInfo(projectPath string) *GitInfo {
	gitDir := filepath.Join(projectPath, ".git")
	// Check if it's a git repo
	cmd := exec.Command("git", "-C", projectPath, "rev-parse", "--git-dir")
	if err := cmd.Run(); err != nil {
		return nil
	}

	info := &GitInfo{}

	// Get branch name
	cmd = exec.Command("git", "-C", projectPath, "rev-parse", "--abbrev-ref", "HEAD")
	if out, err := cmd.Output(); err == nil {
		info.Branch = strings.TrimSpace(string(out))
	}

	// Check if dirty
	cmd = exec.Command("git", "-C", projectPath, "status", "--porcelain")
	if out, err := cmd.Output(); err == nil {
		info.Dirty = len(out) > 0
	}

	return info
}
```

#### 2. Update Project struct and discovery
```go
type Project struct {
	Name         string
	Path         string
	ThoughtsPath string
	Git          *GitInfo
}

// In FindProjects, after creating project:
project.Git = GetGitInfo(projectPath)
```

#### 3. Update index template with git info
```html
<div class="project">
    <h2><a href="/project/{{.Name}}">{{.Name}}</a></h2>
    {{if .Git}}
    <div class="git-info">
        <span class="branch">{{.Git.Branch}}</span>
        {{if .Git.Dirty}}<span class="dirty">*</span>{{end}}
    </div>
    {{end}}
</div>
```

Add CSS:
```css
.git-info { font-size: 0.85em; color: #888; }
.branch { color: #7ee787; }
.dirty { color: #f85149; }
```

#### 4. Recent files handler
**File**: `internal/server/recent.go`
```go
package server

import (
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/loganj/birdseye/internal/discovery"
)

type RecentFile struct {
	Project  string
	FilePath string
	FileName string
	ModTime  time.Time
}

func (s *Server) handleRecent(w http.ResponseWriter, r *http.Request) {
	var files []RecentFile

	for _, project := range s.projects {
		filepath.Walk(project.ThoughtsPath, func(path string, info os.FileInfo, err error) error {
			if err != nil || info.IsDir() || !strings.HasSuffix(path, ".md") {
				return nil
			}
			relPath, _ := filepath.Rel(project.ThoughtsPath, path)
			files = append(files, RecentFile{
				Project:  project.Name,
				FilePath: relPath,
				FileName: filepath.Base(path),
				ModTime:  info.ModTime(),
			})
			return nil
		})
	}

	sort.Slice(files, func(i, j int) bool {
		return files[i].ModTime.After(files[j].ModTime)
	})

	// Limit to 50 most recent
	if len(files) > 50 {
		files = files[:50]
	}

	data := struct {
		Files []RecentFile
	}{
		Files: files,
	}
	s.tmpl.ExecuteTemplate(w, "recent.html", data)
}
```

#### 5. Add route and recent template
```go
s.mux.HandleFunc("/recent", s.handleRecent)
```

**File**: `templates/recent.html`
```html
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Recent Files - Birdseye</title>
    <style>
        /* Same base styles */
        * { box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 900px;
            margin: 0 auto;
            padding: 20px;
            background: #1a1a2e;
            color: #eee;
        }
        a { color: #00d4ff; text-decoration: none; }
        a:hover { text-decoration: underline; }
        .breadcrumb { margin-bottom: 20px; color: #888; }
        .breadcrumb a { color: #888; }
        .file {
            padding: 12px 16px;
            background: #16213e;
            margin-bottom: 8px;
            border-radius: 6px;
            display: flex;
            justify-content: space-between;
            align-items: center;
        }
        .file:hover { background: #1a2744; }
        .file-project { color: #888; font-size: 0.85em; }
        .file-time { color: #888; font-size: 0.85em; }
    </style>
</head>
<body>
    <div class="breadcrumb">
        <a href="/">← All Projects</a> / Recent
    </div>

    <h1>Recent Files</h1>

    {{range .Files}}
    <div class="file">
        <div>
            <a href="/file/{{.Project}}/{{.FilePath}}">{{.FileName}}</a>
            <div class="file-project">{{.Project}}</div>
        </div>
        <span class="file-time">{{.ModTime.Format "Jan 2, 2006 3:04 PM"}}</span>
    </div>
    {{end}}
</body>
</html>
```

#### 6. Update index with recent link
```html
<p>
    <a href="/search">🔍 Search</a> |
    <a href="/recent">🕐 Recent files</a>
</p>
```

### Success Criteria:

#### Automated Verification:
- [ ] `just build` compiles without errors
- [ ] `curl http://localhost:8080/recent` returns HTML with file list

#### Manual Verification:
- [ ] Project cards show git branch names
- [ ] Dirty repos show asterisk indicator
- [ ] Recent files page shows files sorted by modification time
- [ ] Navigation between views works smoothly

**Implementation Note**: Pause for manual confirmation.

---

## Phase 6: Polish and Embed

### Overview
Embed templates into the binary so it's truly self-contained, add file counts to project cards, and improve navigation.

### Changes Required:

#### 1. Embed templates
**File**: `templates/templates.go`
```go
package templates

import "embed"

//go:embed *.html
var FS embed.FS
```

#### 2. Update server to use embedded templates
```go
import "github.com/loganj/birdseye/templates"

func New(root string, projects []discovery.Project) *Server {
	s := &Server{
		root:     root,
		projects: projects,
		mux:      http.NewServeMux(),
	}
	s.tmpl = template.Must(template.ParseFS(templates.FS, "*.html"))
	s.routes()
	return s
}
```

#### 3. Add file counts to projects
Update discovery to count files:
```go
type Project struct {
	Name         string
	Path         string
	ThoughtsPath string
	Git          *GitInfo
	FileCount    int
}

func countMdFiles(dir string) int {
	count := 0
	filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		if err == nil && !info.IsDir() && strings.HasSuffix(path, ".md") {
			count++
		}
		return nil
	})
	return count
}
```

#### 4. Update index template
```html
<div class="project">
    <h2><a href="/project/{{.Name}}">{{.Name}}</a></h2>
    <div class="project-meta">
        {{if .Git}}<span class="branch">{{.Git.Branch}}{{if .Git.Dirty}}*{{end}}</span> · {{end}}
        {{.FileCount}} files
    </div>
</div>
```

### Success Criteria:

#### Automated Verification:
- [ ] `just build` produces single binary
- [ ] Binary runs without templates/ directory present
- [ ] `./birdseye` works from any directory

#### Manual Verification:
- [ ] Project cards show file counts
- [ ] All features work with embedded templates
- [ ] Binary can be moved/copied and still works

---

## Testing Strategy

### Unit Tests:
- `discovery.FindProjects` finds correct projects
- `discovery.ListThoughts` returns files sorted correctly
- `discovery.GetGitInfo` handles non-git directories gracefully

### Integration Tests:
- Start server, verify all routes return 200
- Search returns expected results for known content

### Manual Testing Steps:
1. `just run` and verify dashboard loads
2. Click through to a project, into subdirectories, into files
3. Search for a known term and verify results
4. Check Recent files view
5. Move binary to /tmp, run it, verify it works

## Future Enhancements (Out of Scope)

- File watching for auto-refresh
- Keyboard navigation
- Mark files as read/reviewed
- RSS/Atom feed of changes
- File editing
- Dark/light theme toggle

## References

- Go embed: https://pkg.go.dev/embed
- Goldmark: https://github.com/yuin/goldmark
- htmx: https://htmx.org (if we add interactivity later)
