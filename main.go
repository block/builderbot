package main

import (
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"path/filepath"

	"github.com/loganj/birdseye/internal/discovery"
	"github.com/loganj/birdseye/internal/server"
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

	fmt.Printf("Scanning %s for projects with thoughts/ directories...\n", rootDir)

	projects, err := discovery.FindProjects(rootDir)
	if err != nil {
		log.Fatal(err)
	}

	fmt.Printf("Found %d projects\n", len(projects))
	for _, p := range projects {
		if p.Git != nil {
			fmt.Printf("  - %s (%s, %d files)\n", p.Name, p.Git.Branch, p.FileCount)
		} else {
			fmt.Printf("  - %s (%d files)\n", p.Name, p.FileCount)
		}
	}

	srv := server.New(rootDir, projects)
	addr := fmt.Sprintf(":%d", *port)
	fmt.Printf("\nStarting server at http://localhost%s\n", addr)
	log.Fatal(http.ListenAndServe(addr, srv))
}
