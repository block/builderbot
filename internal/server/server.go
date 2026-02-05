package server

import (
	"html/template"
	"net/http"
	"strings"

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

func (s *Server) handleIndex(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}

	data := struct {
		Projects []discovery.Project
	}{
		Projects: s.projects,
	}
	s.tmpl.ExecuteTemplate(w, "index.html", data)
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
