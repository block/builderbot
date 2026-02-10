package server

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/loganj/birdseye/internal/activity"
	"github.com/loganj/birdseye/internal/agents"
	"github.com/loganj/birdseye/internal/cache"
	"github.com/loganj/birdseye/internal/comments"
	"github.com/loganj/birdseye/internal/config"
	"github.com/loganj/birdseye/internal/discovery"
	"github.com/loganj/birdseye/internal/watcher"
)

func TestAPIProjectFiles_ReturnsGroups(t *testing.T) {
	c := cache.New()
	act := activity.New()
	w, err := watcher.New(c, act)
	if err != nil {
		t.Fatalf("failed to create watcher: %v", err)
	}

	cs := comments.NewStore(c, act)
	am := agents.New(c, cs, 8080)
	cfg := &config.Config{}

	s := New(c, w, cs, nil, am, act, "", cfg, "")

	// Trigger ensureLoaded so it doesn't reset our test data
	s.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest(http.MethodGet, "/", nil))

	projectName := "test/project"
	project := discovery.Project{
		Name:          "project",
		Path:          "/tmp/test",
		WorkspaceName: "test",
		Origin:        "workspace",
		Sources: []discovery.FileSource{
			{Name: "rp1", Type: "tree", RootPath: "/tmp/test/.rp1", Auto: true},
		},
	}
	c.SetProjects([]discovery.Project{project})

	now := time.Now()
	files := []cache.FileInfo{
		{Project: projectName, Source: "rp1", Path: "context/index.md", FullPath: ".rp1/context/index.md", Name: "index.md", FileType: "knowledge", ModTime: now},
		{Project: projectName, Source: "rp1", Path: "work/features/auth/requirements.md", FullPath: ".rp1/work/features/auth/requirements.md", Name: "requirements.md", FileType: "requirement", ModTime: now},
	}
	c.SetProjectFiles(projectName, files)

	req := httptest.NewRequest(http.MethodGet, "/api/project/"+projectName, nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected status 200, got %d", rec.Code)
	}

	var groups []APIFileGroupView
	if err := json.Unmarshal(rec.Body.Bytes(), &groups); err != nil {
		t.Fatalf("failed to parse JSON: %v", err)
	}

	// Should have 2 flat groups: Context, auth
	if len(groups) != 2 {
		t.Fatalf("expected 2 groups, got %d", len(groups))
	}

	if groups[0].Name != "Context" {
		t.Errorf("expected first group 'Context', got %q", groups[0].Name)
	}
	if groups[0].Source != "rp1" {
		t.Errorf("expected source 'rp1', got %q", groups[0].Source)
	}
	if !groups[0].Auto {
		t.Error("expected auto=true")
	}

	if groups[1].Name != "auth" {
		t.Errorf("expected second group 'auth', got %q", groups[1].Name)
	}
}

func TestAPIRecent_ReturnsFiles(t *testing.T) {
	c := cache.New()
	act := activity.New()
	w, err := watcher.New(c, act)
	if err != nil {
		t.Fatalf("failed to create watcher: %v", err)
	}

	cs := comments.NewStore(c, act)
	am := agents.New(c, cs, 8080)
	cfg := &config.Config{}

	s := New(c, w, cs, nil, am, act, "", cfg, "")

	projectName := "test/project"
	now := time.Now()
	files := []cache.FileInfo{
		{Project: projectName, Name: "index.md", FullPath: ".rp1/context/index.md", Source: "rp1", FileType: "knowledge", ModTime: now},
		{Project: projectName, Name: "requirements.md", FullPath: ".rp1/work/features/auth/requirements.md", Source: "rp1", FileType: "requirement", ModTime: now.Add(-1 * time.Minute)},
	}
	c.SetProjectFiles(projectName, files)

	req := httptest.NewRequest(http.MethodGet, "/api/recent", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected status 200, got %d", rec.Code)
	}

	var response []APIFile
	if err := json.Unmarshal(rec.Body.Bytes(), &response); err != nil {
		t.Fatalf("failed to parse JSON: %v", err)
	}

	if len(response) != 2 {
		t.Fatalf("expected 2 files, got %d", len(response))
	}

	// Files should have name (base filename), not qualified display name
	if response[0].Name != "index.md" {
		t.Errorf("expected name 'index.md', got %q", response[0].Name)
	}
	if response[1].Name != "requirements.md" {
		t.Errorf("expected name 'requirements.md', got %q", response[1].Name)
	}

	// Path should be project-relative
	if response[1].Path != ".rp1/work/features/auth/requirements.md" {
		t.Errorf("expected path preserved, got %q", response[1].Path)
	}
}
