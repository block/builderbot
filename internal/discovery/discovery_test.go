package discovery

import (
	"testing"
)

func TestGroupRP1Paths(t *testing.T) {
	st := GetSourceType("rp1")
	if st == nil || st.GroupFiles == nil {
		t.Fatal("rp1 source type not registered or has no GroupFiles")
	}

	tests := []struct {
		name     string
		paths    []string
		expected []FileGroup
	}{
		{
			name:     "empty input",
			paths:    nil,
			expected: nil,
		},
		{
			name: "context files",
			paths: []string{
				"context/index.md",
				"context/architecture.md",
			},
			expected: []FileGroup{
				{Name: "Context", Paths: []string{"context/index.md", "context/architecture.md"}},
			},
		},
		{
			name: "feature files grouped by feature ID",
			paths: []string{
				"work/features/auth/requirements.md",
				"work/features/auth/design.md",
				"work/features/data-layer/tasks.md",
			},
			expected: []FileGroup{
				{Name: "auth", Paths: []string{"work/features/auth/requirements.md", "work/features/auth/design.md"}},
				{Name: "data-layer", Paths: []string{"work/features/data-layer/tasks.md"}},
			},
		},
		{
			name: "category ordering: Context, PRDs, Quick Builds, Other",
			paths: []string{
				"work/charter.md",
				"work/quick-builds/build-1.md",
				"work/prds/my-prd.md",
				"context/index.md",
			},
			expected: []FileGroup{
				{Name: "Context", Paths: []string{"context/index.md"}},
				{Name: "PRDs", Paths: []string{"work/prds/my-prd.md"}},
				{Name: "Quick Builds", Paths: []string{"work/quick-builds/build-1.md"}},
				{Name: "Other", Paths: []string{"work/charter.md"}},
			},
		},
		{
			name: "categories before features, features sorted alphabetically",
			paths: []string{
				"context/index.md",
				"work/features/zebra/tasks.md",
				"work/features/alpha/requirements.md",
				"work/prds/my-prd.md",
			},
			expected: []FileGroup{
				{Name: "Context", Paths: []string{"context/index.md"}},
				{Name: "PRDs", Paths: []string{"work/prds/my-prd.md"}},
				{Name: "alpha", Paths: []string{"work/features/alpha/requirements.md"}},
				{Name: "zebra", Paths: []string{"work/features/zebra/tasks.md"}},
			},
		},
		{
			name: "malformed feature path goes to Other",
			paths: []string{
				"work/features/lonely-file.md",
			},
			expected: []FileGroup{
				{Name: "Other", Paths: []string{"work/features/lonely-file.md"}},
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := st.GroupFiles(tt.paths)

			if len(got) != len(tt.expected) {
				t.Fatalf("expected %d groups, got %d: %+v", len(tt.expected), len(got), got)
			}

			for i, eg := range tt.expected {
				if got[i].Name != eg.Name {
					t.Errorf("group %d: expected name %q, got %q", i, eg.Name, got[i].Name)
				}
				if len(got[i].Paths) != len(eg.Paths) {
					t.Errorf("group %d (%s): expected %d paths, got %d", i, eg.Name, len(eg.Paths), len(got[i].Paths))
					continue
				}
				for j, ep := range eg.Paths {
					if got[i].Paths[j] != ep {
						t.Errorf("group %d (%s), path %d: expected %q, got %q", i, eg.Name, j, ep, got[i].Paths[j])
					}
				}
			}
		})
	}
}

func BenchmarkGroupRP1Paths(b *testing.B) {
	st := GetSourceType("rp1")
	if st == nil || st.GroupFiles == nil {
		b.Fatal("rp1 source type not registered or has no GroupFiles")
	}

	paths := make([]string, 60)
	for i := 0; i < 10; i++ {
		paths[i] = "context/file-" + string(rune('a'+i)) + ".md"
	}
	for i := 10; i < 60; i++ {
		paths[i] = "work/features/feature-" + string(rune('a'+i)) + "/requirements.md"
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		st.GroupFiles(paths)
	}
}
