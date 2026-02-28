package main

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"runtime"
	"syscall"
	"time"

	"github.com/loganj/penpal/internal/activity"
	"github.com/loganj/penpal/internal/agents"
	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
	"github.com/loganj/penpal/internal/config"
	"github.com/loganj/penpal/internal/mcpserver"
	"github.com/loganj/penpal/internal/server"
	"github.com/loganj/penpal/internal/watcher"
)

func main() {
	port := flag.Int("port", 8080, "port to listen on")
	root := flag.String("root", "", "root directory (deprecated, use config file)")
	flag.Parse()

	args := flag.Args()
	if len(args) == 0 {
		// No subcommand: start server
		runServe(*port, *root)
		return
	}

	switch args[0] {
	case "open":
		if len(args) < 2 {
			fmt.Fprintf(os.Stderr, "Usage: penpal open <path>...\n")
			os.Exit(1)
		}
		runOpen(args[1:], *port)
	default:
		fmt.Fprintf(os.Stderr, "Unknown command: %s\n", args[0])
		fmt.Fprintf(os.Stderr, "Usage: penpal [open <path>...]\n")
		os.Exit(1)
	}
}

func runServe(port int, rootOverride string) {
	config.MigrateFromBirdseye()
	config.EnsureGlobalGitignore()

	// Load or create config
	cfgPath := config.DefaultConfigPath()
	cfg, err := config.Load(cfgPath)
	if err != nil {
		log.Printf("Warning: could not load config: %v", err)
		cfg = &config.Config{}
	}

	config.EnsureDefaults(cfg, rootOverride)

	if err := config.Save(cfgPath, cfg); err != nil {
		log.Printf("Warning: could not save config: %v", err)
	}

	c := cache.New()
	act := activity.New()
	cs := comments.NewStore(c, act)

	w, err := watcher.New(c, act)
	if err != nil {
		log.Fatalf("Failed to create watcher: %v", err)
	}
	defer w.Stop()

	am := agents.New(c, cs, port)
	mcpHandler := mcpserver.NewHandler(cs, c)
	srv := server.New(c, w, cs, mcpHandler, am, act, cfg, cfgPath)
	addr := fmt.Sprintf(":%d", port)

	// Write .mcp.json for MCP client discovery
	mcpConfig := map[string]interface{}{
		"mcpServers": map[string]interface{}{
			"penpal": map[string]interface{}{
				"type": "http",
				"url":  fmt.Sprintf("http://localhost:%d/mcp", port),
			},
		},
	}
	mcpJSON, _ := json.MarshalIndent(mcpConfig, "", "  ")
	os.WriteFile(".mcp.json", mcpJSON, 0644)

	// Write port file for CLI discovery
	if err := config.WritePortFile(port); err != nil {
		log.Printf("Warning: could not write port file: %v", err)
	}

	httpServer := &http.Server{
		Addr:    addr,
		Handler: srv,
	}

	// Graceful shutdown on SIGINT/SIGTERM
	done := make(chan os.Signal, 1)
	signal.Notify(done, os.Interrupt, syscall.SIGTERM)

	go func() {
		fmt.Printf("\nStarting server at http://localhost%s\n", addr)
		fmt.Printf("penpal MCP server: http://localhost%s/mcp\n", addr)
		if err := httpServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Fatal(err)
		}
	}()

	<-done
	fmt.Println("\nShutting down...")
	am.StopAll()
	config.RemovePortFile()
	w.Stop()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	httpServer.Shutdown(ctx)
}

// runOpen opens paths in the Penpal desktop app, launching it if needed.
func runOpen(paths []string, portFlag int) {
	port := config.ReadPortFile()

	// Check if server is already running (desktop app is open)
	if port > 0 && isServerRunning(port) {
		openPaths(port, paths)
		return
	}

	// No running server — launch the desktop app, which starts its own sidecar
	openApp()

	port = portFlag

	// Wait for the app's sidecar server to become ready
	if !waitForServer(port, 10*time.Second) {
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

// waitForServer polls the server until it responds or the timeout expires.
func waitForServer(port int, timeout time.Duration) bool {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if isServerRunning(port) {
			return true
		}
		time.Sleep(200 * time.Millisecond)
	}
	return false
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
