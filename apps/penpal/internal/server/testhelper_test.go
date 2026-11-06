package server

import (
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"

	"github.com/loganj/penpal/internal/activity"
	"github.com/loganj/penpal/internal/agents"
	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
	"github.com/loganj/penpal/internal/config"
	"github.com/loganj/penpal/internal/discovery"
	"github.com/loganj/penpal/internal/watcher"
)

// testServer creates a Server wired with in-memory dependencies.
// Returns the server, its cache for test data setup, and the comments store.
func testServer(t *testing.T) (*Server, *cache.Cache, *comments.Store) {
	t.Helper()
	c := cache.New()
	act := activity.New()
	w, err := watcher.New(c, act)
	if err != nil {
		t.Fatalf("watcher: %v", err)
	}
	cs := comments.NewStore(c, act)
	cfg := &config.Config{}
	cfgPath := filepath.Join(t.TempDir(), "config.json")
	s := New(c, w, cs, nil, nil, act, cfg, cfgPath)
	// Trigger ensureLoaded so it doesn't interfere with tests
	s.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest(http.MethodGet, "/", nil))
	return s, c, cs
}

// attachSession creates an agent manager on the server and attaches a session
// for the given project, returning the session token. Use this when tests need
// to post role="agent" comments via the REST API.
func attachSession(t *testing.T, s *Server, c *cache.Cache, cs *comments.Store, projectName string) string {
	t.Helper()
	mgr := agents.New(c, cs, 0)
	s.agents = mgr
	sess, err := mgr.Attach(projectName, "", "claude", false)
	if err != nil {
		t.Fatalf("attach session: %v", err)
	}
	return sess.Token
}

// seedProject adds a project with files to the cache for testing.
func seedProject(c *cache.Cache, name, path string, files []cache.FileInfo) discovery.Project {
	project := discovery.Project{
		Path:   path,
		Origin: "workspace",
	}

	// Handle workspace vs standalone naming
	parts := strings.SplitN(name, "/", 2)
	if len(parts) == 2 {
		project.WorkspaceName = parts[0]
		project.Name = parts[1]
	} else {
		project.Name = name
		project.Origin = "standalone"
	}

	c.SetProjects(append(c.Projects(), project))
	if files != nil {
		c.SetProjectFiles(project.QualifiedName(), files)
	}
	return project
}
