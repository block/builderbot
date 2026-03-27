package server

import (
	"encoding/json"
	"log"
	"net/http"

	"github.com/loganj/penpal/internal/agents"
)

// agentStatusResponse wraps AgentStatus with server-level fields.
type agentStatusResponse struct {
	*agents.AgentStatus
	NeedsAgent bool `json:"needsAgent,omitempty"`
}

// handleAgentStatus handles GET /api/agents?project=X.
// E-PENPAL-API-ROUTES: GET /api/agents endpoint.
func (s *Server) handleAgentStatus(w http.ResponseWriter, r *http.Request) {
	projectName := r.URL.Query().Get("project")
	if projectName == "" {
		http.Error(w, "missing project parameter", http.StatusBadRequest)
		return
	}

	var status *agents.AgentStatus
	if s.agents != nil {
		status = s.agents.Status(projectName)
	}
	if status == nil {
		status = &agents.AgentStatus{
			Project: projectName,
			Running: false,
		}
	}

	resp := agentStatusResponse{AgentStatus: status}
	if !status.Running && s.comments != nil && s.comments.HasPendingHumanComments(projectName) {
		resp.NeedsAgent = true
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(resp)
}

// handleAgentStart handles POST /api/agents/start?project=X.
// E-PENPAL-API-ROUTES: POST /api/agents/start endpoint.
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
// E-PENPAL-API-ROUTES: POST /api/agents/stop endpoint.
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
//
// E-PENPAL-AGENT-AUTOSTART: maybeStartAgent after handleCreateThread/handleAddComment.
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
