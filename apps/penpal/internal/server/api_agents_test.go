package server

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

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
	token := attachSession(t, s, c, cs, "test-proj")

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
	req := httptest.NewRequest(http.MethodPost, "/api/threads/"+thread.ID+"/comments?session="+token, bytes.NewReader(replyBody))
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

// E-PENPAL-CLI-ATTACH: verifies POST /api/agents/attach succeeds with valid path.
func TestAPIAgentAttach_Success(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	// Create a markdown file so the project path is valid.
	os.MkdirAll(filepath.Join(dir, "thoughts"), 0755)
	os.WriteFile(filepath.Join(dir, "thoughts", "plan.md"), []byte("# Plan"), 0644)
	seedProject(c, "test-proj", dir, nil)
	s.agents = agents.New(c, cs, 0)

	body, _ := json.Marshal(map[string]any{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/agents/attach", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse: %v", err)
	}
	if resp["project"] != "test-proj" {
		t.Errorf("expected project=test-proj, got %q", resp["project"])
	}
	if resp["sessionToken"] == "" {
		t.Error("expected non-empty sessionToken")
	}
}

// E-PENPAL-CLI-CONTENTION: verifies double-attach without force returns 409.
func TestAPIAgentAttach_Conflict(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	os.MkdirAll(filepath.Join(dir, "thoughts"), 0755)
	os.WriteFile(filepath.Join(dir, "thoughts", "plan.md"), []byte("# Plan"), 0644)
	seedProject(c, "test-proj", dir, nil)
	s.agents = agents.New(c, cs, 0)

	body, _ := json.Marshal(map[string]any{"path": dir})

	// First attach.
	req := httptest.NewRequest(http.MethodPost, "/api/agents/attach", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("first attach: expected 200, got %d", rec.Code)
	}

	// Second attach without force should 409.
	body, _ = json.Marshal(map[string]any{"path": dir})
	req = httptest.NewRequest(http.MethodPost, "/api/agents/attach", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusConflict {
		t.Errorf("expected 409, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-CLI-CONTENTION: verifies attach with force=true succeeds when agent active.
func TestAPIAgentAttach_ForceEvicts(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	os.MkdirAll(filepath.Join(dir, "thoughts"), 0755)
	os.WriteFile(filepath.Join(dir, "thoughts", "plan.md"), []byte("# Plan"), 0644)
	seedProject(c, "test-proj", dir, nil)
	s.agents = agents.New(c, cs, 0)

	body, _ := json.Marshal(map[string]any{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/agents/attach", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("first attach: expected 200, got %d", rec.Code)
	}

	// Force attach should succeed.
	body, _ = json.Marshal(map[string]any{"path": dir, "force": true})
	req = httptest.NewRequest(http.MethodPost, "/api/agents/attach", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Errorf("expected 200 for forced attach, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-CLI-ATTACH: verifies attach returns 400 when path is missing.
func TestAPIAgentAttach_MissingPath(t *testing.T) {
	s, c, cs := testServer(t)
	s.agents = agents.New(c, cs, 0)

	body, _ := json.Marshal(map[string]any{})
	req := httptest.NewRequest(http.MethodPost, "/api/agents/attach", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Errorf("expected 400, got %d", rec.Code)
	}
}

// E-PENPAL-CLI-ATTACH: verifies attach returns 503 when s.agents is nil.
func TestAPIAgentAttach_NoManager(t *testing.T) {
	s, _, _ := testServer(t)
	dir := t.TempDir()

	body, _ := json.Marshal(map[string]any{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/agents/attach", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	// Without a manager, the handler returns 503 (or 404 for no project).
	// The path check happens before the manager check, so we need a valid project.
	// Since agents is nil, if we get past path checks we should see 503.
	if rec.Code != http.StatusNotFound && rec.Code != http.StatusServiceUnavailable && rec.Code != http.StatusBadRequest {
		t.Errorf("expected 400/404/503, got %d", rec.Code)
	}
}

// E-PENPAL-SESSION-MGMT: verifies wait returns 401 for invalid session.
func TestAPIAgentWait_InvalidSession(t *testing.T) {
	s, c, cs := testServer(t)
	s.agents = agents.New(c, cs, 0)

	req := httptest.NewRequest(http.MethodGet, "/api/agents/wait?project=test-proj&session=bad-token", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusUnauthorized {
		t.Errorf("expected 401, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-CLI-AGENT-CMDS: verifies wait returns 400 when params are missing.
func TestAPIAgentWait_MissingParams(t *testing.T) {
	s, c, cs := testServer(t)
	s.agents = agents.New(c, cs, 0)

	req := httptest.NewRequest(http.MethodGet, "/api/agents/wait", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Errorf("expected 400, got %d", rec.Code)
	}
}

// E-PENPAL-CLI-CONTENTION: verifies POST /api/agents/stop returns 200 even when no agent running.
func TestAPIAgentStop_ReturnsOK_WhenNoAgent(t *testing.T) {
	s, c, cs := testServer(t)
	s.agents = agents.New(c, cs, 0)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	req := httptest.NewRequest(http.MethodPost, "/api/agents/stop?project=test-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Errorf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-AGENT-SELF-ID: verifies attach returns agentName and uses it for comment author.
func TestAPIAgentAttach_AgentName(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	os.MkdirAll(filepath.Join(dir, "thoughts"), 0755)
	os.WriteFile(filepath.Join(dir, "thoughts", "plan.md"), []byte("# Plan"), 0644)
	seedProject(c, "test-proj", dir, nil)
	s.agents = agents.New(c, cs, 0)

	// Attach with agent name "amp".
	body, _ := json.Marshal(map[string]any{"path": dir, "agent": "amp"})
	req := httptest.NewRequest(http.MethodPost, "/api/agents/attach", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse: %v", err)
	}
	if resp["agentName"] != "amp" {
		t.Errorf("expected agentName=amp, got %q", resp["agentName"])
	}
	token := resp["sessionToken"]

	// Create a human thread first so we can post an agent reply.
	anchor := comments.Anchor{SelectedText: "Plan"}
	comment := comments.Comment{Author: "user", Role: "human", Body: "Review this"}
	thread, err := cs.CreateThread("test-proj", "thoughts/plan.md", anchor, comment)
	if err != nil {
		t.Fatalf("create thread: %v", err)
	}

	// Post an agent reply — server should override author to "amp".
	replyBody, _ := json.Marshal(map[string]string{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"author":  "ignored",
		"role":    "agent",
		"body":    "Looks good",
	})
	req = httptest.NewRequest(http.MethodPost, "/api/threads/"+thread.ID+"/comments?session="+token, bytes.NewReader(replyBody))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("reply: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var result comments.Thread
	if err := json.Unmarshal(rec.Body.Bytes(), &result); err != nil {
		t.Fatalf("parse reply: %v", err)
	}
	lastComment := result.Comments[len(result.Comments)-1]
	if lastComment.Author != "amp" {
		t.Errorf("expected author=amp, got %q", lastComment.Author)
	}
}

// E-PENPAL-AGENT-SELF-ID: verifies attach defaults agentName to "agent" when not provided.
func TestAPIAgentAttach_DefaultAgentName(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	os.MkdirAll(filepath.Join(dir, "thoughts"), 0755)
	os.WriteFile(filepath.Join(dir, "thoughts", "plan.md"), []byte("# Plan"), 0644)
	seedProject(c, "test-proj", dir, nil)
	s.agents = agents.New(c, cs, 0)

	body, _ := json.Marshal(map[string]any{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/agents/attach", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	json.Unmarshal(rec.Body.Bytes(), &resp)
	if resp["agentName"] != "agent" {
		t.Errorf("expected agentName=agent, got %q", resp["agentName"])
	}
}

// E-PENPAL-CLI-CONTENTION: verifies maybeStartAgent skips when CLI agent is attached.
func TestMaybeStartAgent_SkipsWhenCLIAgentAttached(t *testing.T) {
	s, c, cs := testServer(t)
	s.agents = agents.New(c, cs, 0)
	dir := t.TempDir()
	os.MkdirAll(filepath.Join(dir, "thoughts"), 0755)
	os.WriteFile(filepath.Join(dir, "thoughts", "plan.md"), []byte("# Plan"), 0644)
	seedProject(c, "test-proj", dir, nil)

	// Attach a CLI session.
	s.agents.Attach("test-proj", "", "claude", false)

	// Create a human comment via POST /api/threads (triggers maybeStartAgent).
	threadBody, _ := json.Marshal(map[string]any{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"author":  "user",
		"role":    "human",
		"body":    "Please review this",
		"anchor":  map[string]any{"selectedText": "Plan"},
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads", bytes.NewReader(threadBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("create thread: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	// Give maybeStartAgent's goroutine a moment to run.
	time.Sleep(50 * time.Millisecond)

	// Verify no spawned agent started — Status should be nil (no spawned agent).
	status := s.agents.Status("test-proj")
	if status != nil && status.PID != 0 {
		t.Errorf("expected no spawned agent, but found PID %d", status.PID)
	}
}
