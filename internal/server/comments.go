package server

import (
	"encoding/json"
	"net/http"
	"strings"

	"github.com/loganj/birdseye/internal/comments"
	"github.com/loganj/birdseye/internal/watcher"
)

// handleAPIThreads dispatches GET (list threads) and POST (create thread).
func (s *Server) handleAPIThreads(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		s.handleListThreads(w, r)
	case http.MethodPost:
		s.handleCreateThread(w, r)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

// handleAPIThreadAction handles /api/threads/{id} and /api/threads/{id}/comments.
func (s *Server) handleAPIThreadAction(w http.ResponseWriter, r *http.Request) {
	// Parse the path: /api/threads/{id} or /api/threads/{id}/comments
	rest := strings.TrimPrefix(r.URL.Path, "/api/threads/")
	parts := strings.Split(rest, "/")

	if len(parts) == 0 || parts[0] == "" {
		http.Error(w, "missing thread ID", http.StatusBadRequest)
		return
	}

	threadID := parts[0]

	if len(parts) == 2 && parts[1] == "comments" {
		// POST /api/threads/{id}/comments
		if r.Method != http.MethodPost {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		s.handleAddComment(w, r, threadID)
		return
	}

	if len(parts) == 1 {
		// PATCH /api/threads/{id}
		if r.Method != http.MethodPatch {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		s.handleUpdateThread(w, r, threadID)
		return
	}

	http.NotFound(w, r)
}

// APIFileInReview extends FileInReview with agent activity status.
type APIFileInReview struct {
	FilePath      string `json:"filePath"`
	OpenThreads   int    `json:"openThreads"`
	AgentActive   bool   `json:"agentActive"`
	TypingThreads int    `json:"typingThreads,omitempty"`
}

// handleAPIListReviews handles GET /api/reviews?project=X[&agent=true].
func (s *Server) handleAPIListReviews(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	projectName := r.URL.Query().Get("project")
	if projectName == "" {
		http.Error(w, "missing project parameter", http.StatusBadRequest)
		return
	}

	isAgent := r.URL.Query().Get("agent") == "true"

	files, err := s.comments.ListFilesInReview(projectName)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	// Record heartbeat when an agent polls reviews
	if isAgent {
		for _, f := range files {
			s.comments.RecordHeartbeat(projectName, f.FilePath)
		}
	}

	agentActive := s.agents != nil && s.agents.Status(projectName) != nil && s.agents.Status(projectName).Running
	result := make([]APIFileInReview, len(files))
	for i, f := range files {
		typingThreads := s.comments.TypingCount(projectName, f.FilePath)
		if agentActive && typingThreads == 0 {
			// Agent is running—typing entries survive beyond 60s timeout
			typingThreads = s.comments.TypingCountNoExpiry(projectName, f.FilePath)
		}
		result[i] = APIFileInReview{
			FilePath:      f.FilePath,
			OpenThreads:   f.OpenThreads,
			AgentActive:   agentActive,
			TypingThreads: typingThreads,
		}
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(result)
}

// threadResponse wraps a Thread with ephemeral UI state.
type threadResponse struct {
	comments.Thread
	AgentTyping bool `json:"agentTyping,omitempty"`
}

// handleListThreads handles GET /api/threads?project=X&path=Y[&status=open][&agent=true].
// Paths are project-relative (e.g., "thoughts/plans/foo.md").
func (s *Server) handleListThreads(w http.ResponseWriter, r *http.Request) {
	projectName := r.URL.Query().Get("project")
	if projectName == "" {
		http.Error(w, "missing project parameter", http.StatusBadRequest)
		return
	}

	filePath := r.URL.Query().Get("path")
	status := r.URL.Query().Get("status")
	isAgent := r.URL.Query().Get("agent") == "true"

	// Record heartbeat when an agent polls for threads
	if isAgent && filePath != "" {
		s.comments.RecordHeartbeat(projectName, filePath)
	}

	// When path is omitted, return all open threads across the project
	if filePath == "" {
		threads, err := s.comments.ListOpenThreads(projectName)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		if threads == nil {
			threads = []comments.ThreadWithFile{}
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(threads)
		return
	}

	threads, err := s.comments.LoadThreads(projectName, filePath)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	// Filter by status if requested
	if status != "" {
		var filtered []comments.Thread
		for _, t := range threads {
			if t.Status == status {
				filtered = append(filtered, t)
			}
		}
		threads = filtered
	}

	agentRunning := s.agents != nil && s.agents.Status(projectName) != nil && s.agents.Status(projectName).Running
	var result []threadResponse
	for _, t := range threads {
		tr := threadResponse{Thread: t}
		if s.comments.IsTyping(projectName, filePath, t.ID) {
			tr.AgentTyping = true
		} else if agentRunning && s.comments.HasTypingEntry(projectName, filePath, t.ID) {
			// Agent is running—don't let the 60s timeout hide the indicator
			tr.AgentTyping = true
		}
		result = append(result, tr)
	}

	if result == nil {
		result = []threadResponse{}
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(result)
}

// handleCreateThread handles POST /api/threads.
func (s *Server) handleCreateThread(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Project          string          `json:"project"`
		Path             string          `json:"path"`
		Anchor           comments.Anchor `json:"anchor"`
		Author           string          `json:"author"`
		Role             string          `json:"role"`
		Body             string          `json:"body"`
		SuggestedReplies []string        `json:"suggestedReplies,omitempty"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid JSON: "+err.Error(), http.StatusBadRequest)
		return
	}

	if req.Project == "" || req.Path == "" || req.Author == "" || req.Body == "" {
		http.Error(w, "missing required fields (project, path, author, body)", http.StatusBadRequest)
		return
	}

	comment := comments.Comment{
		Author:           req.Author,
		Role:             req.Role,
		Body:             req.Body,
		SuggestedReplies: req.SuggestedReplies,
	}

	thread, err := s.comments.CreateThread(req.Project, req.Path, req.Anchor, comment)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	s.watcher.Broadcast(watcher.Event{Type: watcher.EventCommentsChanged, Project: req.Project})
	s.maybeStartAgent(req.Project, req.Role)

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(thread)
}

// handleAddComment handles POST /api/threads/{id}/comments.
func (s *Server) handleAddComment(w http.ResponseWriter, r *http.Request, threadID string) {
	var req struct {
		Project          string   `json:"project"`
		Path             string   `json:"path"`
		Author           string   `json:"author"`
		Role             string   `json:"role"`
		Body             string   `json:"body"`
		SuggestedReplies []string `json:"suggestedReplies,omitempty"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid JSON: "+err.Error(), http.StatusBadRequest)
		return
	}

	if req.Project == "" || req.Path == "" || req.Author == "" || req.Body == "" {
		http.Error(w, "missing required fields (project, path, author, body)", http.StatusBadRequest)
		return
	}

	comment := comments.Comment{
		Author:           req.Author,
		Role:             req.Role,
		Body:             req.Body,
		SuggestedReplies: req.SuggestedReplies,
	}

	thread, err := s.comments.AddComment(req.Project, req.Path, threadID, comment)
	if err != nil {
		if strings.Contains(err.Error(), "not found") {
			http.Error(w, err.Error(), http.StatusNotFound)
			return
		}
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	s.watcher.Broadcast(watcher.Event{Type: watcher.EventCommentsChanged, Project: req.Project})
	s.maybeStartAgent(req.Project, req.Role)

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(thread)
}

// handleUpdateThread handles PATCH /api/threads/{id} (resolve or reopen).
func (s *Server) handleUpdateThread(w http.ResponseWriter, r *http.Request, threadID string) {
	var req struct {
		Project    string `json:"project"`
		Path       string `json:"path"`
		Status     string `json:"status"`
		ResolvedBy string `json:"resolvedBy"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid JSON: "+err.Error(), http.StatusBadRequest)
		return
	}

	if req.Project == "" || req.Path == "" || req.Status == "" {
		http.Error(w, "missing required fields (project, path, status)", http.StatusBadRequest)
		return
	}

	var err error
	switch req.Status {
	case "resolved":
		err = s.comments.ResolveThread(req.Project, req.Path, threadID, req.ResolvedBy)
	case "open":
		err = s.comments.ReopenThread(req.Project, req.Path, threadID)
	default:
		http.Error(w, "invalid status: must be 'resolved' or 'open'", http.StatusBadRequest)
		return
	}

	if err != nil {
		if strings.Contains(err.Error(), "not found") {
			http.Error(w, err.Error(), http.StatusNotFound)
			return
		}
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	s.watcher.Broadcast(watcher.Event{Type: watcher.EventCommentsChanged, Project: req.Project})

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]bool{"ok": true})
}
