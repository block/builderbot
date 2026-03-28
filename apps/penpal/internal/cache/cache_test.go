package cache

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"github.com/loganj/penpal/internal/discovery"
)

// E-PENPAL-SCAN: verifies scan classifies files by source type.
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

// E-PENPAL-SCAN: verifies deduplication when multiple sources overlap the same files.
func TestScanProjectSources_DedupsOverlappingSources(t *testing.T) {
	tmpDir := t.TempDir()
	projectPath := tmpDir

	// Create thoughts/ structure
	thoughtsPath := filepath.Join(projectPath, "thoughts")
	os.MkdirAll(filepath.Join(thoughtsPath, "plans"), 0755)
	os.WriteFile(filepath.Join(thoughtsPath, "plans", "foo.md"), []byte("# Plan"), 0644)
	os.WriteFile(filepath.Join(thoughtsPath, "notes.md"), []byte("# Notes"), 0644)

	// Also create a non-thoughts markdown file at the project root
	os.WriteFile(filepath.Join(projectPath, "README.md"), []byte("# README"), 0644)

	// Project with auto-detected thoughts/ source AND a manual "." tree source
	project := &discovery.Project{
		Name: "test-project",
		Path: projectPath,
		Sources: []discovery.FileSource{
			{
				Name:           "thoughts",
				Type:           "tree",
				SourceTypeName: "thoughts",
				RootPath:       thoughtsPath,
				Auto:           true,
			},
			{
				Name:           ".",
				Type:           "tree",
				SourceTypeName: "manual",
				RootPath:       projectPath,
				Auto:           false,
			},
		},
	}

	files := scanProjectSources(project)

	// Should have exactly 3 unique files (no duplicates from overlapping sources)
	if len(files) != 3 {
		t.Fatalf("Expected 3 files, got %d", len(files))
	}

	// Count occurrences of each FullPath
	pathCounts := make(map[string]int)
	pathSource := make(map[string]string)
	for _, f := range files {
		pathCounts[f.FullPath]++
		if _, ok := pathSource[f.FullPath]; !ok {
			pathSource[f.FullPath] = f.Source
		}
	}

	// No file should appear more than once
	for path, count := range pathCounts {
		if count > 1 {
			t.Errorf("File %q appears %d times, want 1", path, count)
		}
	}

	// Both thoughts files should be owned by "thoughts" source, not "."
	if src := pathSource["thoughts/plans/foo.md"]; src != "thoughts" {
		t.Errorf("thoughts/plans/foo.md should be source %q, got %q", "thoughts", src)
	}
	if src := pathSource["thoughts/notes.md"]; src != "thoughts" {
		t.Errorf("thoughts/notes.md should be source %q, got %q", "thoughts", src)
	}

	// README.md should still appear (owned by "." source)
	if _, ok := pathCounts["README.md"]; !ok {
		t.Error("README.md should be present from the '.' source")
	}
}

// E-PENPAL-TITLE-EXTRACT: verifies H1 heading extraction from markdown files.
func TestExtractTitle(t *testing.T) {
	tests := []struct {
		name    string
		content string
		want    string
	}{
		{"h1 on first line", "# My Plan Title\n\nSome content", "My Plan Title"},
		{"h1 after blank lines", "\n\n# Plan After Blanks\n\nContent", "Plan After Blanks"},
		{"no h1", "Some content\nMore content\n## H2 heading", ""},
		{"h2 not extracted", "## Not an H1\n\nContent", ""},
		{"empty file", "", ""},
		{"h1 with extra spaces", "#   Spaced Title  \n", "Spaced Title"},
		{"only hash no space", "#NoSpace\n", ""},
		{"h1 deep in file", "line1\nline2\nline3\nline4\nline5\n# Deep Title\n", "Deep Title"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tmpFile := filepath.Join(t.TempDir(), "test.md")
			os.WriteFile(tmpFile, []byte(tt.content), 0644)
			got := extractTitle(tmpFile)
			if got != tt.want {
				t.Errorf("extractTitle() = %q, want %q", got, tt.want)
			}
		})
	}
}

// E-PENPAL-TITLE-EXTRACT: verifies graceful handling of nonexistent files.
func TestExtractTitle_NonexistentFile(t *testing.T) {
	got := extractTitle("/nonexistent/path/file.md")
	if got != "" {
		t.Errorf("extractTitle() for nonexistent file = %q, want empty", got)
	}
}

// E-PENPAL-TITLE-EXTRACT: verifies titles are extracted during scan for all file types.
func TestScanProjectSources_ExtractsTitleForAllFiles(t *testing.T) {
	tmpDir := t.TempDir()

	thoughtsPath := filepath.Join(tmpDir, "thoughts")
	os.MkdirAll(filepath.Join(thoughtsPath, "plans"), 0755)
	os.MkdirAll(filepath.Join(thoughtsPath, "research"), 0755)

	os.WriteFile(filepath.Join(thoughtsPath, "plans", "my-plan.md"), []byte("# Per-Tab Navigation\n\nPlan content"), 0644)
	os.WriteFile(filepath.Join(thoughtsPath, "research", "analysis.md"), []byte("# Research Analysis\n\nResearch content"), 0644)

	project := &discovery.Project{
		Name: "test-project",
		Path: tmpDir,
		Sources: []discovery.FileSource{
			{
				Name:           "thoughts",
				Type:           "tree",
				SourceTypeName: "thoughts",
				RootPath:       thoughtsPath,
				Auto:           true,
			},
		},
		LastModified: time.Now(),
	}

	files := scanProjectSources(project)

	fileMap := make(map[string]FileInfo)
	for _, f := range files {
		fileMap[f.FullPath] = f
	}

	if f, ok := fileMap["thoughts/plans/my-plan.md"]; ok {
		if f.Title != "Per-Tab Navigation" {
			t.Errorf("plan file Title = %q, want %q", f.Title, "Per-Tab Navigation")
		}
	} else {
		t.Error("plan file not found")
	}

	if f, ok := fileMap["thoughts/research/analysis.md"]; ok {
		if f.Title != "Research Analysis" {
			t.Errorf("research file Title = %q, want %q", f.Title, "Research Analysis")
		}
	} else {
		t.Error("research file not found")
	}
}

// E-PENPAL-TITLE-EXTRACT: verifies EnrichTitles fills in missing titles from disk.
func TestCache_EnrichTitles(t *testing.T) {
	tmpDir := t.TempDir()

	planFile := filepath.Join(tmpDir, "my-plan.md")
	os.WriteFile(planFile, []byte("# My Great Plan\n\nContent here"), 0644)

	c := New()
	projectName := "test/project"

	files := []FileInfo{
		{
			Name:        "my-plan.md",
			FullPath:    "my-plan.md",
			ProjectPath: tmpDir,
			FileType:    "plan",
			Title:       "",
		},
		{
			Name:        "notes.md",
			FullPath:    "notes.md",
			ProjectPath: tmpDir,
			FileType:    "other",
			Title:       "",
		},
	}
	c.SetProjectFiles(projectName, files)

	c.EnrichTitles(projectName)

	enriched := c.ProjectFiles(projectName)
	if enriched[0].Title != "My Great Plan" {
		t.Errorf("plan file Title = %q, want %q", enriched[0].Title, "My Great Plan")
	}
	// notes.md doesn't exist on disk, so title stays empty
	if enriched[1].Title != "" {
		t.Errorf("missing file Title = %q, want empty", enriched[1].Title)
	}
}

// E-PENPAL-SCAN: verifies nested git worktree directories are skipped during scan.
func TestScanProjectSources_SkipsNestedWorktrees(t *testing.T) {
	tmpDir := t.TempDir()

	// Create a "." tree source rooted at the project root
	os.WriteFile(filepath.Join(tmpDir, "README.md"), []byte("# README"), 0644)
	os.MkdirAll(filepath.Join(tmpDir, "thoughts"), 0755)
	os.WriteFile(filepath.Join(tmpDir, "thoughts", "plan.md"), []byte("# Plan"), 0644)

	// Create a nested worktree at .claude/worktrees/my-branch.
	// Real git worktrees have a .git FILE (not directory) containing "gitdir: ...".
	wtDir := filepath.Join(tmpDir, ".claude", "worktrees", "my-branch")
	os.MkdirAll(wtDir, 0755)
	os.WriteFile(filepath.Join(wtDir, ".git"), []byte("gitdir: /fake/path/.git/worktrees/my-branch\n"), 0644)
	os.MkdirAll(filepath.Join(wtDir, "thoughts"), 0755)
	os.WriteFile(filepath.Join(wtDir, "thoughts", "plan.md"), []byte("# Worktree Plan"), 0644)
	os.WriteFile(filepath.Join(wtDir, "README.md"), []byte("# Worktree README"), 0644)

	project := &discovery.Project{
		Name: "test-project",
		Path: tmpDir,
		Sources: []discovery.FileSource{
			{
				Name:           "all",
				Type:           "tree",
				SourceTypeName: "manual",
				RootPath:       tmpDir,
				Auto:           false,
			},
		},
	}

	files := scanProjectSources(project)

	// Should only see the main project's files, not the worktree's duplicates
	names := map[string]bool{}
	for _, f := range files {
		names[f.FullPath] = true
	}

	if !names["README.md"] {
		t.Error("expected README.md from project root")
	}
	if !names["thoughts/plan.md"] {
		t.Error("expected thoughts/plan.md from project root")
	}
	// Files inside the nested worktree should be skipped
	if names[".claude/worktrees/my-branch/README.md"] {
		t.Error("worktree README.md should not appear in scan")
	}
	if names[".claude/worktrees/my-branch/thoughts/plan.md"] {
		t.Error("worktree thoughts/plan.md should not appear in scan")
	}
	if len(files) != 2 {
		t.Errorf("expected 2 files, got %d", len(files))
		for _, f := range files {
			t.Logf("  %s", f.FullPath)
		}
	}
}

// E-PENPAL-SCAN, P-PENPAL-SRC-GITIGNORE: verifies gitignored directories are skipped.
func TestScanProjectSources_SkipsGitignored(t *testing.T) {
	tmpDir := t.TempDir()

	// Initialise a git repo so git check-ignore works.
	runGit(t, tmpDir, "init")
	runGit(t, tmpDir, "config", "user.email", "test@test.com")
	runGit(t, tmpDir, "config", "user.name", "test")

	// Create .gitignore that ignores "build/" and "vendor/".
	os.WriteFile(filepath.Join(tmpDir, ".gitignore"), []byte("build/\nvendor/\n"), 0644)

	// Create visible files.
	os.MkdirAll(filepath.Join(tmpDir, "docs"), 0755)
	os.WriteFile(filepath.Join(tmpDir, "docs", "readme.md"), []byte("# Readme"), 0644)
	os.WriteFile(filepath.Join(tmpDir, "notes.md"), []byte("# Notes"), 0644)

	// Create files inside gitignored directories.
	os.MkdirAll(filepath.Join(tmpDir, "build", "out"), 0755)
	os.WriteFile(filepath.Join(tmpDir, "build", "out", "generated.md"), []byte("# Gen"), 0644)
	os.MkdirAll(filepath.Join(tmpDir, "vendor", "lib"), 0755)
	os.WriteFile(filepath.Join(tmpDir, "vendor", "lib", "dep.md"), []byte("# Dep"), 0644)

	project := &discovery.Project{
		Name: "test",
		Path: tmpDir,
		Sources: []discovery.FileSource{
			{
				Name:     "all",
				Type:     "tree",
				RootPath: tmpDir,
			},
		},
	}

	files := scanProjectSources(project)

	paths := map[string]bool{}
	for _, f := range files {
		paths[f.FullPath] = true
	}

	if !paths["docs/readme.md"] {
		t.Error("expected docs/readme.md (not gitignored)")
	}
	if !paths["notes.md"] {
		t.Error("expected notes.md (not gitignored)")
	}
	if paths["build/out/generated.md"] {
		t.Error("build/out/generated.md should be skipped (gitignored)")
	}
	if paths["vendor/lib/dep.md"] {
		t.Error("vendor/lib/dep.md should be skipped (gitignored)")
	}
}

// E-PENPAL-SCAN, P-PENPAL-SRC-GITIGNORE: registered source root overrides gitignore.
func TestScanProjectSources_GitignoreDoesNotSkipSourceRoot(t *testing.T) {
	tmpDir := t.TempDir()

	runGit(t, tmpDir, "init")
	runGit(t, tmpDir, "config", "user.email", "test@test.com")
	runGit(t, tmpDir, "config", "user.name", "test")

	// Gitignore the "build/" directory.
	os.WriteFile(filepath.Join(tmpDir, ".gitignore"), []byte("build/\n"), 0644)

	// But we have a registered source explicitly pointing into build/docs.
	os.MkdirAll(filepath.Join(tmpDir, "build", "docs"), 0755)
	os.WriteFile(filepath.Join(tmpDir, "build", "docs", "api.md"), []byte("# API"), 0644)

	project := &discovery.Project{
		Name: "test",
		Path: tmpDir,
		Sources: []discovery.FileSource{
			{
				Name:     "build-docs",
				Type:     "tree",
				RootPath: filepath.Join(tmpDir, "build", "docs"),
			},
		},
	}

	files := scanProjectSources(project)

	if len(files) != 1 {
		t.Fatalf("expected 1 file, got %d", len(files))
	}
	if files[0].FullPath != "build/docs/api.md" {
		t.Errorf("expected build/docs/api.md, got %s", files[0].FullPath)
	}
}

// E-PENPAL-SCAN: non-git directories work without errors.
func TestGitIgnoreChecker_NonGitDir(t *testing.T) {
	tmpDir := t.TempDir()
	checker := newGitIgnoreChecker(tmpDir)
	if checker.isGitRepo {
		t.Fatal("expected non-git dir to be detected")
	}
	if checker.IsIgnored(filepath.Join(tmpDir, "anything")) {
		t.Error("non-git dir should never report paths as ignored")
	}
}

func runGit(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v failed: %v\n%s", args, err, out)
	}
}

// E-PENPAL-CACHE: verifies FindFile returns files by project and path.
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
