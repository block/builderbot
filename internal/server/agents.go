package server

import (
	"encoding/json"
	"log"
	"net/http"
)

// handleAgentStatus handles GET /api/agents?project=X.
func (s *Server) handleAgentStatus(w http.ResponseWriter, r *http.Request) {
	projectName := r.URL.Query().Get("project")
	if projectName == "" {
		http.Error(w, "missing project parameter", http.StatusBadRequest)
		return
	}

	if s.agents == nil {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"running": false,
			"project": projectName,
		})
		return
	}

	status := s.agents.Status(projectName)
	if status == nil {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"running": false,
			"project": projectName,
		})
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(status)
}

// handleAgentStart handles POST /api/agents/start?project=X.
func (s *Server) handleAgentStart(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	projectName := r.URL.Query().Get("project")
	if projectName == "" {
		http.Error(w, "missing project parameter", http.StatusBadRequest)
		return
	}

	if s.agents == nil {
		http.Error(w, "agent manager not available", http.StatusServiceUnavailable)
		return
	}

	agent, err := s.agents.Start(projectName)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	if agent == nil {
		// Already running
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(s.agents.Status(projectName))
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(s.agents.Status(projectName))
}

// handleAgentStop handles POST /api/agents/stop?project=X.
func (s *Server) handleAgentStop(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	projectName := r.URL.Query().Get("project")
	if projectName == "" {
		http.Error(w, "missing project parameter", http.StatusBadRequest)
		return
	}

	if s.agents == nil {
		http.Error(w, "agent manager not available", http.StatusServiceUnavailable)
		return
	}

	if err := s.agents.Stop(projectName); err != nil {
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]bool{"ok": true})
}

// maybeStartAgent launches an agent for the project if:
// 1. The comment is from a human
// 2. No agent is already running for this project
func (s *Server) maybeStartAgent(projectName, role string) {
	if role != "human" || s.agents == nil {
		return
	}
	go func() {
		if _, err := s.agents.Start(projectName); err != nil {
			log.Printf("Auto-start agent for %s: %v", projectName, err)
		}
	}()
}
