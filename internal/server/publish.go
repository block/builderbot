package server

import (
	"encoding/json"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"time"

	"github.com/loganj/birdseye/internal/publish"
)

func (s *Server) handlePublish(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	var input struct {
		Project string `json:"project"`
		Path    string `json:"path"`
	}
	if err := json.NewDecoder(r.Body).Decode(&input); err != nil {
		http.Error(w, "invalid request body", http.StatusBadRequest)
		return
	}
	if input.Project == "" || input.Path == "" {
		http.Error(w, "project and path are required", http.StatusBadRequest)
		return
	}

	project := s.cache.FindProject(input.Project)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	fullPath := filepath.Join(project.Path, input.Path)
	content, err := os.ReadFile(fullPath)
	if err != nil {
		http.Error(w, "file not found", http.StatusNotFound)
		return
	}

	siteName := publish.GenerateSiteName(input.Project, input.Path)
	title := filepath.Base(input.Path)

	result, err := publish.Publish(content, title, siteName, "")
	if err != nil {
		log.Printf("Publish to Blockcell failed: %v", err)
		http.Error(w, "publish failed: "+err.Error(), http.StatusInternalServerError)
		return
	}

	// Save publish state
	if err := publish.SaveState(project.Path, input.Path, &publish.PublishState{
		SiteName:      siteName,
		URL:           result.URL,
		LastPublished: time.Now(),
	}); err != nil {
		log.Printf("Warning: could not save publish state: %v", err)
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]string{
		"url":      result.URL,
		"siteName": siteName,
	})
}

func (s *Server) handlePublishState(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	projectName := r.URL.Query().Get("project")
	filePath := r.URL.Query().Get("path")
	if projectName == "" || filePath == "" {
		http.Error(w, "project and path are required", http.StatusBadRequest)
		return
	}

	project := s.cache.FindProject(projectName)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	state, err := publish.LoadState(project.Path)
	if err != nil {
		http.Error(w, "failed to load state", http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	entry := state[filePath]
	if entry == nil {
		json.NewEncoder(w).Encode(map[string]interface{}{})
	} else {
		json.NewEncoder(w).Encode(entry)
	}
}
