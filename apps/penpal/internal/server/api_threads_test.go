package server

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"

	"github.com/loganj/penpal/internal/agents"
	"github.com/loganj/penpal/internal/comments"
)

// E-PENPAL-API-ROUTES: verifies POST /api/threads creates thread and GET lists it.
func TestAPIThreads_CreateAndList(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create a thread
	createBody, _ := json.Marshal(map[string]interface{}{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"anchor":  map[string]string{"selectedText": "some text"},
		"author":  "user",
		"role":    "human",
		"body":    "Hello",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads", bytes.NewReader(createBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("create: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var thread comments.Thread
	if err := json.Unmarshal(rec.Body.Bytes(), &thread); err != nil {
		t.Fatalf("parse thread: %v", err)
	}
	if thread.ID == "" {
		t.Fatal("expected thread to have an ID")
	}
	if thread.Status != "open" {
		t.Errorf("expected status 'open', got %q", thread.Status)
	}

	// List threads for that file
	req = httptest.NewRequest(http.MethodGet, "/api/threads?project=test-proj&path=thoughts/plan.md", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("list: expected 200, got %d", rec.Code)
	}

	var threads []json.RawMessage
	if err := json.Unmarshal(rec.Body.Bytes(), &threads); err != nil {
		t.Fatalf("parse threads: %v", err)
	}
	if len(threads) != 1 {
		t.Fatalf("expected 1 thread, got %d", len(threads))
	}
}

// E-PENPAL-API-ROUTES: verifies POST /api/threads/{id}/comments adds a reply.
func TestAPIThreads_AddComment(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create thread
	createBody, _ := json.Marshal(map[string]interface{}{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"anchor":  map[string]string{"selectedText": "text"},
		"author":  "user",
		"role":    "human",
		"body":    "First",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads", bytes.NewReader(createBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	var thread comments.Thread
	json.Unmarshal(rec.Body.Bytes(), &thread)

	// Add a comment
	commentBody, _ := json.Marshal(map[string]string{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"author":  "bot",
		"role":    "agent",
		"body":    "Reply",
	})
	req = httptest.NewRequest(http.MethodPost, "/api/threads/"+thread.ID+"/comments", bytes.NewReader(commentBody))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("add comment: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var updated comments.Thread
	if err := json.Unmarshal(rec.Body.Bytes(), &updated); err != nil {
		t.Fatalf("parse updated thread: %v", err)
	}
	if len(updated.Comments) != 2 {
		t.Errorf("expected 2 comments, got %d", len(updated.Comments))
	}
}

// E-PENPAL-API-ROUTES: verifies PATCH /api/threads/{id} resolves and reopens.
func TestAPIThreads_ResolveAndReopen(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create thread
	createBody, _ := json.Marshal(map[string]interface{}{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"anchor":  map[string]string{"selectedText": "text"},
		"author":  "user",
		"role":    "human",
		"body":    "Hello",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads", bytes.NewReader(createBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	var thread comments.Thread
	json.Unmarshal(rec.Body.Bytes(), &thread)

	// Resolve
	resolveBody, _ := json.Marshal(map[string]string{
		"project":    "test-proj",
		"path":       "thoughts/plan.md",
		"status":     "resolved",
		"resolvedBy": "user",
	})
	req = httptest.NewRequest(http.MethodPatch, "/api/threads/"+thread.ID, bytes.NewReader(resolveBody))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("resolve: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	// Reopen
	reopenBody, _ := json.Marshal(map[string]string{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"status":  "open",
	})
	req = httptest.NewRequest(http.MethodPatch, "/api/threads/"+thread.ID, bytes.NewReader(reopenBody))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("reopen: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-API-ROUTES: verifies GET /api/threads?project=X returns project-wide open threads.
func TestAPIThreads_ListOpenAcrossProject(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	req := httptest.NewRequest(http.MethodGet, "/api/threads?project=test-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var threads []comments.ThreadWithFile
	if err := json.Unmarshal(rec.Body.Bytes(), &threads); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if len(threads) != 0 {
		t.Errorf("expected empty list, got %d", len(threads))
	}
}

// E-PENPAL-REVIEW-COUNT: verifies GET /api/reviews returns files with open threads.
func TestAPIReviews_ListFilesInReview(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create the actual source file so ListFilesInReview finds it
	if err := os.MkdirAll(filepath.Join(dir, "thoughts"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "thoughts", "plan.md"), []byte("text"), 0o644); err != nil {
		t.Fatal(err)
	}

	// Create a thread to put a file "in review"
	createBody, _ := json.Marshal(map[string]interface{}{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"anchor":  map[string]string{"selectedText": "text"},
		"author":  "user",
		"role":    "human",
		"body":    "Review this",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads", bytes.NewReader(createBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	// List reviews
	req = httptest.NewRequest(http.MethodGet, "/api/reviews?project=test-proj", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var reviews []APIFileInReview
	if err := json.Unmarshal(rec.Body.Bytes(), &reviews); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if len(reviews) != 1 {
		t.Fatalf("expected 1 file in review, got %d", len(reviews))
	}
	if reviews[0].FilePath != "thoughts/plan.md" {
		t.Errorf("expected path 'thoughts/plan.md', got %q", reviews[0].FilePath)
	}
}

// E-PENPAL-WORKING: verifies agentWorking=true when SetWorking is active for a thread.
func TestAPIThreads_AgentWorkingField(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create a thread
	createBody, _ := json.Marshal(map[string]interface{}{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"anchor":  map[string]string{"selectedText": "some text"},
		"author":  "user",
		"role":    "human",
		"body":    "Hello",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads", bytes.NewReader(createBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("create: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var thread comments.Thread
	if err := json.Unmarshal(rec.Body.Bytes(), &thread); err != nil {
		t.Fatalf("parse thread: %v", err)
	}

	// Mark the thread as working
	cs.SetWorking("test-proj", "thoughts/plan.md", thread.ID, "")

	// List threads and check agentWorking
	req = httptest.NewRequest(http.MethodGet, "/api/threads?project=test-proj&path=thoughts/plan.md", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("list: expected 200, got %d", rec.Code)
	}

	var threads []threadResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &threads); err != nil {
		t.Fatalf("parse threads: %v", err)
	}
	if len(threads) != 1 {
		t.Fatalf("expected 1 thread, got %d", len(threads))
	}
	if !threads[0].AgentWorking {
		t.Errorf("expected agentWorking=true")
	}
}

// E-PENPAL-WORKING: verifies workingAfterCommentId is returned in thread response.
func TestAPIThreads_WorkingAfterCommentId(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create a thread
	createBody, _ := json.Marshal(map[string]interface{}{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"anchor":  map[string]string{"selectedText": "some text"},
		"author":  "user",
		"role":    "human",
		"body":    "Hello",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads", bytes.NewReader(createBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	var thread comments.Thread
	if err := json.Unmarshal(rec.Body.Bytes(), &thread); err != nil {
		t.Fatalf("parse thread: %v", err)
	}
	commentID := thread.Comments[0].ID

	// Mark the thread as working with the comment ID
	cs.SetWorking("test-proj", "thoughts/plan.md", thread.ID, commentID)

	// List threads and check workingAfterCommentId
	req = httptest.NewRequest(http.MethodGet, "/api/threads?project=test-proj&path=thoughts/plan.md", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	var threads []threadResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &threads); err != nil {
		t.Fatalf("parse threads: %v", err)
	}
	if len(threads) != 1 {
		t.Fatalf("expected 1 thread, got %d", len(threads))
	}
	if !threads[0].AgentWorking {
		t.Error("expected agentWorking=true")
	}
	if threads[0].WorkingAfterCommentID != commentID {
		t.Errorf("workingAfterCommentId = %q, want %q", threads[0].WorkingAfterCommentID, commentID)
	}
}

// E-PENPAL-WORKING: verifies agentWorking defaults to false when no working indicator is set.
func TestAPIThreads_AgentWorkingFalseByDefault(t *testing.T) {
	s, c, _ := testServer(t)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create a thread
	createBody, _ := json.Marshal(map[string]interface{}{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"anchor":  map[string]string{"selectedText": "some text"},
		"author":  "user",
		"role":    "human",
		"body":    "Hello",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads", bytes.NewReader(createBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("create: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	// List threads without setting any working indicator
	req = httptest.NewRequest(http.MethodGet, "/api/threads?project=test-proj&path=thoughts/plan.md", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("list: expected 200, got %d", rec.Code)
	}

	var threads []threadResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &threads); err != nil {
		t.Fatalf("parse threads: %v", err)
	}
	if len(threads) != 1 {
		t.Fatalf("expected 1 thread, got %d", len(threads))
	}
	if threads[0].AgentWorking {
		t.Errorf("expected agentWorking=false by default")
	}
}

// E-PENPAL-HEARTBEAT: verifies heartbeat is recorded when agent=true query param is set on threads endpoint.
func TestAPIThreads_HeartbeatRecordedOnAgentPoll(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Poll with agent=true
	req := httptest.NewRequest(http.MethodGet, "/api/threads?project=test-proj&path=thoughts/plan.md&agent=true", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	if !cs.IsAgentActive("test-proj", "thoughts/plan.md") {
		t.Errorf("expected IsAgentActive=true after agent poll")
	}
}

// E-PENPAL-API-ROUTES: verifies agentActive and workingThreads fields in reviews response.
func TestAPIReviews_AgentActiveAndWorkingFields(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create the actual source file so ListFilesInReview finds it
	if err := os.MkdirAll(filepath.Join(dir, "thoughts"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "thoughts", "plan.md"), []byte("text"), 0o644); err != nil {
		t.Fatal(err)
	}

	// Create a thread to put a file "in review"
	createBody, _ := json.Marshal(map[string]interface{}{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"anchor":  map[string]string{"selectedText": "text"},
		"author":  "user",
		"role":    "human",
		"body":    "Review this",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads", bytes.NewReader(createBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("create thread: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var thread comments.Thread
	if err := json.Unmarshal(rec.Body.Bytes(), &thread); err != nil {
		t.Fatalf("parse thread: %v", err)
	}

	// Set up a running agent and working indicator
	s.agents = agents.New(c, cs, 0)
	s.agents.SimulateRunning("test-proj", 1000, 200000, 0.5, 1)
	cs.SetWorking("test-proj", "thoughts/plan.md", thread.ID, "")

	// List reviews
	req = httptest.NewRequest(http.MethodGet, "/api/reviews?project=test-proj", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var reviews []APIFileInReview
	if err := json.Unmarshal(rec.Body.Bytes(), &reviews); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if len(reviews) != 1 {
		t.Fatalf("expected 1 file in review, got %d", len(reviews))
	}
	if !reviews[0].AgentActive {
		t.Errorf("expected agentActive=true")
	}
	if reviews[0].WorkingThreads != 1 {
		t.Errorf("expected workingThreads=1, got %d", reviews[0].WorkingThreads)
	}
}

// E-PENPAL-HEARTBEAT: verifies heartbeat is recorded when agent=true is set on reviews endpoint.
func TestAPIReviews_HeartbeatRecordedOnAgentPoll(t *testing.T) {
	s, c, cs := testServer(t)
	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create the actual source file
	if err := os.MkdirAll(filepath.Join(dir, "thoughts"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "thoughts", "plan.md"), []byte("text"), 0o644); err != nil {
		t.Fatal(err)
	}

	// Create a thread to put a file "in review"
	createBody, _ := json.Marshal(map[string]interface{}{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"anchor":  map[string]string{"selectedText": "text"},
		"author":  "user",
		"role":    "human",
		"body":    "Review this",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads", bytes.NewReader(createBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("create thread: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	// Poll reviews with agent=true
	req = httptest.NewRequest(http.MethodGet, "/api/reviews?project=test-proj&agent=true", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	// Verify heartbeat was recorded for the file in review
	if !cs.IsAgentActive("test-proj", "thoughts/plan.md") {
		t.Errorf("expected IsAgentActive=true after agent poll on reviews")
	}
}
