package server

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/discovery"
)

func TestAPIProjectFiles_ReturnsGroups(t *testing.T) {
	s, c, _ := testServer(t)

	projectName := "test/project"
	project := seedProject(c, projectName, "/tmp/test", nil)
	project.Sources = []discovery.FileSource{
		{Name: "rp1", Type: "tree", SourceTypeName: "rp1", RootPath: "/tmp/test/.rp1", Auto: true},
	}
	// Re-set projects to include the sources we just added
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

	// Should have 2 flat groups: auth, Context (context always last)
	if len(groups) != 2 {
		t.Fatalf("expected 2 groups, got %d", len(groups))
	}

	if groups[0].Name != "Feature: auth" {
		t.Errorf("expected first group 'Feature: auth', got %q", groups[0].Name)
	}
	if groups[0].Source != "rp1" {
		t.Errorf("expected source 'rp1', got %q", groups[0].Source)
	}
	if !groups[0].Auto {
		t.Error("expected auto=true")
	}

	if groups[1].Name != "Context" {
		t.Errorf("expected second group 'Context', got %q", groups[1].Name)
	}
}

func TestAPIRecent_ReturnsFiles(t *testing.T) {
	s, c, _ := testServer(t)

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
