package publish

import (
	"archive/zip"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"regexp"
	"strings"
)

const defaultBlockcellURL = "https://blockcell.sqprod.co"

type BlockcellResult struct {
	VersionID string `json:"version_id"`
	Status    string `json:"status"`
	URL       string `json:"url"`
}

// Publish renders the markdown file to HTML and uploads it to Blockcell.
func Publish(markdownSrc []byte, title, siteName, blockcellURL string) (*BlockcellResult, error) {
	if blockcellURL == "" {
		blockcellURL = defaultBlockcellURL
	}

	htmlContent, err := RenderHTML(markdownSrc, title)
	if err != nil {
		return nil, err
	}

	// Create zip in memory with index.html
	var zipBuf bytes.Buffer
	zw := zip.NewWriter(&zipBuf)
	w, err := zw.Create("index.html")
	if err != nil {
		return nil, fmt.Errorf("creating zip entry: %w", err)
	}
	if _, err := w.Write(htmlContent); err != nil {
		return nil, fmt.Errorf("writing zip entry: %w", err)
	}
	if err := zw.Close(); err != nil {
		return nil, fmt.Errorf("closing zip: %w", err)
	}

	// Build multipart upload
	var body bytes.Buffer
	mw := multipart.NewWriter(&body)
	part, err := mw.CreateFormFile("file", siteName+".zip")
	if err != nil {
		return nil, fmt.Errorf("creating form file: %w", err)
	}
	if _, err := io.Copy(part, &zipBuf); err != nil {
		return nil, fmt.Errorf("copying zip to form: %w", err)
	}
	mw.Close()

	url := fmt.Sprintf("%s/api/v1/sites/%s/upload", blockcellURL, siteName)
	req, err := http.NewRequest("POST", url, &body)
	if err != nil {
		return nil, fmt.Errorf("creating request: %w", err)
	}
	req.Header.Set("Content-Type", mw.FormDataContentType())
	req.Header.Set("Accept", "application/json")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("uploading to blockcell: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		respBody, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("blockcell upload failed (status %d): %s", resp.StatusCode, string(respBody))
	}

	var result BlockcellResult
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, fmt.Errorf("decoding response: %w", err)
	}
	result.URL = fmt.Sprintf("%s/sites/%s/", blockcellURL, siteName)
	return &result, nil
}

var nonAlphanumeric = regexp.MustCompile(`[^a-z0-9]+`)

// GenerateSiteName produces a deterministic, URL-safe slug from project name and file path.
func GenerateSiteName(project, filePath string) string {
	// Combine project and file path, strip extension
	raw := project + "-" + strings.TrimSuffix(filePath, ".md")
	raw = strings.ToLower(raw)
	// Replace path separators and non-alphanumeric chars with hyphens
	slug := nonAlphanumeric.ReplaceAllString(raw, "-")
	// Trim leading/trailing hyphens
	slug = strings.Trim(slug, "-")
	// Prefix with "penpal-"
	slug = "penpal-" + slug
	// Blockcell site names have a max length; truncate if needed
	if len(slug) > 63 {
		slug = slug[:63]
		slug = strings.TrimRight(slug, "-")
	}
	return slug
}
