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
	goPort := flag.Int("go-port", 8081, "port for Go template UI")
	root := flag.String("root", "", "root directory (deprecated, use config file)")
	dev := flag.Bool("dev", false, "development mode: reload templates from disk on each request")
	flag.Parse()

	if flag.NArg() > 0 {
		runOpen(flag.Args(), *port)
		return
	}

	runServe(*port, *goPort, *dev, *root)
}

func runServe(port int, goPort int, dev bool, rootOverride string) {
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

	var templateDir string
	if dev {
		templateDir = "templates"
	}

	am := agents.New(c, cs, port)
	mcpHandler := mcpserver.NewHandler(cs, c)
	srv := server.New(c, w, cs, mcpHandler, am, act, templateDir, cfg, cfgPath)
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

	goAddr := fmt.Sprintf(":%d", goPort)
	goHTTPServer := &http.Server{
		Addr:    goAddr,
		Handler: srv.GoHandler(),
	}

	// Graceful shutdown on SIGINT/SIGTERM
	done := make(chan os.Signal, 1)
	signal.Notify(done, os.Interrupt, syscall.SIGTERM)

	go func() {
		fmt.Printf("\nStarting server at http://localhost%s\n", addr)
		fmt.Printf("Go template UI:    http://localhost%s\n", goAddr)
		fmt.Printf("penpal MCP server: http://localhost%s/mcp\n", addr)
		if err := httpServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Fatal(err)
		}
	}()

	go func() {
		if err := goHTTPServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Printf("Go template server error: %v", err)
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
	goHTTPServer.Shutdown(ctx)
}

// runOpen opens paths in a running penpal instance, starting the server if needed.
func runOpen(paths []string, portFlag int) {
	port := config.ReadPortFile()

	// Check if server is already running at that port
	if port > 0 && isServerRunning(port) {
		openPaths(port, paths)
		return
	}

	// No running server - start one on the fixed port
	port = portFlag

	exe, err := os.Executable()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not determine executable path: %v\n", err)
		os.Exit(1)
	}

	cmd := exec.Command(exe, fmt.Sprintf("-port=%d", port))
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	// Detach the child process so it survives parent exit
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not start server: %v\n", err)
		os.Exit(1)
	}

	// Release the child so it's not reaped when we exit
	cmd.Process.Release()

	fmt.Printf("Started penpal server on port %d (pid %d)\n", port, cmd.Process.Pid)

	// Wait for server to become ready
	if !waitForServer(port, 10*time.Second) {
		fmt.Fprintf(os.Stderr, "Error: server did not start within timeout\n")
		os.Exit(1)
	}

	openPaths(port, paths)
}

// openPaths sends each path to the /api/open endpoint and opens the returned URL in a browser.
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

		var result struct {
			URL string `json:"url"`
		}
		json.NewDecoder(resp.Body).Decode(&result)
		resp.Body.Close()

		if result.URL != "" {
			fullURL := fmt.Sprintf("http://localhost:%d%s", port, result.URL)
			fmt.Printf("Opening %s\n", fullURL)
			openBrowser(fullURL)
		}
	}
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

// openBrowser opens the given URL in the default browser.
func openBrowser(url string) {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", url)
	case "linux":
		cmd = exec.Command("xdg-open", url)
	default:
		fmt.Printf("Open in browser: %s\n", url)
		return
	}
	cmd.Start()
}
