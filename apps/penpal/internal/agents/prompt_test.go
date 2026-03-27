package agents

import (
	"strings"
	"testing"
)

// E-PENPAL-AGENT-PROMPT: verifies buildPrompt contains key agent instructions.
func TestBuildPrompt(t *testing.T) {
	prompt := buildPrompt("myproject")

	// Verify project name is embedded
	if !strings.Contains(prompt, `"myproject"`) {
		t.Error("expected prompt to contain the quoted project name")
	}

	// Verify key tool references
	if !strings.Contains(prompt, "penpal_files_in_review") {
		t.Error("expected prompt to mention penpal_files_in_review")
	}
	if !strings.Contains(prompt, "penpal_wait_for_changes") {
		t.Error("expected prompt to mention penpal_wait_for_changes")
	}

	// Verify exit condition about 10 timeouts
	if !strings.Contains(prompt, "10 consecutive timeouts") {
		t.Error("expected prompt to mention 10 consecutive timeouts exit condition")
	}
}

// E-PENPAL-AGENT-PROMPT: verifies buildPrompt uses project name in multiple places.
func TestBuildPromptProjectNameSubstitution(t *testing.T) {
	prompt := buildPrompt("Dev/my-docs")

	// The project name should appear at least 3 times (once per Sprintf %q arg)
	count := strings.Count(prompt, `"Dev/my-docs"`)
	if count < 3 {
		t.Errorf("expected project name to appear at least 3 times, got %d", count)
	}
}
