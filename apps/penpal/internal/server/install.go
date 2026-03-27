package server

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/loganj/penpal/internal/claudepath"
	"github.com/loganj/penpal/internal/config"
)

type installComponentStatus struct {
	Installed bool   `json:"installed"`
	Path      string `json:"path,omitempty"`
	Error     string `json:"error,omitempty"`
}

type installToolsResponse struct {
	CLI       installComponentStatus `json:"cli"`
	Plugin    installComponentStatus `json:"plugin"`
	ClaudeBin string                 `json:"claudeBin,omitempty"` // resolved path to claude binary (empty if not found)
}

// installConfig holds injectable paths for testing.
type installConfig struct {
	binDir    string // override for CLI symlink target directory
	appRoot   string // override for .app bundle root
	claudeBin string // resolved path to claude binary
}

// E-PENPAL-INSTALL-CLI: CLI symlink creation; E-PENPAL-INSTALL-PLUGIN: plugin marketplace install.
func (s *Server) handleInstallTools(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		s.handleInstallToolsStatus(w, r)
	case http.MethodPost:
		s.handleInstallToolsInstall(w, r)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func (s *Server) handleInstallToolsStatus(w http.ResponseWriter, _ *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	cfg := s.getInstallConfig()
	resp := checkInstallStatus(cfg)
	resp.ClaudeBin = cfg.claudeBin
	json.NewEncoder(w).Encode(resp)
}

func (s *Server) handleInstallToolsInstall(w http.ResponseWriter, _ *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	cfg := s.getInstallConfig()
	resp := performInstall(cfg)

	// If plugin installed successfully, remember the claude path
	if resp.Plugin.Installed && cfg.claudeBin != "" {
		s.rememberClaudePath(cfg.claudeBin)
	}

	resp.ClaudeBin = cfg.claudeBin
	json.NewEncoder(w).Encode(resp)
}

func (s *Server) getInstallConfig() installConfig {
	if s.installCfg != nil {
		return *s.installCfg
	}
	return installConfig{
		claudeBin: s.resolveClaudePath(),
	}
}

// resolveClaudePath finds the claude binary, checking the remembered config path first,
// then falling back to PATH and well-known locations.
func (s *Server) resolveClaudePath() string {
	s.cfgMu.Lock()
	remembered := s.cfg.ClaudePath
	s.cfgMu.Unlock()

	resolved := claudepath.Resolve(remembered)

	// If we found it and it differs from what was remembered, persist it
	if resolved != "" && resolved != remembered {
		s.rememberClaudePath(resolved)
	}

	return resolved
}

func (s *Server) rememberClaudePath(path string) {
	s.cfgMu.Lock()
	defer s.cfgMu.Unlock()
	if s.cfg.ClaudePath != path {
		s.cfg.ClaudePath = path
		config.Save(s.cfgPath, s.cfg)
		log.Printf("Remembered claude path: %s", path)
	}
}

// E-PENPAL-CLAUDE-PATH: GET/PUT /api/claude-path with validation.
func (s *Server) handleClaudePath(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]string{
			"path":    s.resolveClaudePath(),
			"version": claudepath.Version(s.resolveClaudePath()),
		})
	case http.MethodPut:
		var body struct {
			Path string `json:"path"`
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			http.Error(w, "bad request", http.StatusBadRequest)
			return
		}
		if body.Path == "" || !claudepath.IsExecutable(body.Path) {
			http.Error(w, "path is not a valid executable", http.StatusBadRequest)
			return
		}
		s.rememberClaudePath(body.Path)
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]string{
			"path":    body.Path,
			"version": claudepath.Version(body.Path),
		})
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

// resolveAppRoot finds the .app bundle root from the current executable path.
// The server binary lives at <app>/Contents/MacOS/penpal-server, so we go up 3 levels.
func resolveAppRoot() (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", fmt.Errorf("could not determine executable path: %w", err)
	}
	exe, err = filepath.EvalSymlinks(exe)
	if err != nil {
		return "", fmt.Errorf("could not resolve executable symlinks: %w", err)
	}
	// Go up from Contents/MacOS/penpal-server to the .app root
	appRoot := filepath.Dir(filepath.Dir(filepath.Dir(exe)))
	if !strings.HasSuffix(appRoot, ".app") {
		return "", fmt.Errorf("executable is not inside an .app bundle: %s", appRoot)
	}
	return appRoot, nil
}

// resolveBinDir returns the best directory for placing the CLI symlink.
// E-PENPAL-INSTALL-CLI: falls back to /usr/local/bin if Homebrew is not found.
func resolveBinDir() string {
	// Try Homebrew prefix first
	if out, err := exec.Command("brew", "--prefix").Output(); err == nil {
		prefix := strings.TrimSpace(string(out))
		binDir := filepath.Join(prefix, "bin")
		if info, err := os.Stat(binDir); err == nil && info.IsDir() {
			return binDir
		}
	}
	return "/usr/local/bin"
}

func checkInstallStatus(cfg installConfig) installToolsResponse {
	resp := installToolsResponse{}

	binDir := cfg.binDir
	if binDir == "" {
		binDir = resolveBinDir()
	}

	symlinkPath := filepath.Join(binDir, "penpal")
	if target, err := os.Readlink(symlinkPath); err == nil {
		resp.CLI.Installed = true
		resp.CLI.Path = target
	}

	if cfg.claudeBin != "" {
		if out, err := exec.Command(cfg.claudeBin, "plugin", "list").Output(); err == nil {
			if strings.Contains(string(out), "penpal") {
				resp.Plugin.Installed = true
			}
		}
	}

	return resp
}

// E-PENPAL-INSTALL-CLI: creates CLI symlink; E-PENPAL-INSTALL-PLUGIN: runs claude plugin install.
func performInstall(cfg installConfig) installToolsResponse {
	resp := installToolsResponse{}

	appRoot := cfg.appRoot
	if appRoot == "" {
		var err error
		appRoot, err = resolveAppRoot()
		if err != nil {
			errMsg := fmt.Sprintf("could not find app bundle: %v", err)
			resp.CLI.Error = errMsg
			resp.Plugin.Error = errMsg
			return resp
		}
	}

	// Install CLI symlink
	binDir := cfg.binDir
	if binDir == "" {
		binDir = resolveBinDir()
	}
	cliSource := filepath.Join(appRoot, "Contents", "MacOS", "penpal-cli")
	symlinkPath := filepath.Join(binDir, "penpal")

	if err := os.MkdirAll(binDir, 0o755); err != nil {
		resp.CLI.Error = fmt.Sprintf("could not create bin directory: %v", err)
	} else {
		// Remove existing symlink/file first
		os.Remove(symlinkPath)
		if err := os.Symlink(cliSource, symlinkPath); err != nil {
			resp.CLI.Error = fmt.Sprintf("could not create symlink: %v", err)
		} else {
			resp.CLI.Installed = true
			resp.CLI.Path = symlinkPath
			log.Printf("CLI symlink created: %s → %s", symlinkPath, cliSource)
		}
	}

	// Install Claude Code plugin via claude CLI
	// The marketplace root is Contents/Resources/ which contains:
	//   .claude-plugin/marketplace.json  (marketplace descriptor)
	//   plugin/                          (the actual plugin)
	marketplaceDir := filepath.Join(appRoot, "Contents", "Resources")
	marketplaceJSON := filepath.Join(marketplaceDir, ".claude-plugin", "marketplace.json")
	if _, err := os.Stat(marketplaceJSON); err != nil {
		resp.Plugin.Error = fmt.Sprintf("marketplace.json not found in app bundle: %v", err)
	} else if cfg.claudeBin == "" {
		resp.Plugin.Error = "claude binary not found; install Claude Code first (https://claude.ai/install.sh)"
	} else {
		if out, err := exec.Command(cfg.claudeBin, "plugin", "marketplace", "add", marketplaceDir).CombinedOutput(); err != nil {
			resp.Plugin.Error = fmt.Sprintf("marketplace add failed: %v (%s)", err, strings.TrimSpace(string(out)))
		} else if out, err := exec.Command(cfg.claudeBin, "plugin", "install", "penpal").CombinedOutput(); err != nil {
			resp.Plugin.Error = fmt.Sprintf("plugin install failed: %v (%s)", err, strings.TrimSpace(string(out)))
		} else {
			resp.Plugin.Installed = true
			log.Printf("Claude Code plugin installed from %s", marketplaceDir)
		}
	}

	return resp
}
