package server

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/loganj/birdseye/internal/agents"
	"github.com/loganj/birdseye/internal/cache"
	"github.com/loganj/birdseye/internal/comments"
	"github.com/loganj/birdseye/internal/config"
	"github.com/loganj/birdseye/internal/watcher"
)

// Integration tests for rp1-differentiation feature
// These tests verify that display names are correctly passed through the API layer

// TestRecentFiles_DisplayName tests that recent files API returns qualified display names
func TestRecentFiles_DisplayName(t *testing.T) {
	c := cache.New()
	w, err := watcher.New(c)
	if err != nil {
		t.Fatalf("failed to create watcher: %v", err)
	}

	cs := comments.NewStore(c)
	am := agents.New(c, cs, 8080)
	cfg := &config.Config{}

	s := New(c, w, cs, nil, am, "", cfg, "")

	projectName := "test/project"
	files := []cache.FileInfo{
		{
			Project:   projectName,
			Name:      "requirements.md",
			FullPath:  ".rp1/work/features/test-feature/requirements.md",
			Source:    "rp1",
			FeatureID: "test-feature",
			FileType:  "requirements",
			ModTime:   time.Now().Add(-1 * time.Minute),
		},
		{
			Project:  projectName,
			Name:     "index.md",
			FullPath: ".rp1/context/index.md",
			Source:   "rp1",
			Category: "Context",
			FileType: "other",
			ModTime:  time.Now(),
		},
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
		t.Fatalf("failed to parse JSON response: %v", err)
	}

	if len(response) != 2 {
		t.Fatalf("expected 2 files in response, got %d", len(response))
	}

	// First file (most recent) should be index.md with simple name
	if response[0].DisplayName != "index.md" {
		t.Errorf("expected DisplayName 'index.md' for first file, got %q", response[0].DisplayName)
	}

	// Second file should be requirements.md with qualified name
	if response[1].DisplayName != "test-feature/requirements.md" {
		t.Errorf("expected DisplayName 'test-feature/requirements.md' for second file, got %q", response[1].DisplayName)
	}

	// Verify Path is preserved (for navigation)
	if response[1].Path != ".rp1/work/features/test-feature/requirements.md" {
		t.Errorf("expected Path to be preserved, got %q", response[1].Path)
	}
}

// TestAPIFile_DisplayName_Integration tests that the /api/recent endpoint returns
// qualified display names for feature files and simple names for non-feature files.
// This is a lightweight integration test that verifies the API contract without
// requiring full project discovery.
func TestAPIFile_DisplayName_Integration(t *testing.T) {
	c := cache.New()
	w, err := watcher.New(c)
	if err != nil {
		t.Fatalf("failed to create watcher: %v", err)
	}

	cs := comments.NewStore(c)
	am := agents.New(c, cs, 8080)
	cfg := &config.Config{}

	s := New(c, w, cs, nil, am, "", cfg, "")

	projectName := "test/project"
	now := time.Now()

	tests := []struct {
		name  string
		files []cache.FileInfo
		want  []struct {
			displayName string
			path        string
		}
	}{
		{
			name: "qualified names for feature files",
			files: []cache.FileInfo{
				{
					Project:   projectName,
					Name:      "requirements.md",
					FullPath:  ".rp1/work/features/auth-refactor/requirements.md",
					Source:    "rp1",
					FeatureID: "auth-refactor",
					FileType:  "requirements",
					ModTime:   now.Add(-2 * time.Minute),
				},
				{
					Project:   projectName,
					Name:      "tasks.md",
					FullPath:  ".rp1/work/features/data-layer/tasks.md",
					Source:    "rp1",
					FeatureID: "data-layer",
					FileType:  "tasks",
					ModTime:   now.Add(-1 * time.Minute),
				},
			},
			want: []struct {
				displayName string
				path        string
			}{
				{"data-layer/tasks.md", ".rp1/work/features/data-layer/tasks.md"},
				{"auth-refactor/requirements.md", ".rp1/work/features/auth-refactor/requirements.md"},
			},
		},
		{
			name: "simple names for non-feature files",
			files: []cache.FileInfo{
				{
					Project:  projectName,
					Name:     "index.md",
					FullPath: ".rp1/context/index.md",
					Source:   "rp1",
					Category: "Context",
					FileType: "other",
					ModTime:  now.Add(-2 * time.Minute),
				},
				{
					Project:  projectName,
					Name:     "my-prd.md",
					FullPath: ".rp1/work/prds/my-prd.md",
					Source:   "rp1",
					Category: "PRDs",
					FileType: "other",
					ModTime:  now.Add(-1 * time.Minute),
				},
			},
			want: []struct {
				displayName string
				path        string
			}{
				{"my-prd.md", ".rp1/work/prds/my-prd.md"},
				{"index.md", ".rp1/context/index.md"},
			},
		},
		{
			name: "mixed feature and non-feature files",
			files: []cache.FileInfo{
				{
					Project:   projectName,
					Name:      "design.md",
					FullPath:  ".rp1/work/features/test-feature/design.md",
					Source:    "rp1",
					FeatureID: "test-feature",
					FileType:  "design",
					ModTime:   now.Add(-3 * time.Minute),
				},
				{
					Project:  projectName,
					Name:     "architecture.md",
					FullPath: ".rp1/context/architecture.md",
					Source:   "rp1",
					Category: "Context",
					FileType: "other",
					ModTime:  now.Add(-2 * time.Minute),
				},
				{
					Project:  projectName,
					Name:     "charter.md",
					FullPath: ".rp1/work/charter.md",
					Source:   "rp1",
					Category: "Other",
					FileType: "other",
					ModTime:  now.Add(-1 * time.Minute),
				},
			},
			want: []struct {
				displayName string
				path        string
			}{
				{"charter.md", ".rp1/work/charter.md"},
				{"architecture.md", ".rp1/context/architecture.md"},
				{"test-feature/design.md", ".rp1/work/features/test-feature/design.md"},
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			c.SetProjectFiles(projectName, tt.files)

			req := httptest.NewRequest(http.MethodGet, "/api/recent", nil)
			rec := httptest.NewRecorder()

			s.ServeHTTP(rec, req)

			if rec.Code != http.StatusOK {
				t.Fatalf("expected status 200, got %d", rec.Code)
			}

			var response []APIFile
			if err := json.Unmarshal(rec.Body.Bytes(), &response); err != nil {
				t.Fatalf("failed to parse JSON response: %v", err)
			}

			if len(response) != len(tt.want) {
				t.Fatalf("expected %d files in response, got %d", len(tt.want), len(response))
			}

			for i, want := range tt.want {
				if response[i].DisplayName != want.displayName {
					t.Errorf("file %d: expected DisplayName %q, got %q", i, want.displayName, response[i].DisplayName)
				}
				if response[i].Path != want.path {
					t.Errorf("file %d: expected Path %q, got %q", i, want.path, response[i].Path)
				}
			}
		})
	}
}
