package server

import (
	"fmt"
	"html/template"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/loganj/birdseye/internal/discovery"
	"github.com/loganj/birdseye/templates"
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

	// Parse templates from embedded filesystem
	s.tmpl = template.Must(template.ParseFS(templates.FS, "*.html"))

	s.routes()
	return s
}

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	s.mux.ServeHTTP(w, r)
}

func (s *Server) routes() {
	s.mux.HandleFunc("/", s.handleIndex)
	s.mux.HandleFunc("/project/", s.handleProject)
	s.mux.HandleFunc("/file/", s.handleFile)
	s.mux.HandleFunc("/search", s.handleSearch)
	s.mux.HandleFunc("/recent", s.handleRecent)
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

	// Collect all files
	var files []IndexFile
	for _, project := range s.projects {
		filepath.Walk(project.ThoughtsPath, func(path string, info os.FileInfo, err error) error {
			if err != nil || info.IsDir() || !strings.HasSuffix(path, ".md") {
				return nil
			}
			relPath, _ := filepath.Rel(project.ThoughtsPath, path)
			files = append(files, IndexFile{
				Project:  project.Name,
				FilePath: relPath,
				FileName: filepath.Base(path),
				ModTime:  info.ModTime(),
				Age:      formatAge(info.ModTime()),
			})
			return nil
		})
	}

	// Sort by modification time
	sort.Slice(files, func(i, j int) bool {
		return files[i].ModTime.After(files[j].ModTime)
	})

	// Limit to 100 most recent
	if len(files) > 100 {
		files = files[:100]
	}

	// Sort projects by last modified
	sortedProjects := make([]discovery.Project, len(s.projects))
	copy(sortedProjects, s.projects)
	sort.Slice(sortedProjects, func(i, j int) bool {
		return sortedProjects[i].LastModified.After(sortedProjects[j].LastModified)
	})

	data := struct {
		Projects []discovery.Project
		Files    []IndexFile
	}{
		Projects: sortedProjects,
		Files:    files,
	}
	s.tmpl.ExecuteTemplate(w, "index.html", data)
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

	// Build breadcrumb parts
	var breadcrumbs []struct {
		Name string
		Path string
	}
	if subpath != "" {
		pathParts := strings.Split(subpath, "/")
		currentPath := ""
		for _, part := range pathParts {
			if currentPath == "" {
				currentPath = part
			} else {
				currentPath = currentPath + "/" + part
			}
			breadcrumbs = append(breadcrumbs, struct {
				Name string
				Path string
			}{
				Name: part,
				Path: currentPath,
			})
		}
	}

	data := struct {
		Project     *discovery.Project
		Subpath     string
		Files       []discovery.ThoughtFile
		Breadcrumbs []struct {
			Name string
			Path string
		}
	}{
		Project:     project,
		Subpath:     subpath,
		Files:       files,
		Breadcrumbs: breadcrumbs,
	}
	s.tmpl.ExecuteTemplate(w, "project.html", data)
}
