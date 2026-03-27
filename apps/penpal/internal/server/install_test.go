package server

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/loganj/penpal/internal/config"
)

// E-PENPAL-INSTALL-CLI: verifies GET /api/install-tools returns status JSON.
func TestInstallToolsStatus_ReturnsJSON(t *testing.T) {
	s, _, _ := testServer(t)
	s.installCfg = &installConfig{binDir: t.TempDir()}

	req := httptest.NewRequest(http.MethodGet, "/api/install-tools", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp installToolsResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}

	// CLI should not be installed (empty temp binDir has no symlink)
	if resp.CLI.Installed {
		t.Error("expected CLI not installed")
	}
	// Plugin status depends on host machine — just verify the field exists
}

// E-PENPAL-INSTALL-CLI: verifies existing CLI symlink is detected.
func TestInstallToolsStatus_DetectsExistingSymlink(t *testing.T) {
	s, _, _ := testServer(t)
	binDir := t.TempDir()
	s.installCfg = &installConfig{binDir: binDir}

	// Create a symlink manually
	symlinkPath := filepath.Join(binDir, "penpal")
	target := "/Applications/Penpal.app/Contents/MacOS/penpal-cli"
	if err := os.Symlink(target, symlinkPath); err != nil {
		t.Fatal(err)
	}

	req := httptest.NewRequest(http.MethodGet, "/api/install-tools", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	var resp installToolsResponse
	json.Unmarshal(rec.Body.Bytes(), &resp)

	if !resp.CLI.Installed {
		t.Error("expected CLI installed")
	}
	if resp.CLI.Path != target {
		t.Errorf("expected path %q, got %q", target, resp.CLI.Path)
	}
}

// E-PENPAL-INSTALL-CLI: verifies POST /api/install-tools creates CLI symlink.
func TestInstallToolsInstall_CreatesSymlink(t *testing.T) {
	s, _, _ := testServer(t)
	binDir := t.TempDir()
	appRoot := t.TempDir()

	// Create the fake app bundle structure
	macosDir := filepath.Join(appRoot, "Contents", "MacOS")
	marketplaceDir := filepath.Join(appRoot, "Contents", "Resources", ".claude-plugin")
	os.MkdirAll(macosDir, 0o755)
	os.MkdirAll(marketplaceDir, 0o755)
	os.WriteFile(filepath.Join(marketplaceDir, "marketplace.json"), []byte(`{"name":"penpal","plugins":[]}`), 0o644)
	os.WriteFile(filepath.Join(macosDir, "penpal-cli"), []byte("fake"), 0o755)

	s.installCfg = &installConfig{binDir: binDir, appRoot: appRoot}

	req := httptest.NewRequest(http.MethodPost, "/api/install-tools", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp installToolsResponse
	json.Unmarshal(rec.Body.Bytes(), &resp)

	if !resp.CLI.Installed {
		t.Errorf("expected CLI installed, got error: %s", resp.CLI.Error)
	}

	// Verify symlink exists
	symlinkPath := filepath.Join(binDir, "penpal")
	target, err := os.Readlink(symlinkPath)
	if err != nil {
		t.Fatalf("symlink not created: %v", err)
	}
	expectedTarget := filepath.Join(appRoot, "Contents", "MacOS", "penpal-cli")
	if target != expectedTarget {
		t.Errorf("symlink points to %q, expected %q", target, expectedTarget)
	}
}

// E-PENPAL-INSTALL-CLI: verifies install is idempotent (second call succeeds).
func TestInstallToolsInstall_Idempotent(t *testing.T) {
	s, _, _ := testServer(t)
	binDir := t.TempDir()
	appRoot := t.TempDir()

	macosDir := filepath.Join(appRoot, "Contents", "MacOS")
	marketplaceDir := filepath.Join(appRoot, "Contents", "Resources", ".claude-plugin")
	os.MkdirAll(macosDir, 0o755)
	os.MkdirAll(marketplaceDir, 0o755)
	os.WriteFile(filepath.Join(marketplaceDir, "marketplace.json"), []byte(`{"name":"penpal","plugins":[]}`), 0o644)
	os.WriteFile(filepath.Join(macosDir, "penpal-cli"), []byte("fake"), 0o755)

	s.installCfg = &installConfig{binDir: binDir, appRoot: appRoot}

	// Call POST twice — second call should not error
	for i := 0; i < 2; i++ {
		req := httptest.NewRequest(http.MethodPost, "/api/install-tools", nil)
		rec := httptest.NewRecorder()
		s.ServeHTTP(rec, req)

		if rec.Code != http.StatusOK {
			t.Fatalf("call %d: expected 200, got %d: %s", i+1, rec.Code, rec.Body.String())
		}

		var resp installToolsResponse
		json.Unmarshal(rec.Body.Bytes(), &resp)

		if !resp.CLI.Installed {
			t.Errorf("call %d: expected CLI installed, got error: %s", i+1, resp.CLI.Error)
		}
	}
}

func TestInstallToolsInstall_MethodNotAllowed(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodDelete, "/api/install-tools", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("expected 405, got %d", rec.Code)
	}
}

// E-PENPAL-CLAUDE-PATH: verifies install status includes claudeBin field.
func TestInstallToolsStatus_IncludesClaudeBin(t *testing.T) {
	s, _, _ := testServer(t)
	// Create a fake claude binary and inject it via installCfg
	dir := t.TempDir()
	fakeClaude := filepath.Join(dir, "claude")
	os.WriteFile(fakeClaude, []byte("#!/bin/sh\n"), 0755)

	s.installCfg = &installConfig{binDir: t.TempDir(), claudeBin: fakeClaude}

	req := httptest.NewRequest(http.MethodGet, "/api/install-tools", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	var resp installToolsResponse
	json.Unmarshal(rec.Body.Bytes(), &resp)

	if resp.ClaudeBin != fakeClaude {
		t.Errorf("expected claudeBin %q, got %q", fakeClaude, resp.ClaudeBin)
	}
}

func TestInstallToolsStatus_ClaudeBinEmpty(t *testing.T) {
	s, _, _ := testServer(t)
	s.installCfg = &installConfig{binDir: t.TempDir(), claudeBin: ""}

	req := httptest.NewRequest(http.MethodGet, "/api/install-tools", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	var resp installToolsResponse
	json.Unmarshal(rec.Body.Bytes(), &resp)

	if resp.ClaudeBin != "" {
		t.Errorf("expected empty claudeBin, got %q", resp.ClaudeBin)
	}
}

// E-PENPAL-INSTALL-PLUGIN: verifies plugin install fails when claude binary is missing.
func TestPerformInstall_NoClaudeBin_ReportsError(t *testing.T) {
	appRoot := t.TempDir()
	macosDir := filepath.Join(appRoot, "Contents", "MacOS")
	marketplaceDir := filepath.Join(appRoot, "Contents", "Resources", ".claude-plugin")
	os.MkdirAll(macosDir, 0o755)
	os.MkdirAll(marketplaceDir, 0o755)
	os.WriteFile(filepath.Join(marketplaceDir, "marketplace.json"), []byte(`{}`), 0o644)
	os.WriteFile(filepath.Join(macosDir, "penpal-cli"), []byte("fake"), 0o755)

	cfg := installConfig{binDir: t.TempDir(), appRoot: appRoot, claudeBin: ""}
	resp := performInstall(cfg)

	if resp.Plugin.Installed {
		t.Error("expected plugin not installed when claudeBin is empty")
	}
	if !strings.Contains(resp.Plugin.Error, "claude binary not found") {
		t.Errorf("expected 'claude binary not found' error, got %q", resp.Plugin.Error)
	}
}

// E-PENPAL-CLAUDE-PATH: verifies PUT /api/claude-path saves and persists valid path.
func TestClaudePath_PUT_Valid(t *testing.T) {
	s, _, _ := testServer(t)
	cfgPath := filepath.Join(t.TempDir(), "config.json")
	s.cfgPath = cfgPath

	// Create a fake claude binary
	dir := t.TempDir()
	fakeClaude := filepath.Join(dir, "claude")
	os.WriteFile(fakeClaude, []byte("#!/bin/sh\n"), 0755)

	body := `{"path":"` + fakeClaude + `"}`
	req := httptest.NewRequest(http.MethodPut, "/api/claude-path", strings.NewReader(body))
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	json.Unmarshal(rec.Body.Bytes(), &resp)
	if resp["path"] != fakeClaude {
		t.Errorf("expected path %q, got %q", fakeClaude, resp["path"])
	}

	// Verify it was persisted
	saved, err := config.Load(cfgPath)
	if err != nil {
		t.Fatalf("load config: %v", err)
	}
	if saved.ClaudePath != fakeClaude {
		t.Errorf("expected persisted claudePath %q, got %q", fakeClaude, saved.ClaudePath)
	}
}

// E-PENPAL-CLAUDE-PATH: verifies PUT /api/claude-path rejects invalid path.
func TestClaudePath_PUT_InvalidPath(t *testing.T) {
	s, _, _ := testServer(t)

	body := `{"path":"/nonexistent/path/to/claude"}`
	req := httptest.NewRequest(http.MethodPut, "/api/claude-path", strings.NewReader(body))
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-CLAUDE-PATH: verifies PUT /api/claude-path rejects empty path.
func TestClaudePath_PUT_EmptyPath(t *testing.T) {
	s, _, _ := testServer(t)

	body := `{"path":""}`
	req := httptest.NewRequest(http.MethodPut, "/api/claude-path", strings.NewReader(body))
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-CLAUDE-PATH: verifies GET /api/claude-path returns remembered path.
func TestClaudePath_GET(t *testing.T) {
	s, _, _ := testServer(t)
	// Pre-set a remembered path
	dir := t.TempDir()
	fakeClaude := filepath.Join(dir, "claude")
	os.WriteFile(fakeClaude, []byte("#!/bin/sh\n"), 0755)
	s.cfg.ClaudePath = fakeClaude

	req := httptest.NewRequest(http.MethodGet, "/api/claude-path", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	json.Unmarshal(rec.Body.Bytes(), &resp)
	if resp["path"] != fakeClaude {
		t.Errorf("expected path %q, got %q", fakeClaude, resp["path"])
	}
}

func TestClaudePath_MethodNotAllowed(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodDelete, "/api/claude-path", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("expected 405, got %d", rec.Code)
	}
}
