package publish

import (
	"archive/zip"
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestPublish_UploadsValidZip(t *testing.T) {
	var receivedBody []byte
	var receivedContentType string
	var receivedPath string

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		receivedPath = r.URL.Path
		receivedContentType = r.Header.Get("Content-Type")
		var err error
		receivedBody, err = io.ReadAll(r.Body)
		if err != nil {
			t.Fatalf("reading request body: %v", err)
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(BlockcellResult{
			VersionID: "v1",
			Status:    "active",
		})
	}))
	defer server.Close()

	md := []byte("# Test\n\nHello world.\n")
	result, err := Publish(md, "Test Page", "my-site", server.URL)
	if err != nil {
		t.Fatalf("Publish failed: %v", err)
	}

	// Check URL path
	if receivedPath != "/api/v1/sites/my-site/upload" {
		t.Errorf("unexpected path: %s", receivedPath)
	}

	// Check content type is multipart
	if !strings.HasPrefix(receivedContentType, "multipart/form-data") {
		t.Errorf("unexpected content type: %s", receivedContentType)
	}

	// Check result URL
	expectedURL := server.URL + "/sites/my-site/"
	if result.URL != expectedURL {
		t.Errorf("unexpected result URL: got %s, want %s", result.URL, expectedURL)
	}

	// Parse the multipart body to extract the zip
	boundary := strings.Split(receivedContentType, "boundary=")[1]
	reader := strings.NewReader(string(receivedBody))
	mr := newMultipartReader(reader, boundary)
	zipData := extractZipFromMultipart(t, mr)

	// Verify the zip contains index.html
	zr, err := zip.NewReader(bytes.NewReader(zipData), int64(len(zipData)))
	if err != nil {
		t.Fatalf("reading zip: %v", err)
	}
	if len(zr.File) != 1 {
		t.Fatalf("expected 1 file in zip, got %d", len(zr.File))
	}
	if zr.File[0].Name != "index.html" {
		t.Errorf("expected index.html, got %s", zr.File[0].Name)
	}

	// Read and verify index.html content
	rc, err := zr.File[0].Open()
	if err != nil {
		t.Fatalf("opening zip entry: %v", err)
	}
	defer rc.Close()
	htmlBytes, err := io.ReadAll(rc)
	if err != nil {
		t.Fatalf("reading zip entry: %v", err)
	}
	htmlContent := string(htmlBytes)
	if !strings.Contains(htmlContent, "<!DOCTYPE html>") {
		t.Error("index.html missing DOCTYPE")
	}
	if !strings.Contains(htmlContent, "Hello world.") {
		t.Error("index.html missing rendered content")
	}
}

func TestPublish_ErrorResponse(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
		w.Write([]byte("internal server error"))
	}))
	defer server.Close()

	_, err := Publish([]byte("# Test\n"), "Test", "my-site", server.URL)
	if err == nil {
		t.Fatal("expected error for 500 response")
	}
	if !strings.Contains(err.Error(), "status 500") {
		t.Errorf("expected status 500 in error, got: %v", err)
	}
}

func TestGenerateSiteName(t *testing.T) {
	tests := []struct {
		project  string
		filePath string
		want     string
	}{
		{"myproject", "docs/readme.md", "penpal-myproject-docs-readme"},
		{"My Project", "thoughts/plans/big-plan.md", "penpal-my-project-thoughts-plans-big-plan"},
		{"ws/proj", "file.md", "penpal-ws-proj-file"},
	}
	for _, tt := range tests {
		got := GenerateSiteName(tt.project, tt.filePath)
		if got != tt.want {
			t.Errorf("GenerateSiteName(%q, %q) = %q, want %q", tt.project, tt.filePath, got, tt.want)
		}
	}
}

// helpers

func newMultipartReader(r io.Reader, boundary string) *multipartReader {
	return &multipartReader{r: r, boundary: boundary}
}

type multipartReader struct {
	r        io.Reader
	boundary string
}

func extractZipFromMultipart(t *testing.T, mr *multipartReader) []byte {
	t.Helper()
	// Simple extraction: find the zip data between multipart boundaries.
	// The zip starts after the headers (double CRLF) and ends before the closing boundary.
	data, err := io.ReadAll(mr.r)
	if err != nil {
		t.Fatalf("reading multipart: %v", err)
	}
	body := string(data)
	// Find the start of the file content (after headers)
	headerEnd := strings.Index(body, "\r\n\r\n")
	if headerEnd == -1 {
		t.Fatal("could not find end of multipart headers")
	}
	content := body[headerEnd+4:]
	// Find the closing boundary
	closeBoundary := "\r\n--" + mr.boundary
	boundaryIdx := strings.Index(content, closeBoundary)
	if boundaryIdx == -1 {
		t.Fatal("could not find closing boundary")
	}
	return []byte(content[:boundaryIdx])
}
