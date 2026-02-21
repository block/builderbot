package server

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/discovery"
)

func assertHTMLResponse(t *testing.T, rec *httptest.ResponseRecorder) string {
	t.Helper()
	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d; body: %.500s", rec.Code, rec.Body.String())
	}
	body := rec.Body.String()
	if !strings.Contains(body, "<html") && !strings.Contains(body, "<!DOCTYPE") && !strings.Contains(body, "<div") {
		t.Errorf("expected HTML response body, got:\n%.500s", body)
	}
	return body
}

func TestHTMLIndex_Redirects(t *testing.T) {
	s, _, _ := testServer(t)

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusFound {
		t.Fatalf("expected 302, got %d", rec.Code)
	}
	if loc := rec.Header().Get("Location"); loc != "/recent" {
		t.Errorf("expected redirect to /recent, got %q", loc)
	}
}

func TestHTMLProject_RendersFileGroups(t *testing.T) {
	s, c, _ := testServer(t)

	project := discovery.Project{
		Name:          "myproject",
		Path:          "/tmp/test-proj",
		WorkspaceName: "test",
		WorkspacePath: "/tmp/test",
		Origin:        "workspace",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: "/tmp/test-proj/thoughts", Auto: true},
		},
	}
	c.SetProjects([]discovery.Project{project})
	c.SetProjectFiles("test/myproject", []cache.FileInfo{
		{Project: "test/myproject", Source: "thoughts", SourceType: "tree", Path: "plans/roadmap.md", FullPath: "thoughts/plans/roadmap.md", Name: "roadmap.md", ModTime: time.Now()},
	})

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/project/test/myproject", nil)
	s.ServeHTTP(rec, req)

	body := assertHTMLResponse(t, rec)
	if !strings.Contains(body, "roadmap.md") {
		t.Errorf("expected body to contain 'roadmap.md'")
	}
}

func TestHTMLFile_RendersMarkdown(t *testing.T) {
	s, c, _ := testServer(t)

	tmpDir := t.TempDir()
	mdDir := filepath.Join(tmpDir, "thoughts", "plans")
	os.MkdirAll(mdDir, 0755)
	os.WriteFile(filepath.Join(mdDir, "test.md"), []byte("# Hello World\n\nSome content"), 0644)

	project := discovery.Project{
		Name:          "project",
		Path:          tmpDir,
		WorkspaceName: "test",
		WorkspacePath: filepath.Dir(tmpDir),
		Origin:        "workspace",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: filepath.Join(tmpDir, "thoughts"), Auto: true},
		},
	}
	c.SetProjects([]discovery.Project{project})
	c.SetProjectFiles("test/project", []cache.FileInfo{
		{Project: "test/project", Source: "thoughts", SourceType: "tree", Path: "plans/test.md", FullPath: "thoughts/plans/test.md", Name: "test.md", ModTime: time.Now()},
	})

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/file/test/project/thoughts/plans/test.md", nil)
	s.ServeHTTP(rec, req)

	body := assertHTMLResponse(t, rec)
	if !strings.Contains(body, "Hello World") {
		t.Errorf("expected body to contain 'Hello World'")
	}
}

func TestHTMLRecent_RendersList(t *testing.T) {
	s, _, _ := testServer(t)

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/recent", nil)
	s.ServeHTTP(rec, req)

	assertHTMLResponse(t, rec)
}

func TestHTMLInReview_RendersList(t *testing.T) {
	s, _, _ := testServer(t)

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/in-review", nil)
	s.ServeHTTP(rec, req)

	assertHTMLResponse(t, rec)
}

func TestHTMLSearch_Empty(t *testing.T) {
	s, _, _ := testServer(t)

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/search", nil)
	s.ServeHTTP(rec, req)

	body := assertHTMLResponse(t, rec)
	if !strings.Contains(strings.ToLower(body), "search") {
		t.Errorf("expected body to contain 'search'")
	}
}

func TestHTMLSearch_WithQuery(t *testing.T) {
	s, c, _ := testServer(t)

	tmpDir := t.TempDir()
	thoughtsDir := filepath.Join(tmpDir, "thoughts")
	os.MkdirAll(thoughtsDir, 0755)
	os.WriteFile(filepath.Join(thoughtsDir, "test-file.md"), []byte("# Test content"), 0644)

	project := discovery.Project{
		Name:          "searchproj",
		Path:          tmpDir,
		WorkspaceName: "ws",
		WorkspacePath: filepath.Dir(tmpDir),
		Origin:        "workspace",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: thoughtsDir, Auto: true},
		},
	}
	c.SetProjects([]discovery.Project{project})
	c.SetProjectFiles("ws/searchproj", []cache.FileInfo{
		{Project: "ws/searchproj", Source: "thoughts", SourceType: "tree", Path: "test-file.md", FullPath: "thoughts/test-file.md", Name: "test-file.md", ModTime: time.Now()},
	})

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/search?q=test", nil)
	s.ServeHTTP(rec, req)

	body := assertHTMLResponse(t, rec)
	if !strings.Contains(body, "test-file.md") {
		t.Errorf("expected body to contain 'test-file.md'")
	}
}
