// cleanup-old-comments removes old comment directories from
// {project}/thoughts/.birdseye/ after migration to the new location.
//
// Run this ONLY after verifying the migration was successful.
//
// Usage: go run cmd/cleanup-old-comments/main.go [root]
// Default root: ~/Development
package main

import (
	"fmt"
	"os"
	"path/filepath"
)

func main() {
	root := ""
	if len(os.Args) > 1 {
		root = os.Args[1]
	}
	if root == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			fmt.Fprintf(os.Stderr, "Error: %v\n", err)
			os.Exit(1)
		}
		root = filepath.Join(home, "Development")
	}

	entries, err := os.ReadDir(root)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error reading %s: %v\n", root, err)
		os.Exit(1)
	}

	total := 0
	for _, entry := range entries {
		if !entry.IsDir() || entry.Name()[0] == '.' {
			continue
		}

		oldDir := filepath.Join(root, entry.Name(), "thoughts", ".birdseye")

		info, err := os.Stat(oldDir)
		if err != nil || !info.IsDir() {
			continue
		}

		fmt.Printf("Removing: %s\n", oldDir)
		if err := os.RemoveAll(oldDir); err != nil {
			fmt.Fprintf(os.Stderr, "  Error: %v\n", err)
			continue
		}
		total++
	}

	fmt.Printf("\nDone. Removed %d old .birdseye directories.\n", total)
}
