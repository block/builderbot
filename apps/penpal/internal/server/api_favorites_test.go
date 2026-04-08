package server

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"testing"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/config"
	"github.com/loganj/penpal/internal/discovery"
)

// E-PENPAL-FAVORITES: verifies directory favorites populate from known markdown metadata even without __all_markdown__.
func TestBuildFavoriteEntries_TreeFallsBackWithoutAllMarkdown(t *testing.T) {
	projectPath := t.TempDir()
	project := &discovery.Project{
		Path: projectPath,
		Sources: []discovery.FileSource{{
			Name:           "docs",
			Type:           "tree",
			SourceTypeName: "manual",
			RootPath:       filepath.Join(projectPath, "docs"),
		}},
	}

	favorites := buildFavoriteEntries(project, []cache.FileInfo{
		{Source: "anchors", FullPath: "docs/guide.md", Name: "guide.md", Title: "Guide"},
		{Source: "anchors", FullPath: "docs/proposals/idea.md", Name: "idea.md", Title: "Idea"},
	})

	if len(favorites) != 1 {
		t.Fatalf("expected 1 favorite, got %d", len(favorites))
	}
	if favorites[0].Kind != "tree" || favorites[0].Path != "docs" {
		t.Fatalf("expected docs tree favorite, got %+v", favorites[0])
	}
	if len(favorites[0].Files) != 2 {
		t.Fatalf("expected docs tree to expose 2 files, got %+v", favorites[0].Files)
	}
	if favorites[0].Files[0].Path != "docs/guide.md" || favorites[0].Files[0].DisplayPath != "guide.md" {
		t.Fatalf("expected first docs file to be guide.md, got %+v", favorites[0].Files[0])
	}
	if favorites[0].Files[1].Path != "docs/proposals/idea.md" || favorites[0].Files[1].DisplayPath != "proposals/idea.md" {
		t.Fatalf("expected nested docs file to preserve subtree display path, got %+v", favorites[0].Files[1])
	}
}

// P-PENPAL-FAVORITES, E-PENPAL-FAVORITES: verifies persisted favorites list separately from normal project sources.
func TestAPIFavorites_ListExistingManualSources(t *testing.T) {
	s, _, _ := testServer(t)

	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "docs"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "docs", "guide.md"), []byte("# Guide"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "notes.md"), []byte("# Notes"), 0o644); err != nil {
		t.Fatal(err)
	}

	s.cfg.Projects = append(s.cfg.Projects, config.ProjectConfig{
		Path: dir,
		Sources: []config.SourceConfig{
			{Type: "tree", Path: "docs"},
			{Type: "files", Files: []string{"notes.md"}},
		},
	})
	s.refreshAfterConfigChange()

	projectName := filepath.Base(dir)
	req := httptest.NewRequest(http.MethodGet, "/api/favorites?project="+url.QueryEscape(projectName), nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var favorites []APIFavoriteEntry
	if err := json.Unmarshal(rec.Body.Bytes(), &favorites); err != nil {
		t.Fatalf("parse favorites: %v", err)
	}
	if len(favorites) != 2 {
		t.Fatalf("expected 2 favorites, got %d", len(favorites))
	}
	if favorites[0].Kind != "tree" || favorites[0].Path != "docs" {
		t.Fatalf("expected first favorite to be docs tree, got %+v", favorites[0])
	}
	if len(favorites[0].Files) != 1 || favorites[0].Files[0].Path != "docs/guide.md" || favorites[0].Files[0].DisplayPath != "guide.md" {
		t.Fatalf("expected docs tree to expose guide.md, got %+v", favorites[0].Files)
	}
	if favorites[1].Kind != "file" || favorites[1].Path != "notes.md" {
		t.Fatalf("expected second favorite to be notes.md file, got %+v", favorites[1])
	}

	req = httptest.NewRequest(http.MethodGet, "/api/project/"+projectName, nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("project files: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}
	var groups []APIFileGroupView
	if err := json.Unmarshal(rec.Body.Bytes(), &groups); err != nil {
		t.Fatalf("parse project groups: %v", err)
	}
	for _, group := range groups {
		if group.Source == "docs" || group.Name == "docs" {
			t.Fatalf("manual favorite leaked into project source groups: %+v", group)
		}
	}
}

// P-PENPAL-FAVORITES, P-PENPAL-FAVORITE-ACTIONS, E-PENPAL-FAVORITES: verifies add/remove favorites API round-trip.
func TestAPIFavorites_AddAndRemove(t *testing.T) {
	s, _, _ := testServer(t)

	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "docs"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "docs", "guide.md"), []byte("# Guide"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "notes.md"), []byte("# Notes"), 0o644); err != nil {
		t.Fatal(err)
	}

	s.cfg.Projects = append(s.cfg.Projects, config.ProjectConfig{Path: dir})
	s.refreshAfterConfigChange()

	projectName := filepath.Base(dir)

	addFavorite := func(path string) {
		body, _ := json.Marshal(map[string]string{"project": projectName, "path": path})
		req := httptest.NewRequest(http.MethodPost, "/api/favorites", bytes.NewReader(body))
		req.Header.Set("Content-Type", "application/json")
		rec := httptest.NewRecorder()
		s.ServeHTTP(rec, req)
		if rec.Code != http.StatusNoContent {
			t.Fatalf("add %s: expected 204, got %d: %s", path, rec.Code, rec.Body.String())
		}
	}
	removeFavorite := func(path string) {
		body, _ := json.Marshal(map[string]string{"project": projectName, "path": path})
		req := httptest.NewRequest(http.MethodDelete, "/api/favorites", bytes.NewReader(body))
		req.Header.Set("Content-Type", "application/json")
		rec := httptest.NewRecorder()
		s.ServeHTTP(rec, req)
		if rec.Code != http.StatusNoContent {
			t.Fatalf("remove %s: expected 204, got %d: %s", path, rec.Code, rec.Body.String())
		}
	}
	listFavorites := func() []APIFavoriteEntry {
		req := httptest.NewRequest(http.MethodGet, "/api/favorites?project="+url.QueryEscape(projectName), nil)
		rec := httptest.NewRecorder()
		s.ServeHTTP(rec, req)
		if rec.Code != http.StatusOK {
			t.Fatalf("list: expected 200, got %d: %s", rec.Code, rec.Body.String())
		}
		var favorites []APIFavoriteEntry
		if err := json.Unmarshal(rec.Body.Bytes(), &favorites); err != nil {
			t.Fatalf("parse favorites: %v", err)
		}
		return favorites
	}

	addFavorite("docs")
	addFavorite("notes.md")

	favorites := listFavorites()
	if len(favorites) != 2 {
		t.Fatalf("expected 2 favorites after add, got %d", len(favorites))
	}
	if favorites[0].Kind != "tree" || favorites[0].Path != "docs" {
		t.Fatalf("expected docs tree favorite first, got %+v", favorites[0])
	}
	if favorites[1].Kind != "file" || favorites[1].Path != "notes.md" {
		t.Fatalf("expected notes.md file favorite second, got %+v", favorites[1])
	}

	removeFavorite("docs")
	favorites = listFavorites()
	if len(favorites) != 1 || favorites[0].Path != "notes.md" {
		t.Fatalf("expected only notes.md favorite after removal, got %+v", favorites)
	}
}
