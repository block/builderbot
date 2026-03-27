package server

import (
	"net/http"
	"os"
	"path/filepath"
	"strings"
)

// spaHandler serves a Single Page Application from a directory on disk.
// It serves static files when they exist and falls back to index.html for
// any path that doesn't match a real file (enabling client-side routing).
// If the directory doesn't exist, all requests return 404.
// E-PENPAL-SPA-SERVE: SPA from frontend/dist/ at /app/, fallback to index.html, path traversal blocked.
type spaHandler struct {
	dir    string
	prefix string // URL prefix to strip (e.g. "/app" when mounted at /app/)
}

func newSPAHandler(dir string, prefix string) *spaHandler {
	return &spaHandler{dir: dir, prefix: prefix}
}

func (h *spaHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet && r.Method != http.MethodHead {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	// If no directory is configured, or it doesn't exist, return 404
	if h.dir == "" {
		http.NotFound(w, r)
		return
	}
	if _, err := os.Stat(h.dir); os.IsNotExist(err) {
		http.NotFound(w, r)
		return
	}

	// Clean the path and prevent traversal
	urlPath := strings.TrimPrefix(r.URL.Path, h.prefix)
	if urlPath == "" || urlPath == "/" {
		urlPath = "/index.html"
	}
	cleanPath := filepath.FromSlash(filepath.Clean(urlPath))

	// Try to serve the requested file
	filePath := filepath.Join(h.dir, cleanPath)

	// Ensure the resolved path is within the dist directory
	absDir, _ := filepath.Abs(h.dir)
	absFile, _ := filepath.Abs(filePath)
	if !strings.HasPrefix(absFile, absDir+string(filepath.Separator)) && absFile != absDir {
		http.NotFound(w, r)
		return
	}

	info, err := os.Stat(filePath)
	if err == nil && !info.IsDir() {
		http.ServeFile(w, r, filePath)
		return
	}

	// SPA fallback: serve index.html for client-side routing
	indexPath := filepath.Join(h.dir, "index.html")
	if _, err := os.Stat(indexPath); err != nil {
		http.NotFound(w, r)
		return
	}
	http.ServeFile(w, r, indexPath)
}
