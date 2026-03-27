package server

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/loganj/penpal/internal/agents"
	"github.com/loganj/penpal/internal/comments"
)

// E-PENPAL-API-ROUTES: verifies GET /api/agents returns running=false with no pending comments.
func TestAPIAgentStatus_NoAgent_NoPendingComments(t *testing.T) {
	s, c, _ := testServer(t)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	req := httptest.NewRequest(http.MethodGet, "/api/agents?project=test-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var resp map[string]interface{}
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse: %v", err)
	}

	if resp["running"] != false {
		t.Errorf("expected running=false")
	}
	if _, ok := resp["needsAgent"]; ok {
		t.Errorf("expected no needsAgent field when no pending comments, got %v", resp["needsAgent"])
	}
}

// E-PENPAL-AGENT-AUTOSTART: verifies needsAgent=true when human comment is pending.
func TestAPIAgentStatus_NeedsAgent_WithPendingHumanComments(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create a thread directly via comments store (bypasses maybeStartAgent)
	anchor := comments.Anchor{SelectedText: "some text"}
	comment := comments.Comment{Author: "user", Role: "human", Body: "Please review"}
	_, err := cs.CreateThread("test-proj", "thoughts/plan.md", anchor, comment)
	if err != nil {
		t.Fatalf("create thread: %v", err)
	}

	// Check agent status — should include needsAgent since agent is not running
	req := httptest.NewRequest(http.MethodGet, "/api/agents?project=test-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var resp map[string]interface{}
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse: %v", err)
	}

	if resp["running"] != false {
		t.Errorf("expected running=false, got %v", resp["running"])
	}
	if resp["needsAgent"] != true {
		t.Errorf("expected needsAgent=true, got %v", resp["needsAgent"])
	}
}

// E-PENPAL-AGENT-AUTOSTART: verifies needsAgent absent after agent replies.
func TestAPIAgentStatus_NoNeedsAgent_WhenAgentReplied(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create a thread with human comment directly
	anchor := comments.Anchor{SelectedText: "text"}
	comment := comments.Comment{Author: "user", Role: "human", Body: "Question"}
	thread, err := cs.CreateThread("test-proj", "thoughts/plan.md", anchor, comment)
	if err != nil {
		t.Fatalf("create thread: %v", err)
	}

	// Add an agent reply so the last comment is from agent
	replyBody, _ := json.Marshal(map[string]string{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"author":  "bot",
		"role":    "agent",
		"body":    "Done",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads/"+thread.ID+"/comments", bytes.NewReader(replyBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("add comment: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	// Check agent status — should NOT include needsAgent since agent already replied
	req = httptest.NewRequest(http.MethodGet, "/api/agents?project=test-proj", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	var resp map[string]interface{}
	json.Unmarshal(rec.Body.Bytes(), &resp)

	if _, ok := resp["needsAgent"]; ok {
		t.Errorf("expected no needsAgent when agent already replied, got %v", resp["needsAgent"])
	}
}

// E-PENPAL-AGENT-AUTOSTART: verifies needsAgent=true after agent finishes with pending comments.
func TestAPIAgentStatus_NeedsAgent_AfterAgentFinished(t *testing.T) {
	s, c, cs := testServer(t)
	s.agents = agents.New(c, cs, 0)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Insert a synthetic agent entry that has already finished.
	s.agents.SimulateFinished("test-proj")

	// Create a pending human comment
	anchor := comments.Anchor{SelectedText: "review me"}
	comment := comments.Comment{Author: "user", Role: "human", Body: "New comment"}
	_, err := cs.CreateThread("test-proj", "thoughts/plan.md", anchor, comment)
	if err != nil {
		t.Fatalf("create thread: %v", err)
	}

	// Agent status should still show needsAgent even though a previous agent exists
	req := httptest.NewRequest(http.MethodGet, "/api/agents?project=test-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var resp map[string]interface{}
	json.Unmarshal(rec.Body.Bytes(), &resp)

	if resp["running"] != false {
		t.Errorf("expected running=false")
	}
	if resp["needsAgent"] != true {
		t.Errorf("expected needsAgent=true for finished agent with pending comments, got %v", resp["needsAgent"])
	}
}

// E-PENPAL-API-ROUTES: verifies full response shape when an agent is actively running.
func TestAPIAgentStatus_RunningAgent_FullResponseShape(t *testing.T) {
	s, c, cs := testServer(t)
	s.agents = agents.New(c, cs, 0)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	s.agents.SimulateRunning("test-proj", 50000, 200000, 1.23, 5)

	req := httptest.NewRequest(http.MethodGet, "/api/agents?project=test-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var resp agentStatusResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse: %v", err)
	}

	if resp.Project != "test-proj" {
		t.Errorf("expected project=%q, got %q", "test-proj", resp.Project)
	}
	if !resp.Running {
		t.Errorf("expected running=true")
	}
	if resp.PID != 99999 {
		t.Errorf("expected pid=99999, got %d", resp.PID)
	}
	if resp.ContextWindow != 200000 {
		t.Errorf("expected contextWindow=200000, got %d", resp.ContextWindow)
	}
	if resp.ContextUsed != 50000 {
		t.Errorf("expected contextUsed=50000, got %d", resp.ContextUsed)
	}
	if resp.ContextPercent != 25.0 {
		t.Errorf("expected contextPercent=25.0, got %f", resp.ContextPercent)
	}
	if resp.TotalCostUSD != 1.23 {
		t.Errorf("expected totalCostUSD=1.23, got %f", resp.TotalCostUSD)
	}
	if resp.NumTurns != 5 {
		t.Errorf("expected numTurns=5, got %d", resp.NumTurns)
	}
	if resp.NeedsAgent {
		t.Errorf("expected needsAgent=false when agent is running")
	}
}

// E-PENPAL-API-ROUTES: verifies POST /api/agents/start returns 503 when manager is nil.
func TestAPIAgentStart_NoManager(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodPost, "/api/agents/start?project=test-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusServiceUnavailable {
		t.Errorf("expected 503, got %d", rec.Code)
	}
}

// E-PENPAL-API-ROUTES: verifies POST /api/agents/start returns 400 when project is missing.
func TestAPIAgentStart_MissingProject(t *testing.T) {
	s, c, cs := testServer(t)
	s.agents = agents.New(c, cs, 0)

	req := httptest.NewRequest(http.MethodPost, "/api/agents/start", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Errorf("expected 400, got %d", rec.Code)
	}
}

// E-PENPAL-API-ROUTES: verifies POST /api/agents/stop returns 503 when manager is nil.
func TestAPIAgentStop_NoManager(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodPost, "/api/agents/stop?project=test-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusServiceUnavailable {
		t.Errorf("expected 503, got %d", rec.Code)
	}
}

// E-PENPAL-API-ROUTES: verifies POST /api/agents/stop returns 400 when project is missing.
func TestAPIAgentStop_MissingProject(t *testing.T) {
	s, c, cs := testServer(t)
	s.agents = agents.New(c, cs, 0)

	req := httptest.NewRequest(http.MethodPost, "/api/agents/stop", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Errorf("expected 400, got %d", rec.Code)
	}
}

// E-PENPAL-API-ROUTES: verifies GET /api/agents returns 400 when project is missing.
func TestAPIAgentStatus_MissingProject(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodGet, "/api/agents", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Errorf("expected 400, got %d", rec.Code)
	}
}
