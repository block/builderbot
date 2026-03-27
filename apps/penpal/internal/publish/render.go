package publish

import (
	_ "embed"
	"fmt"
	"html"
	"strings"

	"github.com/loganj/penpal/internal/markdown"
)

//go:embed mermaid.min.js
var mermaidJS string

// RenderHTML converts markdown to a complete, self-contained HTML page
// with TOC sidebar and mermaid support.
// E-PENPAL-PUBLISH-RENDER: strips frontmatter, renders via goldmark, extracts headings.
func RenderHTML(src []byte, title string) ([]byte, error) {
	src = markdown.StripFrontmatter(src)
	htmlContent, err := markdown.Render(src)
	if err != nil {
		return nil, fmt.Errorf("rendering markdown: %w", err)
	}
	headings := markdown.ExtractHeadings(htmlContent)

	var toc strings.Builder
	for _, h := range headings {
		toc.WriteString(fmt.Sprintf(
			`<a href="#%s" class="level-%d">%s</a>`,
			html.EscapeString(h.ID), h.Level, html.EscapeString(h.Text),
		))
		toc.WriteByte('\n')
	}

	page := fmt.Sprintf(`<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>%s</title>
<style>
%s
</style>
</head>
<body>
<div class="layout">
<nav class="sidebar">
<div class="sidebar-title">%s</div>
<button class="copy-md-btn" onclick="copyMarkdown(this)">Copy markdown</button>
<div class="sidebar-nav">
%s
</div>
</nav>
<main>
<article class="content">
%s
</article>
</main>
</div>
<template id="raw-markdown">%s</template>
<script>%s</script>
<script>
function copyMarkdown(btn) {
    var md = document.getElementById('raw-markdown').content.textContent;
    navigator.clipboard.writeText(md).then(function() {
        var orig = btn.textContent;
        btn.textContent = 'Copied!';
        setTimeout(function() { btn.textContent = orig; }, 1500);
        showToast('Markdown copied to clipboard');
    });
}
function showToast(msg) {
    var t = document.createElement('div');
    t.className = 'toast';
    t.textContent = msg;
    document.body.appendChild(t);
    requestAnimationFrame(function() { t.classList.add('show'); });
    setTimeout(function() {
        t.classList.remove('show');
        setTimeout(function() { t.remove(); }, 300);
    }, 2000);
}
mermaid.initialize({ startOnLoad: false, theme: 'default' });
document.addEventListener('DOMContentLoaded', function() {
    var codeBlocks = document.querySelectorAll('code.language-mermaid');
    var items = [];
    var counter = 0;
    codeBlocks.forEach(function(code) {
        var pre = code.parentElement;
        if (!pre || pre.tagName !== 'PRE') return;
        var text = code.textContent;
        var div = document.createElement('div');
        div.className = 'mermaid-container';
        var mermaidDiv = document.createElement('div');
        mermaidDiv.className = 'mermaid';
        div.appendChild(mermaidDiv);
        pre.parentNode.replaceChild(div, pre);
        items.push({ div: mermaidDiv, source: text });
    });
    function renderNext(i) {
        if (i >= items.length) return;
        counter++;
        mermaid.render('pub-mermaid-' + counter, items[i].source)
            .then(function(result) { items[i].div.innerHTML = result.svg; })
            .catch(function(err) { items[i].div.innerHTML = '<p style="color:red">Diagram render error</p>'; })
            .then(function() { renderNext(i + 1); });
    }
    renderNext(0);
});
</script>
</body>
</html>`,
		html.EscapeString(title),
		pageCSS,
		html.EscapeString(title),
		toc.String(),
		htmlContent,
		html.EscapeString(string(src)),
		mermaidJS,
	)

	return []byte(page), nil
}

const pageCSS = `
* { box-sizing: border-box; }
body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; margin: 0; padding: 0; color: #333; background: #f8f9fa; }
.layout { display: grid; grid-template-columns: 260px 1fr; min-height: 100vh; }

/* Sidebar */
.sidebar { background: #fff; border-right: 1px solid #e8e8e8; padding: 24px 16px; position: sticky; top: 0; height: 100vh; overflow-y: auto; }
.sidebar-title { font-weight: 600; font-size: 1em; margin-bottom: 12px; padding-bottom: 12px; border-bottom: 1px solid #e8e8e8; }
.copy-md-btn { display: block; width: 100%; padding: 6px 8px; margin-bottom: 16px; font-size: 0.8em; color: #555; background: #f5f5f5; border: 1px solid #ddd; border-radius: 4px; cursor: pointer; text-align: left; }
.copy-md-btn:hover { background: #eee; color: #333; border-color: #ccc; }
.sidebar-nav { display: flex; flex-direction: column; }
.sidebar-nav a { display: block; padding: 4px 8px; font-size: 0.85em; color: #555; text-decoration: none; border-radius: 4px; line-height: 1.4; }
.sidebar-nav a:hover { background: #f5f5f5; color: #333; }
.sidebar-nav .level-2 { padding-left: 20px; }
.sidebar-nav .level-3 { padding-left: 32px; font-size: 0.75em; color: #777; }

/* Main content */
main { padding: 32px 48px; max-width: 900px; }

/* Article content — matches penpal file page styles */
.content { background: #fff; padding: 32px 40px; border-radius: 8px; border: 1px solid #e8e8e8; line-height: 1.7; }
.content h1, .content h2, .content h3, .content h4 { color: #333; font-weight: 600; }
.content h1 { border-bottom: 1px solid #e8e8e8; padding-bottom: 8px; }
.content h2 { border-bottom: 1px solid #e8e8e8; padding-bottom: 6px; }
.content code { background: #f5f5f5; padding: 2px 6px; border-radius: 4px; font-size: 0.85em; color: #333; word-break: break-word; }
.content td code { background: none; padding: 0; font-size: 0.8em; color: #555; }
.content pre { background: #282a36; padding: 16px; border-radius: 6px; overflow-x: auto; }
.content pre code { background: none; padding: 0; color: #f8f8f2; }
.content ul, .content ol { padding-left: 24px; }
.content li { margin-bottom: 4px; }
.content blockquote { border-left: 3px solid #0066cc; margin-left: 0; padding-left: 16px; color: #666; }
.content table { border-collapse: collapse; width: 100%; margin: 16px 0; }
.content th, .content td { border: 1px solid #e8e8e8; padding: 8px 12px; text-align: left; }
.content th { background: #f5f5f5; }
.content hr { border: none; border-top: 1px solid #e8e8e8; margin: 24px 0; }
.content img { max-width: 100%; }
.content a { color: #0066cc; }
.content .chroma { background: #282a36; }
.content .mermaid-container { background: #fff; border: 1px solid #e8e8e8; border-radius: 6px; padding: 16px; overflow-x: auto; text-align: center; }

/* Toast */
.toast { position: fixed; bottom: 24px; left: 50%; transform: translateX(-50%) translateY(20px); background: #333; color: #fff; padding: 10px 20px; border-radius: 6px; font-size: 0.85em; opacity: 0; transition: opacity 0.3s, transform 0.3s; z-index: 1000; pointer-events: none; }
.toast.show { opacity: 1; transform: translateX(-50%) translateY(0); }

/* Responsive */
@media (max-width: 768px) {
    .layout { grid-template-columns: 1fr; }
    .sidebar { display: none; }
    main { padding: 16px; }
}
`
