package main

import (
	"context"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	"github.com/loganj/birdseye/internal/cache"
	"github.com/loganj/birdseye/internal/server"
	"github.com/loganj/birdseye/internal/watcher"
)

func main() {
	port := flag.Int("port", 8080, "port to listen on")
	root := flag.String("root", "", "root directory to scan (default: ~/Development)")
	flag.Parse()

	rootDir := *root
	if rootDir == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			log.Fatal(err)
		}
		rootDir = filepath.Join(home, "Development")
	}

	c := cache.New(rootDir)

	w, err := watcher.New(c)
	if err != nil {
		log.Fatalf("Failed to create watcher: %v", err)
	}
	defer w.Stop()

	srv := server.New(c, w)
	addr := fmt.Sprintf(":%d", *port)

	httpServer := &http.Server{
		Addr:    addr,
		Handler: srv,
	}

	// Graceful shutdown on SIGINT/SIGTERM
	done := make(chan os.Signal, 1)
	signal.Notify(done, os.Interrupt, syscall.SIGTERM)

	go func() {
		fmt.Printf("\nStarting server at http://localhost%s\n", addr)
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
