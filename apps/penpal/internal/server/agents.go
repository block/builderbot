package server

import (
	"bytes"
	"context"
	"encoding/json"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"time"

	"github.com/loganj/penpal/internal/agents"
	"github.com/loganj/penpal/internal/comments"
)

// agentStatusResponse wraps AgentStatus with server-level fields.
type agentStatusResponse struct {
	*agents.AgentStatus
	NeedsAgent bool `json:"needsAgent,omitempty"`
}

// handleAgentStatus handles GET /api/agents?project=X.
// E-PENPAL-API-ROUTES: GET /api/agents endpoint.
// E-PENPAL-AGENT-ACTIVE-UNIFIED: checks both spawned agents and CLI sessions.
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

	// If no spawned agent is running, check for an external CLI session.
	if !status.Running && s.isAgentActive(projectName) {
		status.Running = true
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

	// E-PENPAL-CLI-CONTENTION: stop both spawned and CLI-attached agents.
	s.agents.StopAny(projectName)

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]bool{"ok": true})
}

// maybeStartAgent launches an agent for the project if:
// 1. The comment is from a human
// 2. No agent is already running for this project
//
// E-PENPAL-AGENT-AUTOSTART: maybeStartAgent after handleCreateThread/handleAddComment.
// E-PENPAL-CLI-CONTENTION: skips auto-start when an external CLI agent is attached.
func (s *Server) maybeStartAgent(projectName, role string) {
	if role != "human" || s.agents == nil {
		return
	}
	if s.agents.HasActiveAgent(projectName) {
		return
	}
	go func() {
		if _, err := s.agents.Start(projectName); err != nil {
			log.Printf("Auto-start agent for %s: %v", projectName, err)
		}
	}()
}

// handleAgentAttach handles POST /api/agents/attach.
// E-PENPAL-CLI-ATTACH: resolves path to project and creates an external agent session.
func (s *Server) handleAgentAttach(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	var req struct {
		Path  string `json:"path"`
		Force bool   `json:"force"`
		Agent string `json:"agent"` // self-reported agent name (e.g., "amp", "claude")
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid JSON: "+err.Error(), http.StatusBadRequest)
		return
	}

	if req.Path == "" {
		http.Error(w, "path is required", http.StatusBadRequest)
		return
	}

	absPath, err := filepath.Abs(req.Path)
	if err != nil {
		http.Error(w, "invalid path: "+err.Error(), http.StatusBadRequest)
		return
	}

	if _, err := os.Stat(absPath); err != nil {
		http.Error(w, "path not found: "+absPath, http.StatusBadRequest)
		return
	}

	project, worktree := s.cache.FindProjectByPathWithWorktree(absPath)
	if project == nil {
		http.Error(w, "no project found for path: "+absPath, http.StatusNotFound)
		return
	}

	if s.agents == nil {
		http.Error(w, "agent manager not available", http.StatusServiceUnavailable)
		return
	}

	projectQN := project.QualifiedName()
	agentName := req.Agent
	if agentName == "" {
		agentName = "agent"
	}
	// E-PENPAL-AGENT-SELF-ID: pass agent name from request to session.
	sess, err := s.agents.Attach(projectQN, worktree, agentName, req.Force)
	if err != nil {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusConflict)
		json.NewEncoder(w).Encode(map[string]string{"error": err.Error()})
		return
	}

	// Trigger the open flow in the background so the file appears in the UI.
	go func() {
		openBody, _ := json.Marshal(map[string]string{"path": absPath})
		openReq, _ := http.NewRequest(http.MethodPost, "/api/open", bytes.NewReader(openBody))
		openReq.Header.Set("Content-Type", "application/json")
		dw := &discardResponseWriter{}
		s.handleAPIOpen(dw, openReq)
		if dw.statusCode >= 400 {
			log.Printf("Warning: open flow failed for %s (status %d)", absPath, dw.statusCode)
		}
	}()

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]string{
		"project":      projectQN,
		"worktree":     sess.Worktree,
		"sessionToken": sess.Token,
		"agentName":    sess.AgentName,
	})
}

// discardResponseWriter is a no-op http.ResponseWriter used when we need to
// call resolveOpenDirectory/resolveOpenFile for side effects only.
type discardResponseWriter struct {
	header     http.Header
	statusCode int
}

func (d *discardResponseWriter) Header() http.Header {
	if d.header == nil {
		d.header = make(http.Header)
	}
	return d.header
}
func (d *discardResponseWriter) Write(b []byte) (int, error) { return len(b), nil }
func (d *discardResponseWriter) WriteHeader(code int)         { d.statusCode = code }

// handleAgentWait handles GET /api/agents/wait?project=X&session=T&sinceSeq=N&worktree=W.
// E-PENPAL-CLI-AGENT-CMDS: long-poll endpoint for CLI agents, mirrors MCP penpal_wait_for_changes.
func (s *Server) handleAgentWait(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	projectName := r.URL.Query().Get("project")
	sessionToken := r.URL.Query().Get("session")
	worktree := r.URL.Query().Get("worktree")

	if projectName == "" || sessionToken == "" {
		http.Error(w, "missing project or session parameter", http.StatusBadRequest)
		return
	}

	if s.agents == nil {
		http.Error(w, "agent manager not available", http.StatusServiceUnavailable)
		return
	}

	// Validate session and enforce project ownership.
	// E-PENPAL-SESSION-MGMT: validates token and project scoping before processing.
	sess, err := s.agents.ValidateSession(sessionToken)
	if err != nil {
		http.Error(w, err.Error(), http.StatusUnauthorized)
		return
	}
	if sess.Project != projectName {
		http.Error(w, "session does not own this project", http.StatusForbidden)
		return
	}

	// Record heartbeat at start of wait.
	s.agents.RecordSessionHeartbeat(sessionToken)

	// Use the session's worktree as the authoritative value.
	worktree = sess.Worktree

	sinceSeq := uint64(0)
	if v := r.URL.Query().Get("sinceSeq"); v != "" {
		if n, err := strconv.ParseUint(v, 10, 64); err == nil {
			sinceSeq = n
		}
	}

	// Long-poll: wait up to 30s for changes.
	// E-PENPAL-CLI-AGENT-CMDS: uses WaitAndEnrich like MCP penpal_wait_for_changes.
	waitCtx, cancel := context.WithTimeout(r.Context(), 30*time.Second)
	defer cancel()

	var result *comments.WaitResult
	result, err = s.comments.WaitAndEnrich(waitCtx, projectName, worktree, sinceSeq)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	// Record heartbeat after waking.
	s.agents.RecordSessionHeartbeat(sessionToken)

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(result)
}

// validateSessionParam checks the "session" query parameter if present.
// When a session token is provided, it validates the token and verifies that
// the session's project matches the "project" query parameter on the request.
// Returns true if the request should continue, false if an error was written.
// E-PENPAL-SESSION-MGMT: session validation helper for CLI agent requests.
func (s *Server) validateSessionParam(w http.ResponseWriter, r *http.Request) bool {
	sessionToken := r.URL.Query().Get("session")
	if sessionToken == "" {
		return true // no session param — not a CLI agent request
	}
	if s.agents == nil {
		http.Error(w, "agent manager not available", http.StatusServiceUnavailable)
		return false
	}
	sess, err := s.agents.ValidateSession(sessionToken)
	if err != nil {
		http.Error(w, err.Error(), http.StatusUnauthorized)
		return false
	}
	// Enforce project and worktree ownership: the session must own what's being accessed.
	if projectName := r.URL.Query().Get("project"); projectName != "" && sess.Project != projectName {
		http.Error(w, "session does not own this project", http.StatusForbidden)
		return false
	}
	if wt := r.URL.Query().Get("worktree"); wt != "" && sess.Worktree != "" && wt != sess.Worktree {
		http.Error(w, "session does not own this worktree", http.StatusForbidden)
		return false
	}
	s.agents.RecordSessionHeartbeat(sessionToken)
	return true
}

// requireSessionForAgent checks that a valid session exists for agent-role
// writes via REST. Returns true if the request may proceed. If no session is
// provided or the session is invalid/mismatched, writes an HTTP error and
// returns false.
// E-PENPAL-CLI-CONTENTION: prevents session-less agent writes via REST.
func (s *Server) requireSessionForAgent(w http.ResponseWriter, r *http.Request, project string) bool {
	sessionToken := r.URL.Query().Get("session")
	if sessionToken == "" {
		http.Error(w, "agent-role requests require a session token", http.StatusUnauthorized)
		return false
	}
	if s.agents == nil {
		http.Error(w, "agent manager not available", http.StatusServiceUnavailable)
		return false
	}
	sess, err := s.agents.ValidateSession(sessionToken)
	if err != nil {
		http.Error(w, err.Error(), http.StatusUnauthorized)
		return false
	}
	if sess.Project != project {
		http.Error(w, "session does not own this project", http.StatusForbidden)
		return false
	}
	return true
}

// agentNameFromSession returns the agent name from the session identified by the
// "session" query parameter. Returns empty string if no session is present or invalid.
// E-PENPAL-AGENT-SELF-ID: derives comment author from session.
func (s *Server) agentNameFromSession(r *http.Request) string {
	sessionToken := r.URL.Query().Get("session")
	if sessionToken == "" || s.agents == nil {
		return ""
	}
	sess, err := s.agents.ValidateSession(sessionToken)
	if err != nil {
		return ""
	}
	return sess.AgentName
}

// isAgentActive returns true if any agent (spawned or CLI-attached) is active for the project.
// E-PENPAL-AGENT-ACTIVE-UNIFIED: unified agent presence check.
func (s *Server) isAgentActive(projectName string) bool {
	if s.agents == nil {
		return false
	}
	return s.agents.HasActiveAgent(projectName)
}

