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
