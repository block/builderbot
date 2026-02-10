package server

import (
	"testing"
	"time"

	"github.com/loganj/birdseye/internal/cache"
)

func TestGroupRP1Files(t *testing.T) {
	tests := []struct {
		name     string
		files    []cache.FileInfo
		expected GroupedFiles
	}{
		{
			name:  "empty input",
			files: []cache.FileInfo{},
			expected: GroupedFiles{
				Groups: nil,
			},
		},
		{
			name: "feature files only",
			files: []cache.FileInfo{
				{
					Name:      "requirements.md",
					FullPath:  ".rp1/work/features/auth-refactor/requirements.md",
					Source:    "rp1",
					FeatureID: "auth-refactor",
					Category:  "",
					ModTime:   time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "design.md",
					FullPath:  ".rp1/work/features/auth-refactor/design.md",
					Source:    "rp1",
					FeatureID: "auth-refactor",
					Category:  "",
					ModTime:   time.Date(2026, 1, 2, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "tasks.md",
					FullPath:  ".rp1/work/features/data-layer/tasks.md",
					Source:    "rp1",
					FeatureID: "data-layer",
					Category:  "",
					ModTime:   time.Date(2026, 1, 3, 0, 0, 0, 0, time.UTC),
				},
			},
			expected: GroupedFiles{
				Groups: []FileGroup{
					{
						Type: "feature",
						Name: "auth-refactor",
						Files: []FileData{
							{Name: "requirements.md", DisplayName: "auth-refactor/requirements.md", Path: ".rp1/work/features/auth-refactor/requirements.md", Source: "rp1"},
							{Name: "design.md", DisplayName: "auth-refactor/design.md", Path: ".rp1/work/features/auth-refactor/design.md", Source: "rp1"},
						},
					},
					{
						Type: "feature",
						Name: "data-layer",
						Files: []FileData{
							{Name: "tasks.md", DisplayName: "data-layer/tasks.md", Path: ".rp1/work/features/data-layer/tasks.md", Source: "rp1"},
						},
					},
				},
			},
		},
		{
			name: "category files only",
			files: []cache.FileInfo{
				{
					Name:      "index.md",
					FullPath:  ".rp1/context/index.md",
					Source:    "rp1",
					FeatureID: "",
					Category:  "Context",
					ModTime:   time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "architecture.md",
					FullPath:  ".rp1/context/architecture.md",
					Source:    "rp1",
					FeatureID: "",
					Category:  "Context",
					ModTime:   time.Date(2026, 1, 2, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "my-prd.md",
					FullPath:  ".rp1/work/prds/my-prd.md",
					Source:    "rp1",
					FeatureID: "",
					Category:  "PRDs",
					ModTime:   time.Date(2026, 1, 3, 0, 0, 0, 0, time.UTC),
				},
			},
			expected: GroupedFiles{
				Groups: []FileGroup{
					{
						Type: "category",
						Name: "Context",
						Files: []FileData{
							{Name: "index.md", DisplayName: "index.md", Path: ".rp1/context/index.md", Source: "rp1"},
							{Name: "architecture.md", DisplayName: "architecture.md", Path: ".rp1/context/architecture.md", Source: "rp1"},
						},
					},
					{
						Type: "category",
						Name: "PRDs",
						Files: []FileData{
							{Name: "my-prd.md", DisplayName: "my-prd.md", Path: ".rp1/work/prds/my-prd.md", Source: "rp1"},
						},
					},
				},
			},
		},
		{
			name: "mixed category and feature files",
			files: []cache.FileInfo{
				{
					Name:      "index.md",
					FullPath:  ".rp1/context/index.md",
					Source:    "rp1",
					FeatureID: "",
					Category:  "Context",
					ModTime:   time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "requirements.md",
					FullPath:  ".rp1/work/features/auth/requirements.md",
					Source:    "rp1",
					FeatureID: "auth",
					Category:  "",
					ModTime:   time.Date(2026, 1, 2, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "build-1.md",
					FullPath:  ".rp1/work/quick-builds/build-1.md",
					Source:    "rp1",
					FeatureID: "",
					Category:  "Quick Builds",
					ModTime:   time.Date(2026, 1, 3, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "charter.md",
					FullPath:  ".rp1/work/charter.md",
					Source:    "rp1",
					FeatureID: "",
					Category:  "Other",
					ModTime:   time.Date(2026, 1, 4, 0, 0, 0, 0, time.UTC),
				},
			},
			expected: GroupedFiles{
				Groups: []FileGroup{
					{
						Type: "category",
						Name: "Context",
						Files: []FileData{
							{Name: "index.md", DisplayName: "index.md", Path: ".rp1/context/index.md", Source: "rp1"},
						},
					},
					{
						Type: "category",
						Name: "Quick Builds",
						Files: []FileData{
							{Name: "build-1.md", DisplayName: "build-1.md", Path: ".rp1/work/quick-builds/build-1.md", Source: "rp1"},
						},
					},
					{
						Type: "category",
						Name: "Other",
						Files: []FileData{
							{Name: "charter.md", DisplayName: "charter.md", Path: ".rp1/work/charter.md", Source: "rp1"},
						},
					},
					{
						Type: "feature",
						Name: "auth",
						Files: []FileData{
							{Name: "requirements.md", DisplayName: "auth/requirements.md", Path: ".rp1/work/features/auth/requirements.md", Source: "rp1"},
						},
					},
				},
			},
		},
		{
			name: "alphabetical feature sorting",
			files: []cache.FileInfo{
				{
					Name:      "tasks.md",
					FullPath:  ".rp1/work/features/zebra-feature/tasks.md",
					Source:    "rp1",
					FeatureID: "zebra-feature",
					Category:  "",
					ModTime:   time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "tasks.md",
					FullPath:  ".rp1/work/features/alpha-feature/tasks.md",
					Source:    "rp1",
					FeatureID: "alpha-feature",
					Category:  "",
					ModTime:   time.Date(2026, 1, 2, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "tasks.md",
					FullPath:  ".rp1/work/features/middle-feature/tasks.md",
					Source:    "rp1",
					FeatureID: "middle-feature",
					Category:  "",
					ModTime:   time.Date(2026, 1, 3, 0, 0, 0, 0, time.UTC),
				},
			},
			expected: GroupedFiles{
				Groups: []FileGroup{
					{
						Type: "feature",
						Name: "alpha-feature",
						Files: []FileData{
							{Name: "tasks.md", DisplayName: "alpha-feature/tasks.md", Path: ".rp1/work/features/alpha-feature/tasks.md", Source: "rp1"},
						},
					},
					{
						Type: "feature",
						Name: "middle-feature",
						Files: []FileData{
							{Name: "tasks.md", DisplayName: "middle-feature/tasks.md", Path: ".rp1/work/features/middle-feature/tasks.md", Source: "rp1"},
						},
					},
					{
						Type: "feature",
						Name: "zebra-feature",
						Files: []FileData{
							{Name: "tasks.md", DisplayName: "zebra-feature/tasks.md", Path: ".rp1/work/features/zebra-feature/tasks.md", Source: "rp1"},
						},
					},
				},
			},
		},
		{
			name: "all categories in correct order",
			files: []cache.FileInfo{
				{
					Name:      "charter.md",
					FullPath:  ".rp1/work/charter.md",
					Source:    "rp1",
					FeatureID: "",
					Category:  "Other",
					ModTime:   time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "build-1.md",
					FullPath:  ".rp1/work/quick-builds/build-1.md",
					Source:    "rp1",
					FeatureID: "",
					Category:  "Quick Builds",
					ModTime:   time.Date(2026, 1, 2, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "my-prd.md",
					FullPath:  ".rp1/work/prds/my-prd.md",
					Source:    "rp1",
					FeatureID: "",
					Category:  "PRDs",
					ModTime:   time.Date(2026, 1, 3, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "index.md",
					FullPath:  ".rp1/context/index.md",
					Source:    "rp1",
					FeatureID: "",
					Category:  "Context",
					ModTime:   time.Date(2026, 1, 4, 0, 0, 0, 0, time.UTC),
				},
			},
			expected: GroupedFiles{
				Groups: []FileGroup{
					{
						Type: "category",
						Name: "Context",
						Files: []FileData{
							{Name: "index.md", DisplayName: "index.md", Path: ".rp1/context/index.md", Source: "rp1"},
						},
					},
					{
						Type: "category",
						Name: "PRDs",
						Files: []FileData{
							{Name: "my-prd.md", DisplayName: "my-prd.md", Path: ".rp1/work/prds/my-prd.md", Source: "rp1"},
						},
					},
					{
						Type: "category",
						Name: "Quick Builds",
						Files: []FileData{
							{Name: "build-1.md", DisplayName: "build-1.md", Path: ".rp1/work/quick-builds/build-1.md", Source: "rp1"},
						},
					},
					{
						Type: "category",
						Name: "Other",
						Files: []FileData{
							{Name: "charter.md", DisplayName: "charter.md", Path: ".rp1/work/charter.md", Source: "rp1"},
						},
					},
				},
			},
		},
		{
			name: "files within groups maintain input order",
			files: []cache.FileInfo{
				{
					Name:      "field-notes.md",
					FullPath:  ".rp1/work/features/auth/field-notes.md",
					Source:    "rp1",
					FeatureID: "auth",
					Category:  "",
					FileType:  "field-notes",
					ModTime:   time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "requirements.md",
					FullPath:  ".rp1/work/features/auth/requirements.md",
					Source:    "rp1",
					FeatureID: "auth",
					Category:  "",
					FileType:  "requirements",
					ModTime:   time.Date(2026, 1, 2, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "tasks.md",
					FullPath:  ".rp1/work/features/auth/tasks.md",
					Source:    "rp1",
					FeatureID: "auth",
					Category:  "",
					FileType:  "tasks",
					ModTime:   time.Date(2026, 1, 3, 0, 0, 0, 0, time.UTC),
				},
				{
					Name:      "design.md",
					FullPath:  ".rp1/work/features/auth/design.md",
					Source:    "rp1",
					FeatureID: "auth",
					Category:  "",
					FileType:  "design",
					ModTime:   time.Date(2026, 1, 4, 0, 0, 0, 0, time.UTC),
				},
			},
			expected: GroupedFiles{
				Groups: []FileGroup{
					{
						Type: "feature",
						Name: "auth",
						Files: []FileData{
							{Name: "field-notes.md", DisplayName: "auth/field-notes.md", Path: ".rp1/work/features/auth/field-notes.md", Source: "rp1", FileType: "field-notes"},
							{Name: "requirements.md", DisplayName: "auth/requirements.md", Path: ".rp1/work/features/auth/requirements.md", Source: "rp1", FileType: "requirements"},
							{Name: "tasks.md", DisplayName: "auth/tasks.md", Path: ".rp1/work/features/auth/tasks.md", Source: "rp1", FileType: "tasks"},
							{Name: "design.md", DisplayName: "auth/design.md", Path: ".rp1/work/features/auth/design.md", Source: "rp1", FileType: "design"},
						},
					},
				},
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := groupRP1Files(tt.files)

			if len(got.Groups) != len(tt.expected.Groups) {
				t.Fatalf("expected %d groups, got %d", len(tt.expected.Groups), len(got.Groups))
			}

			for i, expectedGroup := range tt.expected.Groups {
				gotGroup := got.Groups[i]

				if gotGroup.Type != expectedGroup.Type {
					t.Errorf("group %d: expected type %q, got %q", i, expectedGroup.Type, gotGroup.Type)
				}

				if gotGroup.Name != expectedGroup.Name {
					t.Errorf("group %d: expected name %q, got %q", i, expectedGroup.Name, gotGroup.Name)
				}

				if len(gotGroup.Files) != len(expectedGroup.Files) {
					t.Errorf("group %d (%s): expected %d files, got %d", i, gotGroup.Name, len(expectedGroup.Files), len(gotGroup.Files))
					continue
				}

				for j, expectedFile := range expectedGroup.Files {
					gotFile := gotGroup.Files[j]

					if gotFile.Name != expectedFile.Name {
						t.Errorf("group %d (%s), file %d: expected name %q, got %q", i, gotGroup.Name, j, expectedFile.Name, gotFile.Name)
					}

					if gotFile.DisplayName != expectedFile.DisplayName {
						t.Errorf("group %d (%s), file %d: expected display name %q, got %q", i, gotGroup.Name, j, expectedFile.DisplayName, gotFile.DisplayName)
					}

					if gotFile.Path != expectedFile.Path {
						t.Errorf("group %d (%s), file %d: expected path %q, got %q", i, gotGroup.Name, j, expectedFile.Path, gotFile.Path)
					}

					if gotFile.Source != expectedFile.Source {
						t.Errorf("group %d (%s), file %d: expected source %q, got %q", i, gotGroup.Name, j, expectedFile.Source, gotFile.Source)
					}
				}
			}
		})
	}
}

func TestConvertToFileData(t *testing.T) {
	files := []cache.FileInfo{
		{
			Name:      "requirements.md",
			FullPath:  ".rp1/work/features/test-feature/requirements.md",
			Source:    "rp1",
			FeatureID: "test-feature",
			ModTime:   time.Date(2026, 1, 15, 10, 30, 0, 0, time.UTC),
			FileType:  "requirements",
		},
		{
			Name:     "index.md",
			FullPath: ".rp1/context/index.md",
			Source:   "rp1",
			Category: "Context",
			ModTime:  time.Date(2026, 1, 14, 9, 0, 0, 0, time.UTC),
			FileType: "other",
		},
	}

	result := convertToFileData(files)

	if len(result) != 2 {
		t.Fatalf("expected 2 files, got %d", len(result))
	}

	// Check first file (feature file)
	if result[0].Name != "requirements.md" {
		t.Errorf("expected name %q, got %q", "requirements.md", result[0].Name)
	}
	if result[0].DisplayName != "test-feature/requirements.md" {
		t.Errorf("expected display name %q, got %q", "test-feature/requirements.md", result[0].DisplayName)
	}
	if result[0].Path != ".rp1/work/features/test-feature/requirements.md" {
		t.Errorf("expected path %q, got %q", ".rp1/work/features/test-feature/requirements.md", result[0].Path)
	}
	if result[0].FileType != "requirements" {
		t.Errorf("expected file type %q, got %q", "requirements", result[0].FileType)
	}

	// Check second file (category file)
	if result[1].Name != "index.md" {
		t.Errorf("expected name %q, got %q", "index.md", result[1].Name)
	}
	if result[1].DisplayName != "index.md" {
		t.Errorf("expected display name %q, got %q", "index.md", result[1].DisplayName)
	}
	if result[1].Path != ".rp1/context/index.md" {
		t.Errorf("expected path %q, got %q", ".rp1/context/index.md", result[1].Path)
	}
}

func BenchmarkGroupRP1Files_SmallSet(b *testing.B) {
	files := make([]cache.FileInfo, 10)
	for i := 0; i < 10; i++ {
		files[i] = cache.FileInfo{
			Name:      "requirements.md",
			FullPath:  ".rp1/work/features/feature-" + string(rune('a'+i)) + "/requirements.md",
			Source:    "rp1",
			FeatureID: "feature-" + string(rune('a'+i)),
			ModTime:   time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
		}
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		groupRP1Files(files)
	}
}

func BenchmarkGroupRP1Files_MediumSet(b *testing.B) {
	files := make([]cache.FileInfo, 50)
	for i := 0; i < 50; i++ {
		files[i] = cache.FileInfo{
			Name:      "requirements.md",
			FullPath:  ".rp1/work/features/feature-" + string(rune('a'+i)) + "/requirements.md",
			Source:    "rp1",
			FeatureID: "feature-" + string(rune('a'+i)),
			ModTime:   time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
		}
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		groupRP1Files(files)
	}
}

func BenchmarkGroupRP1Files_LargeSet(b *testing.B) {
	files := make([]cache.FileInfo, 100)
	for i := 0; i < 100; i++ {
		files[i] = cache.FileInfo{
			Name:      "requirements.md",
			FullPath:  ".rp1/work/features/feature-" + string(rune('a'+i)) + "/requirements.md",
			Source:    "rp1",
			FeatureID: "feature-" + string(rune('a'+i)),
			ModTime:   time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
		}
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		groupRP1Files(files)
	}
}

func BenchmarkGroupRP1Files_MixedContent(b *testing.B) {
	files := make([]cache.FileInfo, 60)

	for i := 0; i < 10; i++ {
		files[i] = cache.FileInfo{
			Name:     "index.md",
			FullPath: ".rp1/context/file-" + string(rune('a'+i)) + ".md",
			Source:   "rp1",
			Category: "Context",
			ModTime:  time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
		}
	}

	for i := 10; i < 60; i++ {
		files[i] = cache.FileInfo{
			Name:      "requirements.md",
			FullPath:  ".rp1/work/features/feature-" + string(rune('a'+i)) + "/requirements.md",
			Source:    "rp1",
			FeatureID: "feature-" + string(rune('a'+i)),
			ModTime:   time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
		}
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		groupRP1Files(files)
	}
}
