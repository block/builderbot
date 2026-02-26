package config

import (
	"log"
	"os"
	"path/filepath"
)

// MigrateFromBirdseye renames legacy config and data directories.
// Call this once at startup before loading config.
func MigrateFromBirdseye() {
	migrateConfigDir()
}

// migrateConfigDir renames ~/.config/birdseye to ~/.config/penpal if needed.
func migrateConfigDir() {
	home, err := os.UserHomeDir()
	if err != nil {
		return
	}
	oldDir := filepath.Join(home, ".config", "birdseye")
	newDir := filepath.Join(home, ".config", "penpal")

	if _, err := os.Stat(oldDir); err != nil {
		return // nothing to migrate
	}
	if _, err := os.Stat(newDir); err == nil {
		return // new dir already exists, don't clobber
	}

	if err := os.Rename(oldDir, newDir); err != nil {
		log.Printf("Warning: could not migrate config directory: %v", err)
		return
	}
	log.Printf("Migrated config from %s to %s", oldDir, newDir)
}

// MigrateProjectDir renames .birdseye to .penpal within a project directory.
func MigrateProjectDir(projectPath string) {
	oldDir := filepath.Join(projectPath, ".birdseye")
	newDir := filepath.Join(projectPath, ".penpal")

	if _, err := os.Stat(oldDir); err != nil {
		return
	}
	if _, err := os.Stat(newDir); err == nil {
		return
	}

	if err := os.Rename(oldDir, newDir); err != nil {
		log.Printf("Warning: could not migrate %s: %v", oldDir, err)
		return
	}
	log.Printf("Migrated %s to %s", oldDir, newDir)
}
