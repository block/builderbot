package cache

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/loganj/birdseye/internal/discovery"
)

func TestFileInfo_DisplayName(t *testing.T) {
	tests := []struct {
		name string
		file FileInfo
		want string
	}{
		{
			name: "feature file",
			file: FileInfo{
				Name:      "requirements.md",
				FeatureID: "rp1-differentiation",
			},
			want: "rp1-differentiation/requirements.md",
		},
		{
			name: "feature file with design",
			file: FileInfo{
				Name:      "design.md",
				FeatureID: "auth-system",
			},
			want: "auth-system/design.md",
		},
		{
			name: "feature file with tasks",
			file: FileInfo{
				Name:      "tasks.md",
				FeatureID: "rp1-auto-discovery",
			},
			want: "rp1-auto-discovery/tasks.md",
		},
		{
			name: "non-feature file",
			file: FileInfo{
				Name:      "index.md",
				FeatureID: "",
			},
			want: "index.md",
		},
		{
			name: "context file",
			file: FileInfo{
				Name:      "architecture.md",
				FeatureID: "",
				Category:  "Context",
			},
			want: "architecture.md",
		},
		{
			name: "prd file",
			file: FileInfo{
				Name:      "my-prd.md",
				FeatureID: "",
				Category:  "PRDs",
			},
			want: "my-prd.md",
		},
		{
			name: "thoughts file",
			file: FileInfo{
				Name:      "plan.md",
				FeatureID: "",
				Source:    "thoughts",
			},
			want: "plan.md",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := tt.file.DisplayName()
			if got != tt.want {
				t.Errorf("DisplayName() = %v, want %v", got, tt.want)
			}
		})
	}
}

func TestScanProjectSources_PopulatesFeatureIDAndCategory(t *testing.T) {
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
		fileMap[f.Name] = f
	}

	// Test context file
	if f, ok := fileMap["index.md"]; ok {
		if f.FeatureID != "" {
			t.Errorf("Context file should have empty FeatureID, got %q", f.FeatureID)
		}
		if f.Category != "Context" {
			t.Errorf("Context file should have Category 'Context', got %q", f.Category)
		}
		if f.DisplayName() != "index.md" {
			t.Errorf("Context file DisplayName should be 'index.md', got %q", f.DisplayName())
		}
	} else {
		t.Error("Context file not found")
	}

	// Test feature file
	if f, ok := fileMap["requirements.md"]; ok {
		if f.FeatureID != "test-feature" {
			t.Errorf("Feature file should have FeatureID 'test-feature', got %q", f.FeatureID)
		}
		if f.Category != "" {
			t.Errorf("Feature file should have empty Category, got %q", f.Category)
		}
		if f.DisplayName() != "test-feature/requirements.md" {
			t.Errorf("Feature file DisplayName should be 'test-feature/requirements.md', got %q", f.DisplayName())
		}
	} else {
		t.Error("Feature file not found")
	}

	// Test PRD file
	if f, ok := fileMap["my-prd.md"]; ok {
		if f.FeatureID != "" {
			t.Errorf("PRD file should have empty FeatureID, got %q", f.FeatureID)
		}
		if f.Category != "PRDs" {
			t.Errorf("PRD file should have Category 'PRDs', got %q", f.Category)
		}
		if f.DisplayName() != "my-prd.md" {
			t.Errorf("PRD file DisplayName should be 'my-prd.md', got %q", f.DisplayName())
		}
	} else {
		t.Error("PRD file not found")
	}

	// Test Quick Build file
	if f, ok := fileMap["build-1.md"]; ok {
		if f.FeatureID != "" {
			t.Errorf("Quick Build file should have empty FeatureID, got %q", f.FeatureID)
		}
		if f.Category != "Quick Builds" {
			t.Errorf("Quick Build file should have Category 'Quick Builds', got %q", f.Category)
		}
		if f.DisplayName() != "build-1.md" {
			t.Errorf("Quick Build file DisplayName should be 'build-1.md', got %q", f.DisplayName())
		}
	} else {
		t.Error("Quick Build file not found")
	}

	// Test Charter file
	if f, ok := fileMap["charter.md"]; ok {
		if f.FeatureID != "" {
			t.Errorf("Charter file should have empty FeatureID, got %q", f.FeatureID)
		}
		if f.Category != "Other" {
			t.Errorf("Charter file should have Category 'Other', got %q", f.Category)
		}
		if f.DisplayName() != "charter.md" {
			t.Errorf("Charter file DisplayName should be 'charter.md', got %q", f.DisplayName())
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
			Name:      "requirements.md",
			FullPath:  ".rp1/work/features/test-feature/requirements.md",
			FeatureID: "test-feature",
		},
		{
			Name:     "index.md",
			FullPath: ".rp1/context/index.md",
			Category: "Context",
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
