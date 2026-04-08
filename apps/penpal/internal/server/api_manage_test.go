package server

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
	"github.com/loganj/penpal/internal/config"
	"github.com/loganj/penpal/internal/discovery"
)

// E-PENPAL-REMOVE-WORKSPACE: verifies add and remove workspace round-trip.
func TestAPIWorkspaces_AddAndRemove(t *testing.T) {
	s, _, _ := testServer(t)
	dir := t.TempDir()

	// Add
	body, _ := json.Marshal(map[string]string{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/workspaces", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("add: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	// Remove
	body, _ = json.Marshal(map[string]string{"path": dir})
	req = httptest.NewRequest(http.MethodDelete, "/api/workspaces", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("remove: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-ADD-SOURCE: verifies POST /api/sources adds a directory tree source.
func TestAPISources_AddTreeSource(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "docs"), 0o755); err != nil {
		t.Fatal(err)
	}
	seedProject(c, "test-proj", dir, nil)

	body, _ := json.Marshal(map[string]string{
		"project": "test-proj",
		"path":    "docs",
		"name":    "docs",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-API-ROUTES: verifies POST /api/open resolves existing project.
func TestAPIOpen_ExistingProject(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	body, _ := json.Marshal(map[string]string{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/open", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if resp["url"] != "/project/test-proj" {
		t.Errorf("expected url '/project/test-proj', got %q", resp["url"])
	}
}

// E-PENPAL-SSE: verifies /api/navigate returns empty when no pending navigation.
func TestAPINavigate_EmptyByDefault(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodGet, "/api/navigate", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}
	var resp map[string]string
	json.Unmarshal(rec.Body.Bytes(), &resp)
	if resp["url"] != "" {
		t.Errorf("expected empty url, got %q", resp["url"])
	}
}

// E-PENPAL-SSE: verifies /api/open sets pendingNav consumed by /api/navigate.
func TestAPINavigate_SetByOpen(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Call /api/open to set pending navigation
	body, _ := json.Marshal(map[string]string{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/open", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("open: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	// /api/navigate should return the pending URL
	req = httptest.NewRequest(http.MethodGet, "/api/navigate", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	var resp map[string]string
	json.Unmarshal(rec.Body.Bytes(), &resp)
	if resp["url"] != "/project/test-proj" {
		t.Errorf("expected '/project/test-proj', got %q", resp["url"])
	}

	// Second call should return empty (consumed)
	req = httptest.NewRequest(http.MethodGet, "/api/navigate", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	resp = map[string]string{}
	json.Unmarshal(rec.Body.Bytes(), &resp)
	if resp["url"] != "" {
		t.Errorf("expected empty after consume, got %q", resp["url"])
	}
}

// E-PENPAL-API-ROUTES: verifies POST /api/open adds new standalone project.
func TestAPIOpen_NewDirectory(t *testing.T) {
	s, _, _ := testServer(t)
	dir := t.TempDir()

	body, _ := json.Marshal(map[string]string{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/open", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if resp["url"] == "" {
		t.Error("expected non-empty url")
	}
}

// E-PENPAL-FILE-HANDLER-EVENT: verifies POST /api/open auto-adds a standalone .md file not in any project.
func TestAPIOpen_StandaloneMarkdownFile(t *testing.T) {
	s, _, _ := testServer(t)

	// Create a .md file in a directory that is not registered as a project
	dir := t.TempDir()
	filePath := filepath.Join(dir, "notes.md")
	if err := os.WriteFile(filePath, []byte("# Notes"), 0o644); err != nil {
		t.Fatal(err)
	}

	body, _ := json.Marshal(map[string]string{"path": filePath})
	req := httptest.NewRequest(http.MethodPost, "/api/open", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	// Should return a file URL containing the filename
	if resp["url"] == "" {
		t.Fatal("expected non-empty url")
	}
	if !filepath.IsAbs(filePath) {
		t.Fatal("test setup: filePath should be absolute")
	}
	// The URL should reference the file, not just a project
	if !strings.Contains(resp["url"], "notes.md") {
		t.Errorf("expected url to contain 'notes.md', got %q", resp["url"])
	}
}

// E-PENPAL-FILE-HANDLER-EVENT: verifies POST /api/open rejects non-.md files.
func TestAPIOpen_RejectsNonMarkdown(t *testing.T) {
	s, _, _ := testServer(t)

	dir := t.TempDir()
	filePath := filepath.Join(dir, "readme.txt")
	if err := os.WriteFile(filePath, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	body, _ := json.Marshal(map[string]string{"path": filePath})
	req := httptest.NewRequest(http.MethodPost, "/api/open", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400 for non-.md file, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-FILE-HANDLER-PLIST: verifies Info.plist registers Penpal as an alternate markdown handler.
func TestMacOSInfoPlist_FileHandlerRegistration(t *testing.T) {
	plistPath := filepath.Join("..", "..", "frontend", "src-tauri", "Info.plist")
	contents, err := os.ReadFile(plistPath)
	if err != nil {
		t.Fatalf("read Info.plist: %v", err)
	}
	plist := string(contents)
	for _, fragment := range []string{
		"CFBundleDocumentTypes",
		"net.daringfireball.markdown",
		"<string>md</string>",
		"<string>markdown</string>",
		"LSHandlerRank",
		"<string>Alternate</string>",
	} {
		if !strings.Contains(plist, fragment) {
			t.Fatalf("expected Info.plist to contain %q", fragment)
		}
	}
}

// E-PENPAL-DELETE-FILE: verifies POST /api/delete-file removes file from disk.
func TestAPIDeleteFile_Success(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	filePath := filepath.Join(dir, "todelete.md")
	if err := os.WriteFile(filePath, []byte("delete me"), 0o644); err != nil {
		t.Fatal(err)
	}
	seedProject(c, "test-proj", dir, nil)

	req := httptest.NewRequest(http.MethodPost, "/api/delete-file?project=test-proj&path=todelete.md", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	if _, err := os.Stat(filePath); !os.IsNotExist(err) {
		t.Error("expected file to be deleted")
	}
}

// E-PENPAL-DELETE-PROJECT: verifies POST /api/delete-project removes directory via os.RemoveAll.
func TestAPIDeleteProject_Success(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	projDir := filepath.Join(dir, "myproject")
	if err := os.MkdirAll(projDir, 0o755); err != nil {
		t.Fatal(err)
	}
	seedProject(c, "test-proj", projDir, nil)
	s.cfg.Projects = append(s.cfg.Projects, config.ProjectConfig{Path: projDir})

	req := httptest.NewRequest(http.MethodPost, "/api/delete-project?name=test-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	if _, err := os.Stat(projDir); !os.IsNotExist(err) {
		t.Error("expected project directory to be deleted")
	}
}

// E-PENPAL-API-ROUTES: verifies /api/open uses longest-prefix matching for sub-project.
func TestAPIOpen_PrefersSubProjectOverRoot(t *testing.T) {
	s, c, _ := testServer(t)

	// Create a workspace directory structure with (root) and a sub-project
	wsDir := t.TempDir()
	subDir := filepath.Join(wsDir, "subproj")
	if err := os.MkdirAll(subDir, 0o755); err != nil {
		t.Fatal(err)
	}

	// Create a .md file under the sub-project
	filePath := filepath.Join(subDir, "doc.md")
	if err := os.WriteFile(filePath, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	// Seed both (root) (workspace root) and subproj (more specific path).
	// (root) is seeded first to simulate the alphabetical ordering bug.
	seedProject(c, "ws/(root)", wsDir, nil)
	seedProject(c, "ws/subproj", subDir, []cache.FileInfo{{FullPath: "doc.md"}})

	body, _ := json.Marshal(map[string]string{"path": filePath})
	req := httptest.NewRequest(http.MethodPost, "/api/open", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	// Should route to the sub-project, not (root)
	expected := "/file/ws/subproj/doc.md"
	if resp["url"] != expected {
		t.Errorf("expected url %q, got %q", expected, resp["url"])
	}
}

// E-PENPAL-DELETE-FILE: verifies sidecar cleanup and removeEmptyParents after file deletion.
func TestAPIDeleteFile_CleansUpCommentSidecar(t *testing.T) {
	s, c, cs := testServer(t)

	dir := t.TempDir()
	filePath := filepath.Join(dir, "reviewed.md")
	if err := os.WriteFile(filePath, []byte("content"), 0o644); err != nil {
		t.Fatal(err)
	}
	seedProject(c, "test-proj", dir, nil)

	// Create a comment thread on the file
	_, err := cs.CreateThread("test-proj", "reviewed.md", comments.Anchor{
		SelectedText: "content",
	}, comments.Comment{
		Author: "alice",
		Role:   "human",
		Body:   "Needs work",
	})
	if err != nil {
		t.Fatalf("CreateThread: %v", err)
	}

	// Verify the sidecar exists
	sidecarPath := filepath.Join(dir, ".penpal", "comments", "reviewed.md.json")
	if _, err := os.Stat(sidecarPath); err != nil {
		t.Fatalf("expected sidecar to exist: %v", err)
	}

	// Delete the file via API
	req := httptest.NewRequest(http.MethodPost, "/api/delete-file?project=test-proj&path=reviewed.md", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	// The .md file should be gone
	if _, err := os.Stat(filePath); !os.IsNotExist(err) {
		t.Error("expected file to be deleted")
	}

	// The sidecar should also be gone
	if _, err := os.Stat(sidecarPath); !os.IsNotExist(err) {
		t.Error("expected comment sidecar to be deleted")
	}

	// ListFilesInReview should return empty
	files, err := cs.ListFilesInReview("test-proj")
	if err != nil {
		t.Fatalf("ListFilesInReview: %v", err)
	}
	if len(files) != 0 {
		t.Errorf("expected 0 files in review, got %d", len(files))
	}
}

// E-PENPAL-ADD-SOURCE: verifies __all_markdown__ source doesn't block manual file additions.
// The conflict check must skip __all_markdown__ since it covers the entire project tree.
func TestAPISources_AddFileNotBlockedByAllMarkdown(t *testing.T) {
	s, _, _ := testServer(t)

	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "notes.md"), []byte("# Notes"), 0o644)

	// Register as standalone with __all_markdown__ source (always present in real projects)
	s.cfg.Projects = append(s.cfg.Projects, config.ProjectConfig{Path: dir})
	s.refreshAfterConfigChange()

	projName := filepath.Base(dir)

	// Verify the project has __all_markdown__ source
	project := s.cache.FindProject(projName)
	if project == nil {
		t.Fatal("project not found after refresh")
	}
	hasAllMD := false
	for _, src := range project.Sources {
		if src.Name == "__all_markdown__" {
			hasAllMD = true
			break
		}
	}
	if !hasAllMD {
		t.Fatal("project should have __all_markdown__ source")
	}

	// Adding a file should succeed (not 409) even though __all_markdown__ covers it
	body, _ := json.Marshal(map[string]string{
		"project": projName,
		"path":    "notes.md",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d: %s — __all_markdown__ should not block file additions", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-REMOVE-SOURCE: verifies adding a tree source then removing it via DELETE /api/sources.
func TestAPISources_RemoveTreeSource(t *testing.T) {
	s, _, _ := testServer(t)

	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "docs"), 0o755); err != nil {
		t.Fatal(err)
	}
	// Register as a standalone project in config so it survives refreshAfterConfigChange
	s.cfg.Projects = append(s.cfg.Projects, config.ProjectConfig{Path: dir})
	s.refreshAfterConfigChange()

	projName := filepath.Base(dir)

	// Add a tree source
	body, _ := json.Marshal(map[string]string{
		"project": projName,
		"path":    "docs",
		"name":    "docs",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("add tree: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	// Remove the tree source by name
	body, _ = json.Marshal(map[string]string{
		"project": projName,
		"name":    "docs",
	})
	req = httptest.NewRequest(http.MethodDelete, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("remove tree: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	// Removing again should 404
	body, _ = json.Marshal(map[string]string{
		"project": projName,
		"name":    "docs",
	})
	req = httptest.NewRequest(http.MethodDelete, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("remove again: expected 404, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-REMOVE-SOURCE: verifies adding file sources then removing individual files.
func TestAPISources_RemoveFileSource(t *testing.T) {
	s, _, _ := testServer(t)

	dir := t.TempDir()
	// Create markdown files
	for _, name := range []string{"a.md", "b.md"} {
		if err := os.WriteFile(filepath.Join(dir, name), []byte("# "+name), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	// Register as a standalone project in config so it survives refreshAfterConfigChange
	s.cfg.Projects = append(s.cfg.Projects, config.ProjectConfig{Path: dir})
	s.refreshAfterConfigChange()

	projName := filepath.Base(dir)

	// Add first file
	body, _ := json.Marshal(map[string]string{
		"project": projName,
		"path":    "a.md",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("add a.md: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	// Add second file
	body, _ = json.Marshal(map[string]string{
		"project": projName,
		"path":    "b.md",
	})
	req = httptest.NewRequest(http.MethodPost, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("add b.md: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	// Remove just a.md
	body, _ = json.Marshal(map[string]string{
		"project": projName,
		"file":    "a.md",
	})
	req = httptest.NewRequest(http.MethodDelete, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("remove a.md: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	// Remove a.md again should 404
	body, _ = json.Marshal(map[string]string{
		"project": projName,
		"file":    "a.md",
	})
	req = httptest.NewRequest(http.MethodDelete, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("remove a.md again: expected 404, got %d: %s", rec.Code, rec.Body.String())
	}

	// Remove b.md should still succeed
	body, _ = json.Marshal(map[string]string{
		"project": projName,
		"file":    "b.md",
	})
	req = httptest.NewRequest(http.MethodDelete, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("remove b.md: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-REMOVE-SOURCE: verifies auto-detected sources cannot be removed.
func TestAPISources_CannotRemoveAutoDetected(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	// Seed a project with an auto-detected source
	project := seedProject(c, "test-proj", dir, nil)
	project.Sources = []discovery.FileSource{{
		Name: "thoughts",
		Type: "thoughts",
		Auto: true,
	}}
	// Re-set the project in cache with the source
	c.SetProjects([]discovery.Project{project})

	// Attempt to remove the auto-detected source
	body, _ := json.Marshal(map[string]string{
		"project": "test-proj",
		"name":    "thoughts",
	})
	req := httptest.NewRequest(http.MethodDelete, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400 for auto-detected source removal, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-REMOVE-SOURCE: verifies DELETE /api/sources requires project field.
func TestAPISources_RemoveRequiresProject(t *testing.T) {
	s, _, _ := testServer(t)

	body, _ := json.Marshal(map[string]string{
		"name": "docs",
	})
	req := httptest.NewRequest(http.MethodDelete, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-REMOVE-SOURCE: verifies DELETE /api/sources requires name or file.
func TestAPISources_RemoveRequiresNameOrFile(t *testing.T) {
	s, _, _ := testServer(t)

	body, _ := json.Marshal(map[string]string{
		"project": "test-proj",
	})
	req := httptest.NewRequest(http.MethodDelete, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d: %s", rec.Code, rec.Body.String())
	}
}
