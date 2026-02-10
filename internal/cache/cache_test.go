package cache

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/loganj/birdseye/internal/discovery"
)

func TestScanProjectSources_ClassifiesFiles(t *testing.T) {
	// Create temporary project structure
	tmpDir := t.TempDir()
	projectPath := tmpDir

	// Create .rp1 structure
	rp1Path := filepath.Join(projectPath, ".rp1")
	os.MkdirAll(filepath.Join(rp1Path, "context"), 0755)
	os.MkdirAll(filepath.Join(rp1Path, "work", "features", "test-feature"), 0755)
	os.MkdirAll(filepath.Join(rp1Path, "work", "prds"), 0755)
	os.MkdirAll(filepath.Join(rp1Path, "work", "quick-builds"), 0755)

	// Create test files
	contextFile := filepath.Join(rp1Path, "context", "index.md")
	featureFile := filepath.Join(rp1Path, "work", "features", "test-feature", "requirements.md")
	prdFile := filepath.Join(rp1Path, "work", "prds", "my-prd.md")
	quickBuildFile := filepath.Join(rp1Path, "work", "quick-builds", "build-1.md")
	charterFile := filepath.Join(rp1Path, "work", "charter.md")

	os.WriteFile(contextFile, []byte("# Context"), 0644)
	os.WriteFile(featureFile, []byte("# Requirements"), 0644)
	os.WriteFile(prdFile, []byte("# PRD"), 0644)
	os.WriteFile(quickBuildFile, []byte("# Quick Build"), 0644)
	os.WriteFile(charterFile, []byte("# Charter"), 0644)

	// Create project with rp1 source
	project := &discovery.Project{
		Name: "test-project",
		Path: projectPath,
		Sources: []discovery.FileSource{
			{
				Name:     "rp1",
				Type:     "tree",
				RootPath: rp1Path,
				Auto:     true,
			},
		},
		LastModified: time.Now(),
	}

	// Scan the project
	files := scanProjectSources(project)

	// Verify we got all files
	if len(files) != 5 {
		t.Fatalf("Expected 5 files, got %d", len(files))
	}

	// Create a map for easy lookup
	fileMap := make(map[string]FileInfo)
	for _, f := range files {
		fileMap[f.FullPath] = f
	}

	// Test context file
	if f, ok := fileMap[".rp1/context/index.md"]; ok {
		if f.FileType != "knowledge" {
			t.Errorf("Context file should have FileType 'knowledge', got %q", f.FileType)
		}
		if f.Path != "context/index.md" {
			t.Errorf("Context file Path should be 'context/index.md', got %q", f.Path)
		}
	} else {
		t.Error("Context file not found")
	}

	// Test feature file
	if f, ok := fileMap[".rp1/work/features/test-feature/requirements.md"]; ok {
		if f.FileType != "requirement" {
			t.Errorf("Feature file should have FileType 'requirement', got %q", f.FileType)
		}
		if f.Path != "work/features/test-feature/requirements.md" {
			t.Errorf("Feature file Path should be 'work/features/test-feature/requirements.md', got %q", f.Path)
		}
	} else {
		t.Error("Feature file not found")
	}

	// Test PRD file
	if f, ok := fileMap[".rp1/work/prds/my-prd.md"]; ok {
		if f.FileType != "prd" {
			t.Errorf("PRD file should have FileType 'prd', got %q", f.FileType)
		}
	} else {
		t.Error("PRD file not found")
	}

	// Test Quick Build file
	if f, ok := fileMap[".rp1/work/quick-builds/build-1.md"]; ok {
		if f.FileType != "quick" {
			t.Errorf("Quick Build file should have FileType 'quick', got %q", f.FileType)
		}
	} else {
		t.Error("Quick Build file not found")
	}

	// Test Charter file
	if f, ok := fileMap[".rp1/work/charter.md"]; ok {
		if f.FileType != "charter" {
			t.Errorf("Charter file should have FileType 'charter', got %q", f.FileType)
		}
	} else {
		t.Error("Charter file not found")
	}
}

func TestCache_FindFile(t *testing.T) {
	c := New()
	projectName := "test/project"

	files := []FileInfo{
		{
			Name:     "requirements.md",
			FullPath: ".rp1/work/features/test-feature/requirements.md",
		},
		{
			Name:     "index.md",
			FullPath: ".rp1/context/index.md",
		},
	}

	c.SetProjectFiles(projectName, files)

	tests := []struct {
		name         string
		projectName  string
		filePath     string
		wantFound    bool
		wantFileName string
	}{
		{
			name:         "find feature file",
			projectName:  projectName,
			filePath:     ".rp1/work/features/test-feature/requirements.md",
			wantFound:    true,
			wantFileName: "requirements.md",
		},
		{
			name:         "find context file",
			projectName:  projectName,
			filePath:     ".rp1/context/index.md",
			wantFound:    true,
			wantFileName: "index.md",
		},
		{
			name:        "file not found",
			projectName: projectName,
			filePath:    ".rp1/nonexistent.md",
			wantFound:   false,
		},
		{
			name:        "project not found",
			projectName: "nonexistent/project",
			filePath:    ".rp1/context/index.md",
			wantFound:   false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := c.FindFile(tt.projectName, tt.filePath)
			if tt.wantFound {
				if got == nil {
					t.Errorf("FindFile() returned nil, wanted file")
				} else if got.Name != tt.wantFileName {
					t.Errorf("FindFile() returned file with name %q, want %q", got.Name, tt.wantFileName)
				}
			} else {
				if got != nil {
					t.Errorf("FindFile() returned %v, wanted nil", got)
				}
			}
		})
	}
}
