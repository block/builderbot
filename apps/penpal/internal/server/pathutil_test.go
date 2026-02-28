package server

import "testing"

func TestIsSubpath(t *testing.T) {
	tests := []struct {
		parent string
		child  string
		want   bool
	}{
		{"/a/b", "/a/b/c", true},
		{"/a/b", "/a/b/c/d", true},
		{"/a/b", "/a/b", true},
		{"/a/b", "/a/bc", false},
		{"/a/b", "/a", false},
		{"/a/b", "/a/b/../c", false},
		{"/a/b", "/a/b/../../etc/passwd", false},
	}
	for _, tt := range tests {
		got := isSubpath(tt.parent, tt.child)
		if got != tt.want {
			t.Errorf("isSubpath(%q, %q) = %v, want %v", tt.parent, tt.child, got, tt.want)
		}
	}
}
