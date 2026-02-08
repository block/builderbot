package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/loganj/birdseye/internal/agents"
	"github.com/loganj/birdseye/internal/cache"
	"github.com/loganj/birdseye/internal/comments"
	"github.com/loganj/birdseye/internal/config"
	"github.com/loganj/birdseye/internal/mcpserver"
	"github.com/loganj/birdseye/internal/server"
	"github.com/loganj/birdseye/internal/watcher"
)

func main() {
	port := flag.Int("port", 8080, "port to listen on")
	root := flag.String("root", "", "root directory (deprecated, use config file)")
	dev := flag.Bool("dev", false, "development mode: reload templates from disk on each request")
	flag.Parse()

	if flag.NArg() > 0 {
		// Paths provided: open them in a running instance (Phase 6)
		fmt.Fprintf(os.Stderr, "CLI open mode not yet implemented\n")
		os.Exit(1)
	}

	runServe(*port, *dev, *root)
}

func runServe(port int, dev bool, rootOverride string) {
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

	agents.StartPolling()
	defer agents.StopPolling()

	c := cache.New()
	cs := comments.NewStore(c)

	w, err := watcher.New(c)
	if err != nil {
		log.Fatalf("Failed to create watcher: %v", err)
	}
	defer w.Stop()

	var templateDir string
	if dev {
		templateDir = "templates"
	}

	mcpHandler := mcpserver.NewHandler(cs, c)
	srv := server.New(c, w, cs, mcpHandler, templateDir, cfg)
	addr := fmt.Sprintf(":%d", port)

	// Write .mcp.json for MCP client discovery
	mcpConfig := map[string]interface{}{
		"mcpServers": map[string]interface{}{
			"birdseye": map[string]interface{}{
				"url": fmt.Sprintf("http://localhost:%d/mcp", port),
			},
		},
	}
	mcpJSON, _ := json.MarshalIndent(mcpConfig, "", "  ")
	os.WriteFile(".mcp.json", mcpJSON, 0644)

	httpServer := &http.Server{
		Addr:    addr,
		Handler: srv,
	}

	// Graceful shutdown on SIGINT/SIGTERM
	done := make(chan os.Signal, 1)
	signal.Notify(done, os.Interrupt, syscall.SIGTERM)

	go func() {
		fmt.Printf("\nStarting server at http://localhost%s\n", addr)
		fmt.Printf("birdseye MCP server: http://localhost%s/mcp\n", addr)
		if err := httpServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Fatal(err)
		}
	}()

	<-done
	fmt.Println("\nShutting down...")
	w.Stop()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	httpServer.Shutdown(ctx)
}
