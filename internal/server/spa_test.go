package server

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
)

func TestSPAHandler_NoDistDir(t *testing.T) {
	h := newSPAHandler("")
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/app/", nil)
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusNotFound {
		t.Errorf("expected 404 for empty dir, got %d", rec.Code)
	}
}

func TestSPAHandler_MissingDistDir(t *testing.T) {
	h := newSPAHandler("/nonexistent/path/frontend/dist")
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/app/", nil)
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusNotFound {
		t.Errorf("expected 404 for missing dir, got %d", rec.Code)
	}
}

func TestSPAHandler_ServesIndexHTML(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "index.html"), []byte("<html>SPA</html>"), 0644)

	h := newSPAHandler(dir)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/app/", nil)
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Errorf("expected 200, got %d", rec.Code)
	}
	if body := rec.Body.String(); body != "<html>SPA</html>" {
		t.Errorf("unexpected body: %s", body)
	}
}

func TestSPAHandler_ServesStaticFile(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "index.html"), []byte("<html>SPA</html>"), 0644)
	os.MkdirAll(filepath.Join(dir, "assets"), 0755)
	os.WriteFile(filepath.Join(dir, "assets", "app.js"), []byte("console.log('hi')"), 0644)

	h := newSPAHandler(dir)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/app/assets/app.js", nil)
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Errorf("expected 200, got %d", rec.Code)
	}
	if body := rec.Body.String(); body != "console.log('hi')" {
		t.Errorf("unexpected body: %s", body)
	}
}

func TestSPAHandler_FallbackToIndex(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "index.html"), []byte("<html>SPA</html>"), 0644)

	h := newSPAHandler(dir)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/app/workspace/default", nil)
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Errorf("expected 200 (SPA fallback), got %d", rec.Code)
	}
	if body := rec.Body.String(); body != "<html>SPA</html>" {
		t.Errorf("expected index.html content, got: %s", body)
	}
}

func TestSPAHandler_MethodNotAllowed(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "index.html"), []byte("<html>SPA</html>"), 0644)

	h := newSPAHandler(dir)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPost, "/app/", nil)
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusMethodNotAllowed {
		t.Errorf("expected 405, got %d", rec.Code)
	}
}

func TestSPAHandler_PathTraversal(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "index.html"), []byte("<html>SPA</html>"), 0644)

	// Create a file outside the dist directory
	parent := filepath.Dir(dir)
	os.WriteFile(filepath.Join(parent, "secret.txt"), []byte("secret"), 0644)

	h := newSPAHandler(dir)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/app/../secret.txt", nil)
	h.ServeHTTP(rec, req)
	// Should either 404 or serve index.html (SPA fallback), not the secret file
	body := rec.Body.String()
	if body == "secret" {
		t.Error("path traversal succeeded - served file outside dist directory")
	}
}

func TestSPAHandler_IntegrationWithServer(t *testing.T) {
	s, _, _ := testServer(t)

	// /app/ should return 404 when frontend/dist doesn't exist
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/app/", nil)
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNotFound {
		t.Errorf("expected 404 when frontend/dist missing, got %d", rec.Code)
	}
}

func TestSPAHandler_RedirectAppToAppSlash(t *testing.T) {
	s, _, _ := testServer(t)

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/app", nil)
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusMovedPermanently {
		t.Errorf("expected 301 redirect, got %d", rec.Code)
	}
	if loc := rec.Header().Get("Location"); loc != "/app/" {
		t.Errorf("expected redirect to /app/, got %s", loc)
	}
}
