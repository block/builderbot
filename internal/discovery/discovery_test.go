package discovery

import (
	"testing"
)

func TestExtractFeatureID(t *testing.T) {
	tests := []struct {
		name       string
		fullPath   string
		sourceName string
		want       string
	}{
		{
			name:       "standard feature file",
			fullPath:   ".rp1/work/features/rp1-differentiation/requirements.md",
			sourceName: "rp1",
			want:       "rp1-differentiation",
		},
		{
			name:       "feature file with design",
			fullPath:   ".rp1/work/features/auth-system/design.md",
			sourceName: "rp1",
			want:       "auth-system",
		},
		{
			name:       "feature file with nested path",
			fullPath:   ".rp1/work/features/rp1-auto-discovery/tasks.md",
			sourceName: "rp1",
			want:       "rp1-auto-discovery",
		},
		{
			name:       "non-feature file (context)",
			fullPath:   ".rp1/context/index.md",
			sourceName: "rp1",
			want:       "",
		},
		{
			name:       "non-feature file (prds)",
			fullPath:   ".rp1/work/prds/my-prd.md",
			sourceName: "rp1",
			want:       "",
		},
		{
			name:       "non-rp1 source",
			fullPath:   "thoughts/plans/foo.md",
			sourceName: "thoughts",
			want:       "",
		},
		{
			name:       "malformed path (no filename)",
			fullPath:   ".rp1/work/features/rp1-test",
			sourceName: "rp1",
			want:       "",
		},
		{
			name:       "empty path",
			fullPath:   "",
			sourceName: "rp1",
			want:       "",
		},
		{
			name:       "feature with whitespace in ID (trimmed)",
			fullPath:   ".rp1/work/features/ rp1-test /tasks.md",
			sourceName: "rp1",
			want:       "rp1-test",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := ExtractFeatureID(tt.fullPath, tt.sourceName)
			if got != tt.want {
				t.Errorf("ExtractFeatureID() = %v, want %v", got, tt.want)
			}
		})
	}
}

func TestDetectRP1Category(t *testing.T) {
	tests := []struct {
		name       string
		fullPath   string
		sourceName string
		want       string
	}{
		{
			name:       "context file",
			fullPath:   ".rp1/context/index.md",
			sourceName: "rp1",
			want:       "Context",
		},
		{
			name:       "context file nested",
			fullPath:   ".rp1/context/modules.md",
			sourceName: "rp1",
			want:       "Context",
		},
		{
			name:       "prd file",
			fullPath:   ".rp1/work/prds/my-prd.md",
			sourceName: "rp1",
			want:       "PRDs",
		},
		{
			name:       "quick build file",
			fullPath:   ".rp1/work/quick-builds/build-1.md",
			sourceName: "rp1",
			want:       "Quick Builds",
		},
		{
			name:       "feature file (not categorized)",
			fullPath:   ".rp1/work/features/rp1-test/requirements.md",
			sourceName: "rp1",
			want:       "",
		},
		{
			name:       "archived file (not categorized)",
			fullPath:   ".rp1/work/archives/old-feature/tasks.md",
			sourceName: "rp1",
			want:       "",
		},
		{
			name:       "other rp1 file (charter)",
			fullPath:   ".rp1/work/charter.md",
			sourceName: "rp1",
			want:       "Other",
		},
		{
			name:       "other rp1 file (random)",
			fullPath:   ".rp1/README.md",
			sourceName: "rp1",
			want:       "Other",
		},
		{
			name:       "non-rp1 source",
			fullPath:   "thoughts/plans/foo.md",
			sourceName: "thoughts",
			want:       "",
		},
		{
			name:       "empty path",
			fullPath:   "",
			sourceName: "rp1",
			want:       "Other",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := DetectRP1Category(tt.fullPath, tt.sourceName)
			if got != tt.want {
				t.Errorf("DetectRP1Category() = %v, want %v", got, tt.want)
			}
		})
	}
}

func BenchmarkExtractFeatureID(b *testing.B) {
	testCases := []struct {
		fullPath   string
		sourceName string
	}{
		{".rp1/work/features/rp1-differentiation/requirements.md", "rp1"},
		{".rp1/work/features/auth-system/design.md", "rp1"},
		{".rp1/work/features/rp1-auto-discovery/tasks.md", "rp1"},
		{".rp1/context/index.md", "rp1"},
		{".rp1/work/prds/my-prd.md", "rp1"},
		{"thoughts/plans/foo.md", "thoughts"},
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		tc := testCases[i%len(testCases)]
		ExtractFeatureID(tc.fullPath, tc.sourceName)
	}
}

func BenchmarkDetectRP1Category(b *testing.B) {
	testCases := []struct {
		fullPath   string
		sourceName string
	}{
		{".rp1/context/index.md", "rp1"},
		{".rp1/work/prds/my-prd.md", "rp1"},
		{".rp1/work/quick-builds/build-1.md", "rp1"},
		{".rp1/work/features/rp1-test/requirements.md", "rp1"},
		{".rp1/work/charter.md", "rp1"},
		{"thoughts/plans/foo.md", "thoughts"},
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		tc := testCases[i%len(testCases)]
		DetectRP1Category(tc.fullPath, tc.sourceName)
	}
}
