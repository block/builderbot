package server

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

// E-PENPAL-CORS: verifies isLocalOrigin allows expected origins.
func TestIsLocalOrigin(t *testing.T) {
	allowed := []string{
		"tauri://localhost",
		"tauri://some-app",
		"https://tauri.localhost",
		"https://tauri.myapp.dev",
		"http://localhost",
		"http://localhost:3000",
		"http://localhost:8080",
		"http://127.0.0.1",
		"http://127.0.0.1:3000",
	}
	for _, origin := range allowed {
		if !isLocalOrigin(origin) {
			t.Errorf("expected isLocalOrigin(%q) = true", origin)
		}
	}

	disallowed := []string{
		"",
		"http://example.com",
		"https://evil.com",
		"http://192.168.1.1:3000",
		"http://0.0.0.0",
		"https://localhost",
		"ftp://localhost",
	}
	for _, origin := range disallowed {
		if isLocalOrigin(origin) {
			t.Errorf("expected isLocalOrigin(%q) = false", origin)
		}
	}
}

// E-PENPAL-CORS: verifies allowed origins get Access-Control-Allow-Origin header.
func TestCORS_AllowedOrigin(t *testing.T) {
	s, _, _ := testServer(t)

	origins := []string{
		"tauri://localhost",
		"http://localhost:3000",
		"http://127.0.0.1:8080",
		"https://tauri.localhost",
	}
	for _, origin := range origins {
		req := httptest.NewRequest(http.MethodGet, "/api/projects", nil)
		req.Header.Set("Origin", origin)
		rec := httptest.NewRecorder()
		s.ServeHTTP(rec, req)

		got := rec.Header().Get("Access-Control-Allow-Origin")
		if got != origin {
			t.Errorf("origin %q: expected Access-Control-Allow-Origin=%q, got %q", origin, origin, got)
		}
		if rec.Header().Get("Access-Control-Allow-Methods") == "" {
			t.Errorf("origin %q: expected Access-Control-Allow-Methods to be set", origin)
		}
		if rec.Header().Get("Access-Control-Allow-Headers") == "" {
			t.Errorf("origin %q: expected Access-Control-Allow-Headers to be set", origin)
		}
	}
}

// E-PENPAL-CORS: verifies disallowed origins get no CORS headers.
func TestCORS_DisallowedOrigin(t *testing.T) {
	s, _, _ := testServer(t)

	origins := []string{
		"http://example.com",
		"https://evil.com",
		"http://192.168.1.1:3000",
	}
	for _, origin := range origins {
		req := httptest.NewRequest(http.MethodGet, "/api/projects", nil)
		req.Header.Set("Origin", origin)
		rec := httptest.NewRecorder()
		s.ServeHTTP(rec, req)

		if got := rec.Header().Get("Access-Control-Allow-Origin"); got != "" {
			t.Errorf("origin %q: expected no Access-Control-Allow-Origin, got %q", origin, got)
		}
	}
}

// E-PENPAL-CORS: verifies OPTIONS preflight with allowed origin returns 204.
func TestCORS_Preflight(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodOptions, "/api/projects", nil)
	req.Header.Set("Origin", "http://localhost:3000")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusNoContent {
		t.Errorf("expected 204 for preflight, got %d", rec.Code)
	}
	if got := rec.Header().Get("Access-Control-Allow-Origin"); got != "http://localhost:3000" {
		t.Errorf("expected Access-Control-Allow-Origin=http://localhost:3000, got %q", got)
	}
	if rec.Body.Len() != 0 {
		t.Errorf("expected empty body for preflight, got %q", rec.Body.String())
	}
}

// E-PENPAL-CORS: verifies OPTIONS preflight with disallowed origin is not short-circuited.
func TestCORS_PreflightDisallowed(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodOptions, "/api/projects", nil)
	req.Header.Set("Origin", "http://evil.com")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	// Should NOT get CORS headers
	if got := rec.Header().Get("Access-Control-Allow-Origin"); got != "" {
		t.Errorf("expected no CORS header for disallowed preflight, got %q", got)
	}
}

// E-PENPAL-CORS: verifies no CORS headers when no Origin header is sent.
func TestCORS_NoOriginHeader(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodGet, "/api/projects", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if got := rec.Header().Get("Access-Control-Allow-Origin"); got != "" {
		t.Errorf("expected no CORS header when Origin is absent, got %q", got)
	}
}
