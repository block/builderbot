package server

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
)

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
