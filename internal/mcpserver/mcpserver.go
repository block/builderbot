package mcpserver

import (
	"net/http"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// NewHandler creates an HTTP handler implementing the MCP Streamable HTTP
// protocol. It exposes comment and review tools so AI agents can interact
// with penpal programmatically.
func NewHandler(store *comments.Store, c *cache.Cache) http.Handler {
	server := mcp.NewServer(&mcp.Implementation{
		Name:    "penpal",
		Version: "1.0.0",
	}, &mcp.ServerOptions{
		Instructions: "Penpal operates on markdown files for collaborative document review with humans. File paths are relative to the project root (e.g., thoughts/plans/foo.md). It is NOT for code review.\n\nWhen your reply asks for confirmation or presents options, include suggestedReplies (up to 3 short strings) so the human can respond with one click. For example, if proposing a change, include [\"Yes\", \"No\"] or [\"Yes\", \"Yes, but...\", \"No\"]. Keep suggestions short (1-4 words).",
	})
	registerTools(server, store, c)
	return mcp.NewStreamableHTTPHandler(func(r *http.Request) *mcp.Server {
		return server
	}, nil)
}
