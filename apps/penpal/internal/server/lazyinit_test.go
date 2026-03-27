package server

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"

	"github.com/loganj/penpal/internal/activity"
	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
	"github.com/loganj/penpal/internal/config"
	"github.com/loganj/penpal/internal/watcher"
)

// E-PENPAL-LAZY-INIT: verifies the first HTTP request triggers project discovery.
func TestLazyInit_FirstRequestTriggersDiscovery(t *testing.T) {
	// Create a workspace directory with a project inside
	wsDir := t.TempDir()
	projDir := filepath.Join(wsDir, "myproject")
	if err := os.MkdirAll(projDir, 0o755); err != nil {
		t.Fatal(err)
	}
	// Create a thoughts directory so the project is discoverable
	thoughtsDir := filepath.Join(projDir, "thoughts")
	if err := os.MkdirAll(thoughtsDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(thoughtsDir, "test.md"), []byte("# Test"), 0o644); err != nil {
		t.Fatal(err)
	}

	// Create the server with a real workspace config but do NOT call ServeHTTP yet
	c := cache.New()
	act := activity.New()
	w, err := watcher.New(c, act)
	if err != nil {
		t.Fatalf("watcher: %v", err)
	}
	cs := comments.NewStore(c, act)
	cfg := &config.Config{
		Workspaces: []config.Workspace{{Path: wsDir}},
	}
	s := New(c, w, cs, nil, nil, act, cfg, "")

	// Before any HTTP request, the cache should have no projects
	if len(c.Projects()) != 0 {
		t.Fatalf("expected 0 projects before first request, got %d", len(c.Projects()))
	}

	// Issue the first HTTP request — this should trigger ensureLoaded
	req := httptest.NewRequest(http.MethodGet, "/api/projects", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	// After the first request, the cache should have discovered the project
	var resp []json.RawMessage
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if len(resp) == 0 {
		t.Fatal("expected at least one project after first request")
	}

	// Verify through the cache directly as well
	projects := c.Projects()
	if len(projects) == 0 {
		t.Fatal("expected cache to contain projects after first request")
	}

	found := false
	for _, p := range projects {
		if p.Name == "myproject" {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("expected to find project 'myproject' in cache, got: %v", projects)
	}
}
