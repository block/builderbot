package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"time"

	"github.com/loganj/penpal/internal/config"
)

func main() {
	args := os.Args[1:]
	if len(args) == 0 {
		fmt.Fprintf(os.Stderr, "Usage: penpal open <path>...\n")
		os.Exit(1)
	}

	switch args[0] {
	case "open":
		if len(args) < 2 {
			fmt.Fprintf(os.Stderr, "Usage: penpal open <path>...\n")
			os.Exit(1)
		}
		runOpen(args[1:])
	default:
		fmt.Fprintf(os.Stderr, "Unknown command: %s\n", args[0])
		fmt.Fprintf(os.Stderr, "Usage: penpal open <path>...\n")
		os.Exit(1)
	}
}

// runOpen opens paths in the Penpal desktop app, launching it if needed.
func runOpen(paths []string) {
	port := config.ReadPortFile()

	// Check if server is already running (desktop app is open)
	if port > 0 && isServerRunning(port) {
		openPaths(port, paths)
		return
	}

	// No running server — launch the desktop app, which starts its own sidecar
	openApp()

	// Wait for the app's sidecar server to become ready.
	// Re-read the port file each iteration so we pick up the fresh port the
	// new server writes, instead of polling a stale value from a prior run.
	port = waitForServerStart(10 * time.Second)
	if port <= 0 {
		fmt.Fprintf(os.Stderr, "Error: server did not start within timeout\n")
		os.Exit(1)
	}

	openPaths(port, paths)
}

// openPaths sends each path to the /api/open endpoint, then opens the desktop app.
func openPaths(port int, paths []string) {
	for _, arg := range paths {
		absPath, err := filepath.Abs(arg)
		if err != nil {
			fmt.Fprintf(os.Stderr, "Warning: could not resolve path %q: %v\n", arg, err)
			continue
		}

		body, _ := json.Marshal(map[string]string{"path": absPath})
		resp, err := http.Post(
			fmt.Sprintf("http://localhost:%d/api/open", port),
			"application/json",
			bytes.NewReader(body),
		)
		if err != nil {
			fmt.Fprintf(os.Stderr, "Warning: could not contact server for %q: %v\n", arg, err)
			continue
		}

		if resp.StatusCode != http.StatusOK {
			var errResp struct {
				Error string `json:"error"`
			}
			json.NewDecoder(resp.Body).Decode(&errResp)
			resp.Body.Close()
			fmt.Fprintf(os.Stderr, "Warning: server error for %q: %s\n", arg, errResp.Error)
			continue
		}
		resp.Body.Close()
	}

	// Open/focus the desktop app (deep link navigation is a future enhancement)
	openApp()
}

// isServerRunning checks if a penpal server is responding at the given port.
func isServerRunning(port int) bool {
	client := &http.Client{Timeout: 2 * time.Second}
	resp, err := client.Get(fmt.Sprintf("http://localhost:%d/api/projects", port))
	if err != nil {
		return false
	}
	resp.Body.Close()
	return resp.StatusCode == http.StatusOK
}

// waitForServerStart polls for a newly launched server by re-reading the port
// file each iteration. This avoids polling a stale port left behind by a prior
// crash or dev run. Returns the port the server is running on, or 0 on timeout.
func waitForServerStart(timeout time.Duration) int {
	const defaultPort = 8080
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		port := config.ReadPortFile()
		if port <= 0 {
			port = defaultPort
		}
		if isServerRunning(port) {
			return port
		}
		time.Sleep(200 * time.Millisecond)
	}
	return 0
}

// openApp opens (or focuses) the Penpal desktop app.
func openApp() {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", "-a", "Penpal")
	case "linux":
		cmd = exec.Command("xdg-open", "penpal://")
	default:
		fmt.Fprintf(os.Stderr, "Error: unsupported platform %s\n", runtime.GOOS)
		return
	}
	if err := cmd.Run(); err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not open Penpal app: %v\n", err)
		fmt.Fprintf(os.Stderr, "Is Penpal.app installed? Run: just install\n")
	}
}
