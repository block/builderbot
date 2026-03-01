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

	runServe(*port, *root)
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
	addr := fmt.Sprintf("127.0.0.1:%d", port)

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
