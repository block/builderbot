package server

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

// E-PENPAL-FOCUS: verifies POST /api/focus sets project-level watch.
func TestAPIFocus_Project(t *testing.T) {
	s, _, _ := testServer(t)
	seedProject(s.cache, "ws/proj", t.TempDir(), nil)

	req := httptest.NewRequest(http.MethodPost, "/api/focus?project=ws/proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-FOCUS: verifies POST /api/focus with path sets file-level watch.
func TestAPIFocus_File(t *testing.T) {
	s, _, _ := testServer(t)
	seedProject(s.cache, "ws/proj", t.TempDir(), nil)

	req := httptest.NewRequest(http.MethodPost, "/api/focus?project=ws/proj&path=thoughts/plan.md", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-FOCUS: verifies per-window focus with window ID parameter.
func TestAPIFocus_WindowScoped(t *testing.T) {
	s, _, _ := testServer(t)
	seedProject(s.cache, "ws/proj", t.TempDir(), nil)

	req := httptest.NewRequest(http.MethodPost, "/api/focus?window=win-1&project=ws/proj&path=thoughts/plan.md", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	req = httptest.NewRequest(http.MethodDelete, "/api/focus?window=win-1", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-FOCUS: verifies DELETE /api/focus clears all watches.
func TestAPIFocus_Clear(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodDelete, "/api/focus", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}
}

func TestAPIFocus_MissingProject(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodPost, "/api/focus", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d", rec.Code)
	}
}
