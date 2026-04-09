package gitignore

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

func runGit(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		t.Fatalf("git %v failed: %v", args, err)
	}
}

func TestNew_NonGitDir(t *testing.T) {
	m := New(t.TempDir())
	if m != nil {
		t.Error("expected nil matcher for non-git directory")
	}
}

func TestNew_GitRepo(t *testing.T) {
	dir := t.TempDir()
	runGit(t, dir, "init")
	m := New(dir)
	if m == nil {
		t.Fatal("expected non-nil matcher for git repo")
	}
	if m.repoRoot != dir {
		t.Errorf("repoRoot = %s, want %s", m.repoRoot, dir)
	}
}

func TestIsIgnoredDir_NilMatcher(t *testing.T) {
	var m *Matcher
	if m.IsIgnoredDir("/anything") {
		t.Error("nil matcher should never report ignored")
	}
}

func TestIsIgnoredDir_BasicPatterns(t *testing.T) {
	dir := t.TempDir()
	runGit(t, dir, "init")

	os.WriteFile(filepath.Join(dir, ".gitignore"), []byte("build/\nvendor/\n"), 0644)
	os.MkdirAll(filepath.Join(dir, "build", "out"), 0755)
	os.MkdirAll(filepath.Join(dir, "vendor", "lib"), 0755)
	os.MkdirAll(filepath.Join(dir, "docs"), 0755)

	m := New(dir)
	if m == nil {
		t.Fatal("expected matcher")
	}

	tests := []struct {
		path    string
		ignored bool
	}{
		{filepath.Join(dir, "build"), true},
		{filepath.Join(dir, "build", "out"), true},
		{filepath.Join(dir, "vendor"), true},
		{filepath.Join(dir, "vendor", "lib"), true},
		{filepath.Join(dir, "docs"), false},
	}

	for _, tt := range tests {
		got := m.IsIgnoredDir(tt.path)
		if got != tt.ignored {
			t.Errorf("IsIgnoredDir(%s) = %v, want %v", tt.path, got, tt.ignored)
		}
	}
}

func TestIsIgnoredDir_WildcardPatterns(t *testing.T) {
	dir := t.TempDir()
	runGit(t, dir, "init")

	os.WriteFile(filepath.Join(dir, ".gitignore"), []byte("*.tmp\n__pycache__\n"), 0644)
	os.MkdirAll(filepath.Join(dir, "foo.tmp"), 0755)
	os.MkdirAll(filepath.Join(dir, "src", "__pycache__"), 0755)
	os.MkdirAll(filepath.Join(dir, "src", "main"), 0755)

	m := New(dir)
	if m == nil {
		t.Fatal("expected matcher")
	}

	tests := []struct {
		path    string
		ignored bool
	}{
		{filepath.Join(dir, "foo.tmp"), true},
		{filepath.Join(dir, "src", "__pycache__"), true},
		{filepath.Join(dir, "src", "main"), false},
	}

	for _, tt := range tests {
		got := m.IsIgnoredDir(tt.path)
		if got != tt.ignored {
			t.Errorf("IsIgnoredDir(%s) = %v, want %v", tt.path, got, tt.ignored)
		}
	}
}

func TestIsIgnoredDir_Negation(t *testing.T) {
	dir := t.TempDir()
	runGit(t, dir, "init")

	os.WriteFile(filepath.Join(dir, ".gitignore"), []byte("logs/\n!logs/important/\n"), 0644)
	os.MkdirAll(filepath.Join(dir, "logs", "important"), 0755)
	os.MkdirAll(filepath.Join(dir, "logs", "debug"), 0755)

	m := New(dir)
	if m == nil {
		t.Fatal("expected matcher")
	}

	// logs/ is ignored.
	if !m.IsIgnoredDir(filepath.Join(dir, "logs")) {
		t.Error("expected logs/ to be ignored")
	}

	// logs/important/ has a negation rule, but its parent (logs/) is ignored,
	// so it stays ignored (git behavior: cannot re-include under ignored parent).
	if !m.IsIgnoredDir(filepath.Join(dir, "logs", "important")) {
		t.Error("expected logs/important/ to be ignored (parent is ignored)")
	}

	// logs/debug/ is under ignored parent.
	if !m.IsIgnoredDir(filepath.Join(dir, "logs", "debug")) {
		t.Error("expected logs/debug/ to be ignored")
	}
}

func TestIsIgnoredDir_DoubleStarPattern(t *testing.T) {
	dir := t.TempDir()
	runGit(t, dir, "init")

	os.WriteFile(filepath.Join(dir, ".gitignore"), []byte("**/cache\n"), 0644)
	os.MkdirAll(filepath.Join(dir, "cache"), 0755)
	os.MkdirAll(filepath.Join(dir, "a", "b", "cache"), 0755)
	os.MkdirAll(filepath.Join(dir, "src"), 0755)

	m := New(dir)
	if m == nil {
		t.Fatal("expected matcher")
	}

	tests := []struct {
		path    string
		ignored bool
	}{
		{filepath.Join(dir, "cache"), true},
		{filepath.Join(dir, "a", "b", "cache"), true},
		{filepath.Join(dir, "src"), false},
	}

	for _, tt := range tests {
		got := m.IsIgnoredDir(tt.path)
		if got != tt.ignored {
			t.Errorf("IsIgnoredDir(%s) = %v, want %v", tt.path, got, tt.ignored)
		}
	}
}

func TestIsIgnoredDir_NestedGitignore(t *testing.T) {
	dir := t.TempDir()
	runGit(t, dir, "init")

	// Root .gitignore ignores "tmp/" everywhere.
	os.WriteFile(filepath.Join(dir, ".gitignore"), []byte("tmp/\n"), 0644)

	// Subdirectory .gitignore adds its own ignore.
	os.MkdirAll(filepath.Join(dir, "src"), 0755)
	os.WriteFile(filepath.Join(dir, "src", ".gitignore"), []byte("generated/\n"), 0644)

	os.MkdirAll(filepath.Join(dir, "src", "generated"), 0755)
	os.MkdirAll(filepath.Join(dir, "src", "main"), 0755)
	os.MkdirAll(filepath.Join(dir, "tmp"), 0755)
	os.MkdirAll(filepath.Join(dir, "src", "tmp"), 0755)

	m := New(dir)
	if m == nil {
		t.Fatal("expected matcher")
	}

	tests := []struct {
		path    string
		ignored bool
	}{
		{filepath.Join(dir, "tmp"), true},
		{filepath.Join(dir, "src", "tmp"), true},
		{filepath.Join(dir, "src", "generated"), true},
		{filepath.Join(dir, "src", "main"), false},
	}

	for _, tt := range tests {
		got := m.IsIgnoredDir(tt.path)
		if got != tt.ignored {
			t.Errorf("IsIgnoredDir(%s) = %v, want %v", tt.path, got, tt.ignored)
		}
	}
}

func TestIsIgnoredDir_AnchoredPattern(t *testing.T) {
	dir := t.TempDir()
	runGit(t, dir, "init")

	// Leading "/" anchors the pattern to the repo root.
	os.WriteFile(filepath.Join(dir, ".gitignore"), []byte("/build\n"), 0644)
	os.MkdirAll(filepath.Join(dir, "build"), 0755)
	os.MkdirAll(filepath.Join(dir, "src", "build"), 0755)

	m := New(dir)
	if m == nil {
		t.Fatal("expected matcher")
	}

	// /build matches only at root.
	if !m.IsIgnoredDir(filepath.Join(dir, "build")) {
		t.Error("expected build/ at root to be ignored")
	}
	// src/build should NOT match since pattern is anchored.
	if m.IsIgnoredDir(filepath.Join(dir, "src", "build")) {
		t.Error("expected src/build/ to NOT be ignored (anchored pattern)")
	}
}

func TestIsIgnoredDir_GitInfoExclude(t *testing.T) {
	dir := t.TempDir()
	runGit(t, dir, "init")

	// Write to .git/info/exclude instead of .gitignore.
	infoDir := filepath.Join(dir, ".git", "info")
	os.MkdirAll(infoDir, 0755)
	os.WriteFile(filepath.Join(infoDir, "exclude"), []byte("secret/\n"), 0644)

	os.MkdirAll(filepath.Join(dir, "secret"), 0755)
	os.MkdirAll(filepath.Join(dir, "public"), 0755)

	m := New(dir)
	if m == nil {
		t.Fatal("expected matcher")
	}

	if !m.IsIgnoredDir(filepath.Join(dir, "secret")) {
		t.Error("expected secret/ to be ignored via .git/info/exclude")
	}
	if m.IsIgnoredDir(filepath.Join(dir, "public")) {
		t.Error("expected public/ to NOT be ignored")
	}
}

func TestIsIgnoredDir_RepoRoot(t *testing.T) {
	dir := t.TempDir()
	runGit(t, dir, "init")

	m := New(dir)
	if m == nil {
		t.Fatal("expected matcher")
	}

	if m.IsIgnoredDir(dir) {
		t.Error("repo root should never be ignored")
	}
}

func TestIsIgnoredDir_Caching(t *testing.T) {
	dir := t.TempDir()
	runGit(t, dir, "init")

	os.WriteFile(filepath.Join(dir, ".gitignore"), []byte("cache/\n"), 0644)
	os.MkdirAll(filepath.Join(dir, "cache"), 0755)

	m := New(dir)
	if m == nil {
		t.Fatal("expected matcher")
	}

	// First call computes.
	if !m.IsIgnoredDir(filepath.Join(dir, "cache")) {
		t.Error("expected cache/ to be ignored")
	}

	// Second call uses cache — same result.
	if !m.IsIgnoredDir(filepath.Join(dir, "cache")) {
		t.Error("expected cache/ to still be ignored (from cache)")
	}

	// Verify the cache entry exists.
	m.mu.RLock()
	_, ok := m.dirCache[filepath.Join(dir, "cache")]
	m.mu.RUnlock()
	if !ok {
		t.Error("expected dirCache entry to exist")
	}
}

// E-PENPAL-SCAN: patterns without trailing slash match directories too.
func TestIsIgnoredDir_PatternWithoutTrailingSlash(t *testing.T) {
	dir := t.TempDir()
	runGit(t, dir, "init")

	os.WriteFile(filepath.Join(dir, ".gitignore"), []byte("node_modules\n.env\n"), 0644)
	os.MkdirAll(filepath.Join(dir, "node_modules"), 0755)
	os.MkdirAll(filepath.Join(dir, ".env"), 0755)
	os.MkdirAll(filepath.Join(dir, "src"), 0755)
	os.MkdirAll(filepath.Join(dir, "src", "node_modules"), 0755)

	m := New(dir)
	if m == nil {
		t.Fatal("expected matcher")
	}

	tests := []struct {
		path    string
		ignored bool
	}{
		{filepath.Join(dir, "node_modules"), true},
		{filepath.Join(dir, ".env"), true},
		{filepath.Join(dir, "src", "node_modules"), true},
		{filepath.Join(dir, "src"), false},
	}

	for _, tt := range tests {
		got := m.IsIgnoredDir(tt.path)
		if got != tt.ignored {
			t.Errorf("IsIgnoredDir(%s) = %v, want %v", tt.path, got, tt.ignored)
		}
	}
}

// --- parseLine tests ---

func TestParseLine(t *testing.T) {
	tests := []struct {
		input    string
		ok       bool
		negated  bool
		dirOnly  bool
		anchored bool
		pattern  string
	}{
		{"", false, false, false, false, ""},
		{"# comment", false, false, false, false, ""},
		{"build/", true, false, true, false, "build"},
		{"!important/", true, true, true, false, "important"},
		{"/root-only", true, false, false, true, "root-only"},
		{"*.log", true, false, false, false, "*.log"},
		{"a/b", true, false, false, true, "a/b"},
		{"**/cache", true, false, false, true, "**/cache"},
		{"  ", false, false, false, false, ""},
		{"foo\\ ", true, false, false, false, "foo "},
	}

	for _, tt := range tests {
		r, ok := parseLine(tt.input)
		if ok != tt.ok {
			t.Errorf("parseLine(%q): ok = %v, want %v", tt.input, ok, tt.ok)
			continue
		}
		if !ok {
			continue
		}
		if r.negated != tt.negated {
			t.Errorf("parseLine(%q): negated = %v, want %v", tt.input, r.negated, tt.negated)
		}
		if r.dirOnly != tt.dirOnly {
			t.Errorf("parseLine(%q): dirOnly = %v, want %v", tt.input, r.dirOnly, tt.dirOnly)
		}
		if r.anchored != tt.anchored {
			t.Errorf("parseLine(%q): anchored = %v, want %v", tt.input, r.anchored, tt.anchored)
		}
		if r.pattern != tt.pattern {
			t.Errorf("parseLine(%q): pattern = %q, want %q", tt.input, r.pattern, tt.pattern)
		}
	}
}

// --- globMatch tests ---

func TestGlobMatch(t *testing.T) {
	tests := []struct {
		pattern string
		name    string
		match   bool
	}{
		{"foo", "foo", true},
		{"foo", "bar", false},
		{"*.log", "error.log", true},
		{"*.log", "dir/error.log", false},
		{"*", "anything", true},
		{"*", "a/b", false},
		{"?", "a", true},
		{"?", "ab", false},
		{"[abc]", "a", true},
		{"[abc]", "d", false},
		{"[a-z]", "m", true},
		{"[a-z]", "M", false},
		{"[!a-z]", "M", true},
		{"**", "a/b/c", true},
		{"**/foo", "foo", true},
		{"**/foo", "a/foo", true},
		{"**/foo", "a/b/foo", true},
		{"a/**/b", "a/b", true},
		{"a/**/b", "a/x/b", true},
		{"a/**/b", "a/x/y/b", true},
		{"foo/**", "foo/bar", true},
		{"foo/**", "foo/bar/baz", true},
	}

	for _, tt := range tests {
		got := globMatch(tt.pattern, tt.name)
		if got != tt.match {
			t.Errorf("globMatch(%q, %q) = %v, want %v", tt.pattern, tt.name, got, tt.match)
		}
	}
}
