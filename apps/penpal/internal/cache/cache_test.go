package cache

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"github.com/loganj/penpal/internal/discovery"
)

// E-PENPAL-CACHE: verifies AddProject appends a new project.
func TestAddProject_AppendsNewProject(t *testing.T) {
	c := New()
	c.SetProjects([]discovery.Project{
		{Name: "existing", Path: "/a", Origin: "standalone"},
	})

	c.AddProject(discovery.Project{Name: "new-proj", Path: "/b", Origin: "standalone"})

	projects := c.Projects()
	if len(projects) != 2 {
		t.Fatalf("expected 2 projects, got %d", len(projects))
	}
	if projects[1].Name != "new-proj" {
		t.Errorf("expected new-proj, got %s", projects[1].Name)
	}
}

// E-PENPAL-CACHE: verifies AddProject replaces by qualified name (standalone).
func TestAddProject_ReplacesByQualifiedName(t *testing.T) {
	c := New()
	c.SetProjects([]discovery.Project{
		{Name: "proj", Path: "/old", Origin: "standalone"},
	})

	c.AddProject(discovery.Project{Name: "proj", Path: "/new", Origin: "standalone"})

	projects := c.Projects()
	if len(projects) != 1 {
		t.Fatalf("expected 1 project (replaced), got %d", len(projects))
	}
	if projects[0].Path != "/new" {
		t.Errorf("expected path /new, got %s", projects[0].Path)
	}
}

// E-PENPAL-CACHE: verifies AddProject replaces workspace-scoped projects.
func TestAddProject_ReplacesWorkspaceProject(t *testing.T) {
	c := New()
	c.SetProjects([]discovery.Project{
		{Name: "proj", WorkspaceName: "ws", Path: "/old", Origin: "workspace"},
	})

	// Same qualified name "ws/proj"
	c.AddProject(discovery.Project{Name: "proj", WorkspaceName: "ws", Path: "/new", Origin: "workspace"})

	projects := c.Projects()
	if len(projects) != 1 {
		t.Fatalf("expected 1 project (replaced), got %d", len(projects))
	}
	if projects[0].Path != "/new" {
		t.Errorf("expected path /new, got %s", projects[0].Path)
	}
}

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

// E-PENPAL-SCAN: verifies CheckAllProjectsHasFiles sets HasFiles correctly.
func TestCheckAllProjectsHasFiles(t *testing.T) {
	tmpDir := t.TempDir()

	// Project with markdown files
	withMD := filepath.Join(tmpDir, "with-md")
	os.MkdirAll(filepath.Join(withMD, "docs"), 0755)
	os.WriteFile(filepath.Join(withMD, "docs", "readme.md"), []byte("# Hello"), 0644)

	// Project with no markdown files
	withoutMD := filepath.Join(tmpDir, "without-md")
	os.MkdirAll(withoutMD, 0755)
	os.WriteFile(filepath.Join(withoutMD, "main.go"), []byte("package main"), 0644)

	c := New()
	c.SetProjects([]discovery.Project{
		{Name: "has-files", Path: withMD},
		{Name: "no-files", Path: withoutMD},
	})

	c.CheckAllProjectsHasFiles()

	projects := c.Projects()
	for _, p := range projects {
		switch p.Name {
		case "has-files":
			if !p.HasFiles {
				t.Error("project with .md files should have HasFiles=true")
			}
		case "no-files":
			if p.HasFiles {
				t.Error("project without .md files should have HasFiles=false")
			}
		}
	}
}

// E-PENPAL-SCAN: projectHasAnyMarkdown does NOT check gitignore — it's a
// lightweight startup check where false positives are harmless. Verifying that
// .md files in gitignored dirs still count as "has markdown".
func TestProjectHasAnyMarkdown_IgnoresGitignore(t *testing.T) {
	tmpDir := t.TempDir()

	runGit(t, tmpDir, "init")
	runGit(t, tmpDir, "config", "user.email", "test@test.com")
	runGit(t, tmpDir, "config", "user.name", "test")

	// .md files only in a gitignored directory — still returns true
	os.WriteFile(filepath.Join(tmpDir, ".gitignore"), []byte("build/\n"), 0644)
	os.MkdirAll(filepath.Join(tmpDir, "build"), 0755)
	os.WriteFile(filepath.Join(tmpDir, "build", "output.md"), []byte("# Gen"), 0644)

	if !projectHasAnyMarkdown(tmpDir) {
		t.Error("expected true: .md exists even though gitignored (gitignore not checked)")
	}
}

// E-PENPAL-SCAN: verifies projectHasAnyMarkdown skips .hg and .svn dirs.
func TestProjectHasAnyMarkdown_SkipsVCSDirs(t *testing.T) {
	tmpDir := t.TempDir()

	// .md files only in .hg and .svn dirs
	os.MkdirAll(filepath.Join(tmpDir, ".hg"), 0755)
	os.WriteFile(filepath.Join(tmpDir, ".hg", "notes.md"), []byte("# HG"), 0644)
	os.MkdirAll(filepath.Join(tmpDir, ".svn"), 0755)
	os.WriteFile(filepath.Join(tmpDir, ".svn", "notes.md"), []byte("# SVN"), 0644)

	if projectHasAnyMarkdown(tmpDir) {
		t.Error("expected false: only .md files are in .hg/.svn dirs")
	}
}

// E-PENPAL-SRC-ALL-MD: verifies AllFiles deduplicates __all_markdown__ entries.
func TestAllFiles_DeduplicatesAllMarkdown(t *testing.T) {
	c := New()
	now := time.Now()

	c.SetProjectFiles("proj", []FileInfo{
		{Project: "proj", Source: "thoughts", FullPath: "thoughts/plan.md", Name: "plan.md", ModTime: now},
		{Project: "proj", Source: "__all_markdown__", FullPath: "thoughts/plan.md", Name: "plan.md", ModTime: now},
		{Project: "proj", Source: "__all_markdown__", FullPath: "README.md", Name: "README.md", ModTime: now},
	})

	files := c.AllFiles(0)
	if len(files) != 2 {
		t.Fatalf("expected 2 unique files, got %d", len(files))
	}

	// The thoughts/plan.md entry should prefer the typed source
	for _, f := range files {
		if f.FullPath == "thoughts/plan.md" && f.Source != "thoughts" {
			t.Errorf("expected source 'thoughts' for thoughts/plan.md, got %q", f.Source)
		}
		// README.md only exists in __all_markdown__, so that's fine
		if f.FullPath == "README.md" && f.Source != "__all_markdown__" {
			t.Errorf("expected source '__all_markdown__' for README.md, got %q", f.Source)
		}
	}
}

// E-PENPAL-SCAN: verifies EnsureProjectScanned prevents concurrent duplicate scans.
func TestEnsureProjectScanned_NoDuplicateScans(t *testing.T) {
	tmpDir := t.TempDir()
	os.WriteFile(filepath.Join(tmpDir, "notes.md"), []byte("# Notes"), 0644)

	c := New()
	c.SetProjects([]discovery.Project{
		{
			Name: "test",
			Path: tmpDir,
			Sources: []discovery.FileSource{
				{Name: "__all_markdown__", Type: "tree", SourceTypeName: "__all_markdown__", RootPath: tmpDir, Auto: true},
			},
		},
	})

	// First call should scan
	if !c.EnsureProjectScanned("test") {
		t.Error("first call should return true (scan performed)")
	}

	// Second call should be a no-op
	if c.EnsureProjectScanned("test") {
		t.Error("second call should return false (already scanned)")
	}

	// Files should be populated from the first scan
	files := c.ProjectFiles("test")
	if len(files) == 0 {
		t.Error("expected files after scan")
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

// E-PENPAL-SCAN: verifies single-file source resolution matches scan behavior.
func TestResolveFileInfo_ThoughtsSource(t *testing.T) {
	tmpDir := t.TempDir()
	thoughtsDir := filepath.Join(tmpDir, "thoughts")
	os.MkdirAll(filepath.Join(thoughtsDir, "research"), 0755)

	filePath := filepath.Join(thoughtsDir, "research", "topic.md")
	os.WriteFile(filePath, []byte("# My Research"), 0644)

	project := &discovery.Project{
		Name: "test",
		Path: tmpDir,
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: thoughtsDir, Auto: true},
			{Name: "__all_markdown__", Type: "tree", SourceTypeName: "__all_markdown__", RootPath: tmpDir, Auto: true},
		},
	}

	results := ResolveFileInfo(project, filePath)
	if len(results) != 2 {
		t.Fatalf("expected 2 results (thoughts + __all_markdown__), got %d", len(results))
	}

	// First result should be the typed source
	if results[0].Source != "thoughts" {
		t.Errorf("expected first source 'thoughts', got %q", results[0].Source)
	}
	if results[0].FileType != "research" {
		t.Errorf("expected fileType 'research', got %q", results[0].FileType)
	}
	if results[0].Title != "My Research" {
		t.Errorf("expected title 'My Research', got %q", results[0].Title)
	}
	if results[0].FullPath != "thoughts/research/topic.md" {
		t.Errorf("expected fullPath 'thoughts/research/topic.md', got %q", results[0].FullPath)
	}

	// Second result should be __all_markdown__
	if results[1].Source != "__all_markdown__" {
		t.Errorf("expected second source '__all_markdown__', got %q", results[1].Source)
	}
}

// E-PENPAL-SCAN: ResolveFileInfo respects SkipDirs.
func TestResolveFileInfo_SkipDirs(t *testing.T) {
	tmpDir := t.TempDir()
	os.MkdirAll(filepath.Join(tmpDir, "node_modules", "pkg"), 0755)
	filePath := filepath.Join(tmpDir, "node_modules", "pkg", "readme.md")
	os.WriteFile(filePath, []byte("# Dep"), 0644)

	project := &discovery.Project{
		Name: "test",
		Path: tmpDir,
		Sources: []discovery.FileSource{
			{Name: "__all_markdown__", Type: "tree", SourceTypeName: "__all_markdown__", RootPath: tmpDir, Auto: true},
		},
	}

	results := ResolveFileInfo(project, filePath)
	// __all_markdown__ has SkipDirs for node_modules
	if len(results) != 0 {
		t.Fatalf("expected 0 results (node_modules is in SkipDirs), got %d", len(results))
	}
}

// E-PENPAL-SCAN: ResolveFileInfo respects RequireSibling.
func TestResolveFileInfo_RequireSibling(t *testing.T) {
	tmpDir := t.TempDir()

	// Directory WITH ANCHORS.md sibling
	withSibling := filepath.Join(tmpDir, "module-a")
	os.MkdirAll(withSibling, 0755)
	os.WriteFile(filepath.Join(withSibling, "ANCHORS.md"), []byte("---\nprefix: A\n---\n"), 0644)
	os.WriteFile(filepath.Join(withSibling, "PRODUCT.md"), []byte("# Product"), 0644)

	// Directory WITHOUT ANCHORS.md sibling
	withoutSibling := filepath.Join(tmpDir, "module-b")
	os.MkdirAll(withoutSibling, 0755)
	os.WriteFile(filepath.Join(withoutSibling, "PRODUCT.md"), []byte("# Orphan"), 0644)

	project := &discovery.Project{
		Name: "test",
		Path: tmpDir,
		Sources: []discovery.FileSource{
			{Name: "anchors", Type: "tree", SourceTypeName: "anchors", RootPath: tmpDir, Auto: true},
		},
	}

	// File with sibling should be included
	results := ResolveFileInfo(project, filepath.Join(withSibling, "PRODUCT.md"))
	if len(results) != 1 {
		t.Fatalf("expected 1 result for file with ANCHORS.md sibling, got %d", len(results))
	}

	// File without sibling should be excluded
	results = ResolveFileInfo(project, filepath.Join(withoutSibling, "PRODUCT.md"))
	if len(results) != 0 {
		t.Fatalf("expected 0 results for file without ANCHORS.md sibling, got %d", len(results))
	}
}

// E-PENPAL-SCAN: ResolveFileInfo returns nil for non-.md files.
func TestResolveFileInfo_NonMdFile(t *testing.T) {
	tmpDir := t.TempDir()
	filePath := filepath.Join(tmpDir, "readme.txt")
	os.WriteFile(filePath, []byte("hello"), 0644)

	project := &discovery.Project{
		Name: "test",
		Path: tmpDir,
		Sources: []discovery.FileSource{
			{Name: "__all_markdown__", Type: "tree", SourceTypeName: "__all_markdown__", RootPath: tmpDir, Auto: true},
		},
	}

	results := ResolveFileInfo(project, filePath)
	if len(results) != 0 {
		t.Fatalf("expected 0 results for non-.md file, got %d", len(results))
	}
}

// E-PENPAL-SCAN: ResolveFileInfo dedup — first typed source wins.
func TestResolveFileInfo_SourcePriority(t *testing.T) {
	tmpDir := t.TempDir()
	thoughtsDir := filepath.Join(tmpDir, "thoughts")
	os.MkdirAll(thoughtsDir, 0755)
	filePath := filepath.Join(thoughtsDir, "plan.md")
	os.WriteFile(filePath, []byte("# Plan"), 0644)

	project := &discovery.Project{
		Name: "test",
		Path: tmpDir,
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: thoughtsDir, Auto: true},
			// A second typed source covering the same path
			{Name: "manual", Type: "tree", SourceTypeName: "manual", RootPath: thoughtsDir},
			{Name: "__all_markdown__", Type: "tree", SourceTypeName: "__all_markdown__", RootPath: tmpDir, Auto: true},
		},
	}

	results := ResolveFileInfo(project, filePath)
	// Should get 2: first typed source (thoughts) + __all_markdown__
	// The second typed source (manual) should be skipped
	if len(results) != 2 {
		t.Fatalf("expected 2 results, got %d", len(results))
	}
	if results[0].Source != "thoughts" {
		t.Errorf("expected first source 'thoughts', got %q", results[0].Source)
	}
	if results[1].Source != "__all_markdown__" {
		t.Errorf("expected second source '__all_markdown__', got %q", results[1].Source)
	}
}

// E-PENPAL-CACHE: verifies UpsertFile updates existing entries.
func TestUpsertFile_ExistingFile(t *testing.T) {
	tmpDir := t.TempDir()
	filePath := filepath.Join(tmpDir, "thoughts", "plan.md")
	os.MkdirAll(filepath.Join(tmpDir, "thoughts"), 0755)
	os.WriteFile(filePath, []byte("# Old Title"), 0644)

	c := New()
	projectName := "test"
	c.SetProjects([]discovery.Project{
		{Name: "test", Path: tmpDir, Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: filepath.Join(tmpDir, "thoughts"), Auto: true},
		}},
	})

	// Pre-populate cache with an entry
	c.SetProjectFiles(projectName, []FileInfo{
		{Project: "test", Source: "thoughts", FullPath: "thoughts/plan.md", Name: "plan.md", Title: "Old Title", ModTime: time.Now().Add(-1 * time.Hour)},
	})

	// Update the file on disk
	os.WriteFile(filePath, []byte("# New Title"), 0644)

	project := c.FindProject(projectName)
	ok := c.UpsertFile(projectName, project, filePath)
	if !ok {
		t.Fatal("UpsertFile returned false, expected true")
	}

	files := c.ProjectFiles(projectName)
	if len(files) != 1 {
		t.Fatalf("expected 1 file, got %d", len(files))
	}
	if files[0].Title != "New Title" {
		t.Errorf("expected title 'New Title', got %q", files[0].Title)
	}
}

// E-PENPAL-CACHE: verifies UpsertFile adds new files via source resolution.
func TestUpsertFile_NewFile(t *testing.T) {
	tmpDir := t.TempDir()
	thoughtsDir := filepath.Join(tmpDir, "thoughts")
	os.MkdirAll(thoughtsDir, 0755)
	filePath := filepath.Join(thoughtsDir, "new-note.md")
	os.WriteFile(filePath, []byte("# Fresh Note"), 0644)

	c := New()
	projectName := "test"
	project := discovery.Project{
		Name: "test", Path: tmpDir,
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: thoughtsDir, Auto: true},
			{Name: "__all_markdown__", Type: "tree", SourceTypeName: "__all_markdown__", RootPath: tmpDir, Auto: true},
		},
	}
	c.SetProjects([]discovery.Project{project})
	c.SetProjectFiles(projectName, nil) // empty cache

	ok := c.UpsertFile(projectName, &project, filePath)
	if !ok {
		t.Fatal("UpsertFile returned false, expected true")
	}

	files := c.ProjectFiles(projectName)
	if len(files) != 2 {
		t.Fatalf("expected 2 files (thoughts + __all_markdown__), got %d", len(files))
	}

	// Check the typed source entry
	found := false
	for _, f := range files {
		if f.Source == "thoughts" && f.FullPath == "thoughts/new-note.md" {
			found = true
			if f.Title != "Fresh Note" {
				t.Errorf("expected title 'Fresh Note', got %q", f.Title)
			}
		}
	}
	if !found {
		t.Error("expected thoughts source entry for new file")
	}
}

// E-PENPAL-CACHE: verifies UpsertFile returns false for excluded files.
func TestUpsertFile_ExcludedFile(t *testing.T) {
	tmpDir := t.TempDir()
	os.MkdirAll(filepath.Join(tmpDir, "node_modules"), 0755)
	filePath := filepath.Join(tmpDir, "node_modules", "readme.md")
	os.WriteFile(filePath, []byte("# Dep"), 0644)

	c := New()
	projectName := "test"
	project := discovery.Project{
		Name: "test", Path: tmpDir,
		Sources: []discovery.FileSource{
			{Name: "__all_markdown__", Type: "tree", SourceTypeName: "__all_markdown__", RootPath: tmpDir, Auto: true},
		},
	}
	c.SetProjects([]discovery.Project{project})
	c.SetProjectFiles(projectName, nil)

	ok := c.UpsertFile(projectName, &project, filePath)
	if ok {
		t.Error("UpsertFile should return false for file in SkipDirs")
	}

	files := c.ProjectFiles(projectName)
	if len(files) != 0 {
		t.Fatalf("expected 0 files, got %d", len(files))
	}
}

// E-PENPAL-CACHE: verifies RemoveFile removes entries and updates metadata.
func TestRemoveFile(t *testing.T) {
	now := time.Now()
	older := now.Add(-1 * time.Hour)

	c := New()
	projectName := "test"
	c.SetProjects([]discovery.Project{
		{Name: "test", Path: "/tmp/test"},
	})
	c.SetProjectFiles(projectName, []FileInfo{
		{Project: "test", Source: "thoughts", FullPath: "thoughts/plan.md", Name: "plan.md", ModTime: now},
		{Project: "test", Source: "thoughts", FullPath: "thoughts/old.md", Name: "old.md", ModTime: older},
	})

	ok := c.RemoveFile(projectName, "thoughts/plan.md")
	if !ok {
		t.Fatal("RemoveFile returned false, expected true")
	}

	files := c.ProjectFiles(projectName)
	if len(files) != 1 {
		t.Fatalf("expected 1 file, got %d", len(files))
	}
	if files[0].FullPath != "thoughts/old.md" {
		t.Errorf("expected remaining file 'thoughts/old.md', got %q", files[0].FullPath)
	}

	// Verify metadata updated
	project := c.FindProject(projectName)
	if !project.HasFiles {
		t.Error("project should still have files")
	}
	if !project.LastModified.Equal(older) {
		t.Errorf("expected LastModified to be older time, got %v", project.LastModified)
	}
}

// E-PENPAL-CACHE: verifies RemoveFile returns false for non-existent entries.
func TestRemoveFile_NotFound(t *testing.T) {
	c := New()
	c.SetProjectFiles("test", []FileInfo{
		{Project: "test", FullPath: "readme.md"},
	})

	ok := c.RemoveFile("test", "nonexistent.md")
	if ok {
		t.Error("RemoveFile should return false for non-existent file")
	}
}

// E-PENPAL-CACHE: verifies RemoveFile clears HasFiles when last file removed.
func TestRemoveFile_ClearsHasFiles(t *testing.T) {
	c := New()
	c.SetProjects([]discovery.Project{
		{Name: "test", Path: "/tmp/test", HasFiles: true},
	})
	c.SetProjectFiles("test", []FileInfo{
		{Project: "test", FullPath: "only.md", ModTime: time.Now()},
	})

	c.RemoveFile("test", "only.md")

	project := c.FindProject("test")
	if project.HasFiles {
		t.Error("project should have HasFiles=false after removing last file")
	}
}

// E-PENPAL-CACHE: verifies SourcesChanged detects material differences.
func TestSourcesChanged(t *testing.T) {
	base := []discovery.FileSource{
		{Name: "thoughts", Type: "tree", RootPath: "/a/thoughts", SourceTypeName: "thoughts"},
		{Name: "__all_markdown__", Type: "tree", RootPath: "/a", SourceTypeName: "__all_markdown__"},
	}

	// Identical
	same := []discovery.FileSource{
		{Name: "thoughts", Type: "tree", RootPath: "/a/thoughts", SourceTypeName: "thoughts"},
		{Name: "__all_markdown__", Type: "tree", RootPath: "/a", SourceTypeName: "__all_markdown__"},
	}
	if SourcesChanged(base, same) {
		t.Error("identical sources should not be reported as changed")
	}

	// Different count
	fewer := base[:1]
	if !SourcesChanged(base, fewer) {
		t.Error("different count should be reported as changed")
	}

	// Different root path
	moved := []discovery.FileSource{
		{Name: "thoughts", Type: "tree", RootPath: "/b/thoughts", SourceTypeName: "thoughts"},
		{Name: "__all_markdown__", Type: "tree", RootPath: "/a", SourceTypeName: "__all_markdown__"},
	}
	if !SourcesChanged(base, moved) {
		t.Error("different RootPath should be reported as changed")
	}

	// Different name
	renamed := []discovery.FileSource{
		{Name: "rp1", Type: "tree", RootPath: "/a/thoughts", SourceTypeName: "rp1"},
		{Name: "__all_markdown__", Type: "tree", RootPath: "/a", SourceTypeName: "__all_markdown__"},
	}
	if !SourcesChanged(base, renamed) {
		t.Error("different Name should be reported as changed")
	}

	// Different files list
	withFiles := []discovery.FileSource{
		{Name: "manual", Type: "files", Files: []string{"/a/foo.md"}},
	}
	withDiffFiles := []discovery.FileSource{
		{Name: "manual", Type: "files", Files: []string{"/a/bar.md"}},
	}
	if !SourcesChanged(withFiles, withDiffFiles) {
		t.Error("different Files should be reported as changed")
	}
}

// E-PENPAL-CACHE: verifies RescanWith preserves cache for unchanged projects.
func TestRescanWith_PreservesUnchangedProjects(t *testing.T) {
	tmpDir := t.TempDir()

	// Set up two projects
	projADir := filepath.Join(tmpDir, "proj-a")
	projBDir := filepath.Join(tmpDir, "proj-b")
	os.MkdirAll(projADir, 0755)
	os.MkdirAll(projBDir, 0755)
	os.WriteFile(filepath.Join(projADir, "readme.md"), []byte("# A"), 0644)
	os.WriteFile(filepath.Join(projBDir, "readme.md"), []byte("# B"), 0644)

	sourcesA := []discovery.FileSource{
		{Name: "__all_markdown__", Type: "tree", SourceTypeName: "__all_markdown__", RootPath: projADir, Auto: true},
	}
	sourcesB := []discovery.FileSource{
		{Name: "__all_markdown__", Type: "tree", SourceTypeName: "__all_markdown__", RootPath: projBDir, Auto: true},
	}

	c := New()
	c.SetProjects([]discovery.Project{
		{Name: "proj-a", Path: projADir, Sources: sourcesA},
		{Name: "proj-b", Path: projBDir, Sources: sourcesB},
	})

	// Simulate initial scan for proj-a
	c.SetProjectFiles("proj-a", []FileInfo{
		{Project: "proj-a", Source: "__all_markdown__", FullPath: "readme.md", Name: "readme.md", Title: "A", ModTime: time.Now()},
	})

	// proj-b is not scanned yet

	// RescanWith the same projects (unchanged sources)
	c.RescanWith([]discovery.Project{
		{Name: "proj-a", Path: projADir, Sources: sourcesA},
		{Name: "proj-b", Path: projBDir, Sources: sourcesB},
	})

	// proj-a should still have its cached files (not re-walked)
	filesA := c.ProjectFiles("proj-a")
	if len(filesA) != 1 {
		t.Fatalf("expected proj-a to preserve 1 cached file, got %d", len(filesA))
	}
	if filesA[0].Title != "A" {
		t.Errorf("expected preserved title 'A', got %q", filesA[0].Title)
	}

	// proj-b was never scanned, so RescanWith should scan it now
	filesB := c.ProjectFiles("proj-b")
	if len(filesB) != 1 {
		t.Fatalf("expected proj-b to have 1 file after rescan, got %d", len(filesB))
	}
}

// E-PENPAL-CACHE: verifies RescanWith cleans up removed projects.
func TestRescanWith_RemovesOldProjects(t *testing.T) {
	keepSources := []discovery.FileSource{
		{Name: "__all_markdown__", Type: "tree", SourceTypeName: "__all_markdown__", RootPath: "/tmp/keep", Auto: true},
	}
	removeSources := []discovery.FileSource{
		{Name: "__all_markdown__", Type: "tree", SourceTypeName: "__all_markdown__", RootPath: "/tmp/remove", Auto: true},
	}

	c := New()
	c.SetProjects([]discovery.Project{
		{Name: "keep", Path: "/tmp/keep", Sources: keepSources},
		{Name: "remove", Path: "/tmp/remove", Sources: removeSources},
	})
	c.SetProjectFiles("keep", []FileInfo{
		{Project: "keep", FullPath: "readme.md"},
	})
	c.SetProjectFiles("remove", []FileInfo{
		{Project: "remove", FullPath: "readme.md"},
	})

	// RescanWith only the "keep" project (same sources → preserved)
	c.RescanWith([]discovery.Project{
		{Name: "keep", Path: "/tmp/keep", Sources: keepSources},
	})

	// "remove" should be gone from cache
	filesRemoved := c.ProjectFiles("remove")
	if len(filesRemoved) != 0 {
		t.Errorf("expected removed project to have 0 cached files, got %d", len(filesRemoved))
	}

	// "keep" should still have its files (unchanged sources → preserved)
	filesKeep := c.ProjectFiles("keep")
	if len(filesKeep) != 1 {
		t.Errorf("expected kept project to preserve 1 cached file, got %d", len(filesKeep))
	}
}
