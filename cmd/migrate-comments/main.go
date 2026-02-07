// migrate-comments copies comment sidecar JSON files from the old location
// ({project}/thoughts/.birdseye/comments/{path}.json) to the new location
// ({project}/.birdseye/comments/thoughts/{path}.json).
//
// This is a COPY, not a move. Run the cleanup tool after verifying.
//
// Usage: go run cmd/migrate-comments/main.go [root]
// Default root: ~/Development
package main

import (
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
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

		projectPath := filepath.Join(root, entry.Name())
		oldDir := filepath.Join(projectPath, "thoughts", ".birdseye", "comments")

		info, err := os.Stat(oldDir)
		if err != nil || !info.IsDir() {
			continue
		}

		fmt.Printf("Project: %s\n", entry.Name())

		err = filepath.Walk(oldDir, func(path string, info os.FileInfo, err error) error {
			if err != nil || info.IsDir() || !strings.HasSuffix(path, ".json") {
				return nil
			}

			// Compute relative path from old comments dir
			rel, err := filepath.Rel(oldDir, path)
			if err != nil {
				return nil
			}

			// New path: {project}/.birdseye/comments/thoughts/{rel}
			newPath := filepath.Join(projectPath, ".birdseye", "comments", "thoughts", rel)

			if err := os.MkdirAll(filepath.Dir(newPath), 0755); err != nil {
				fmt.Fprintf(os.Stderr, "  Error creating dir: %v\n", err)
				return nil
			}

			if err := copyFile(path, newPath); err != nil {
				fmt.Fprintf(os.Stderr, "  Error copying %s: %v\n", rel, err)
				return nil
			}

			fmt.Printf("  Copied: %s\n", rel)
			total++
			return nil
		})
		if err != nil {
			fmt.Fprintf(os.Stderr, "  Error walking %s: %v\n", oldDir, err)
		}
	}

	fmt.Printf("\nDone. Copied %d comment files.\n", total)
}

func copyFile(src, dst string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()

	out, err := os.Create(dst)
	if err != nil {
		return err
	}
	defer out.Close()

	_, err = io.Copy(out, in)
	return err
}
