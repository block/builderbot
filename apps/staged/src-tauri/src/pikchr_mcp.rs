//! MCP server exposing the `generate_pikchr` tool.
//!
//! Note-writing sessions (project notes and local branch notes) use it to
//! author and validate their Pikchr diagrams before shipping them:
//!
//! `generate_pikchr` turns a natural-language description into validated Pikchr
//! by running a focused internal agent sub-session that renders and repairs its
//! own output (via [`crate::pikchr_subsession`]) before returning the final
//! source plus a path to a saved preview image. Revisions pass the current
//! diagram's source back in so the sub-agent edits real Pikchr rather than
//! re-describing from scratch.
//! The sub-session renders and inspects candidate diagrams through the internal
//! [`run_preview`] path — the same engine the tool ultimately hands back — so
//! the agent never has to hand-write Pikchr or drive a separate preview step.
//!
//! Fidelity: rendering goes through the `pikchr` crate, which bundles the same
//! official `pikchr.c` that the frontend's `pikchr-js` compiles to WASM. The
//! shape geometry — the part that matters most for overlap detection — is
//! therefore identical to what the user eventually sees. Overlap detection then
//! reads the shape and text rectangles straight off the `usvg` tree that also
//! rasterizes the preview, so labels are checked with real, shaped extents
//! rather than bare anchor points. `usvg`/`resvg` lay text out with native
//! system fonts, not the frontend's browser metrics, so text extents differ by
//! a hair (and font fallback can differ); label-overlap thresholds are kept
//! forgiving to match, and this is acceptable for a preview.
//!
//! Unlike `project_mcp`, this handler touches no store, registry, or project.
//! It carries only the provider id and `AppHandle` that `generate_pikchr` needs
//! to spin up its sub-session, so it remains safe to attach to any local
//! session.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use axum::Router;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData, ServerHandler};

use crate::agent::AcpDriver;

/// Wall-clock cap for one `generate_pikchr` call. Each call spins a provider
/// subprocess and runs several turns; the cap keeps a stuck sub-agent from
/// running indefinitely. Enforced by cancelling the sub-session's token.
const GENERATE_PIKCHR_TIMEOUT: Duration = Duration::from_secs(600);

/// Cap the rasterized PNG so a runaway diagram can't allocate a huge pixmap.
const MAX_RENDER_DIMENSION: u32 = 4096;
/// Default rasterization scale — 2× keeps labels legible.
const DEFAULT_SCALE: f32 = 2.0;
const MIN_SCALE: f32 = 0.5;
const MAX_SCALE: f32 = 4.0;
/// Ignore overlaps thinner than this (px in Pikchr's coordinate space) so that
/// shapes which merely share an edge or corner aren't flagged.
const MIN_OVERLAP_PX: f64 = 1.0;
/// Overlaps involving a text label use a more forgiving threshold: usvg lays
/// text out with native fonts rather than the frontend's browser metrics, and
/// its text bounding boxes are the generous SVG line boxes, so a label may
/// nudge a neighbour by a hair without being a real collision.
const MIN_TEXT_OVERLAP_PX: f64 = 3.0;
/// Truncate derived shape labels in the overlap summary.
const MAX_LABEL_CHARS: usize = 48;
/// Temp-file prefix for generated Pikchr preview PNGs.
const TEMP_IMAGE_PREFIX: &str = "staged-pikchr-preview-";

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct GeneratePikchrParams {
    /// Fine-grained, freeform description of the desired diagram: what boxes,
    /// arrows, and labels it has, how they're laid out, and how they relate.
    /// The more specific, the closer the result.
    pub description: String,
    /// When revising an existing diagram, the current diagram's Pikchr source
    /// (the contents of its ```pikchr block, without the fences). The
    /// sub-agent edits this instead of starting from scratch, so intent drifts
    /// less across iterations.
    pub previous_pikchr: Option<String>,
    /// Rasterization scale for the returned PNG preview. Higher values produce
    /// a larger image with more legible labels. Defaults to 2.0; clamped to
    /// the range [0.5, 4.0].
    pub scale: Option<f32>,
}

/// Axis-aligned bounding box in Pikchr's (unscaled) SVG coordinate space.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BBox {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl BBox {
    /// Build a `BBox` from a usvg canvas-space rectangle. Pikchr emits a 1:1
    /// `viewBox`, so usvg's canvas coordinates are the same numbers as the raw
    /// SVG path coordinates.
    fn from_rect(r: usvg::Rect) -> Self {
        BBox {
            min_x: r.left() as f64,
            min_y: r.top() as f64,
            max_x: r.right() as f64,
            max_y: r.bottom() as f64,
        }
    }

    fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    fn height(&self) -> f64 {
        self.max_y - self.min_y
    }

    fn area(&self) -> f64 {
        self.width().max(0.0) * self.height().max(0.0)
    }

    fn center(&self) -> (f64, f64) {
        (
            (self.min_x + self.max_x) / 2.0,
            (self.min_y + self.max_y) / 2.0,
        )
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }

    /// Width and height of the rectangle where `self` and `other` overlap.
    /// Both are zero (or negative) when the boxes don't intersect.
    fn overlap_extent(&self, other: &BBox) -> (f64, f64) {
        let w = self.max_x.min(other.max_x) - self.min_x.max(other.min_x);
        let h = self.max_y.min(other.max_y) - self.min_y.max(other.min_y);
        (w, h)
    }
}

/// Whether an [`Element`] is a container shape or a rendered text label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ElementKind {
    Box,
    Text,
}

/// One piece of rendered geometry with a real rectangle in Pikchr's coordinate
/// space. Both box-like shapes and text labels are represented uniformly so
/// overlaps can be checked between any pair — box↔box, label↔box, label↔label.
#[derive(Clone, Debug)]
struct Element {
    kind: ElementKind,
    bounds: BBox,
    /// A `Text` element's rendered content, or a `Box`'s derived label (the
    /// text sitting inside it, joined). `None` for an unlabeled box.
    text: Option<String>,
}

/// One detected overlap between two elements, by index into the element list.
#[derive(Clone, Debug)]
struct Overlap {
    a: usize,
    b: usize,
    overlap_w: f64,
    overlap_h: f64,
}

// =============================================================================
// Pikchr rendering (SVG)
// =============================================================================

/// Outcome of rendering Pikchr source to SVG.
struct RenderedSvg {
    svg: String,
    width: i64,
    height: i64,
}

/// Render Pikchr source to SVG using the bundled `pikchr.c` engine.
///
/// Returns the Pikchr error message (plain text) on parse/layout failure so the
/// caller can hand it straight back to the agent.
fn render_pikchr_svg(source: &str) -> Result<RenderedSvg, String> {
    use pikchr::{Pikchr, PikchrFlags};

    let pic = Pikchr::render(source, None, PikchrFlags::default())?;
    Ok(RenderedSvg {
        svg: pic.rendered().to_string(),
        width: pic.width() as i64,
        height: pic.height() as i64,
    })
}

// =============================================================================
// Overlap detection
// =============================================================================

/// Extract every box-like shape and text label from the rendered SVG tree.
///
/// The tree is walked recursively (Pikchr may nest elements in groups).
/// Box-like shapes are *closed, stroked* paths: Pikchr draws boxes/ovals/
/// diamonds as closed outlined paths, while arrow shafts are open paths and
/// arrowheads are filled (stroke-less) polygons — so both arrow parts fall out
/// naturally. Text labels take their true, shaped rectangle from usvg's
/// `abs_bounding_box`, computed with the same fonts that draw the PNG, so a
/// label finally has a real extent instead of a bare anchor point.
fn extract_elements(tree: &usvg::Tree) -> Vec<Element> {
    fn walk(group: &usvg::Group, out: &mut Vec<Element>) {
        for node in group.children() {
            match node {
                usvg::Node::Group(g) => walk(g, out),
                usvg::Node::Path(p) => {
                    if let Some(bounds) = box_path_bounds(p) {
                        out.push(Element {
                            kind: ElementKind::Box,
                            bounds,
                            text: None,
                        });
                    }
                }
                usvg::Node::Text(t) => {
                    let text = normalize_label(&text_content(t));
                    if !text.is_empty() {
                        out.push(Element {
                            kind: ElementKind::Text,
                            bounds: BBox::from_rect(t.abs_bounding_box()),
                            text: Some(text),
                        });
                    }
                }
                usvg::Node::Image(_) => {}
            }
        }
    }

    let mut elements = Vec::new();
    walk(tree.root(), &mut elements);
    elements
}

/// Bounds of a path when it is a container shape — a closed, stroked outline —
/// else `None`. Degenerate hairline shapes are dropped.
fn box_path_bounds(path: &usvg::Path) -> Option<BBox> {
    // Only outlined (stroked) shapes are containers; a fill-only path is an
    // arrowhead polygon.
    path.stroke()?;
    let closed = path
        .data()
        .segments()
        .any(|seg| seg == tiny_skia::PathSegment::Close);
    if !closed {
        return None;
    }
    let bounds = BBox::from_rect(path.abs_bounding_box());
    (bounds.width() > MIN_OVERLAP_PX && bounds.height() > MIN_OVERLAP_PX).then_some(bounds)
}

/// Concatenate a text node's chunks into a single string.
fn text_content(text: &usvg::Text) -> String {
    text.chunks()
        .iter()
        .flat_map(|chunk| chunk.text().chars())
        .collect()
}

/// Collapse whitespace into single ASCII spaces and trim. Pikchr separates
/// label words with non-breaking spaces, which `split_whitespace` handles.
fn normalize_label(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_label(s: &str) -> String {
    if s.chars().count() <= MAX_LABEL_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX_LABEL_CHARS).collect();
    out.push('…');
    out
}

/// For each text element, the index of its "home" box — the smallest box whose
/// bounds contain the label's centre — or `None` when it sits in no box. Box
/// elements map to `None`. This both attributes labels to boxes and tells a
/// label resting inside its box apart from one straying onto a neighbour.
fn home_boxes(elements: &[Element]) -> Vec<Option<usize>> {
    elements
        .iter()
        .map(|element| {
            if element.kind != ElementKind::Text {
                return None;
            }
            let (cx, cy) = element.bounds.center();
            elements
                .iter()
                .enumerate()
                .filter(|(_, b)| b.kind == ElementKind::Box && b.bounds.contains(cx, cy))
                .min_by(|(_, a), (_, b)| {
                    a.bounds
                        .area()
                        .partial_cmp(&b.bounds.area())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)
        })
        .collect()
}

/// Fill each box's `text` with the labels whose home is that box, joined in
/// document order, so overlaps can name boxes by their label.
fn assign_box_labels(elements: &mut [Element], homes: &[Option<usize>]) {
    // Gather each box's label first, then write, so we're not reading and
    // mutating `elements` at the same time.
    let labels: Vec<Option<String>> = elements
        .iter()
        .enumerate()
        .map(|(bi, element)| {
            if element.kind != ElementKind::Box {
                return None;
            }
            let parts: Vec<&str> = elements
                .iter()
                .zip(homes)
                .filter(|(_, home)| **home == Some(bi))
                .filter_map(|(e, _)| e.text.as_deref())
                .collect();
            (!parts.is_empty()).then(|| truncate_label(&parts.join(" ")))
        })
        .collect();
    for (element, label) in elements.iter_mut().zip(labels) {
        if label.is_some() {
            element.text = label;
        }
    }
}

/// Find pairs of elements that overlap by more than a hairline.
///
/// Box↔box overlaps are layout collisions, as before. Overlaps involving a
/// label are the new, text-aware cases: two labels colliding (text↔text), or a
/// label straying onto a box it doesn't belong to (text↔box). A label sitting
/// inside its own — or an enclosing — box is the normal case and is not
/// reported, and the separate lines of one multi-line label (which share a home
/// box) don't flag each other.
fn find_overlaps(elements: &[Element], homes: &[Option<usize>]) -> Vec<Overlap> {
    let mut overlaps = Vec::new();
    for a in 0..elements.len() {
        for b in (a + 1)..elements.len() {
            let ea = &elements[a];
            let eb = &elements[b];
            let (w, h) = ea.bounds.overlap_extent(&eb.bounds);
            let both_boxes = ea.kind == ElementKind::Box && eb.kind == ElementKind::Box;
            let threshold = if both_boxes {
                MIN_OVERLAP_PX
            } else {
                MIN_TEXT_OVERLAP_PX
            };
            if w <= threshold || h <= threshold {
                continue;
            }
            match (ea.kind, eb.kind) {
                (ElementKind::Box, ElementKind::Box) => {}
                (ElementKind::Text, ElementKind::Text) => {
                    // Separate lines of the same label share a home box and are
                    // stacked by design — don't flag them against each other.
                    if let (Some(ha), Some(hb)) = (homes[a], homes[b]) {
                        if ha == hb {
                            continue;
                        }
                    }
                }
                _ => {
                    // A label inside a box (its own or an enclosing one) is
                    // expected; only flag it when the box does not contain the
                    // label's centre.
                    let (text, boxel) = if ea.kind == ElementKind::Text {
                        (ea, eb)
                    } else {
                        (eb, ea)
                    };
                    let (cx, cy) = text.bounds.center();
                    if boxel.bounds.contains(cx, cy) {
                        continue;
                    }
                }
            }
            overlaps.push(Overlap {
                a,
                b,
                overlap_w: w,
                overlap_h: h,
            });
        }
    }
    overlaps
}

/// Run the full overlap analysis on a parsed Pikchr SVG tree.
fn analyze_overlaps(tree: &usvg::Tree) -> (Vec<Element>, Vec<Overlap>) {
    let mut elements = extract_elements(tree);
    let homes = home_boxes(&elements);
    assign_box_labels(&mut elements, &homes);
    let overlaps = find_overlaps(&elements, &homes);
    (elements, overlaps)
}

/// Human-readable description of one element for the overlap summary.
fn describe_element(element: &Element) -> String {
    let noun = match element.kind {
        ElementKind::Box => "box",
        ElementKind::Text => "label",
    };
    match &element.text {
        Some(text) => format!("{noun} \"{text}\""),
        None => {
            let (cx, cy) = element.bounds.center();
            format!("{noun} near ({cx:.0}, {cy:.0})")
        }
    }
}

/// Build the text summary returned alongside the image. Text matters because
/// not every provider forwards image content to the model, and even
/// vision-less models can act on a textual overlap report.
fn build_summary(width: i64, height: i64, elements: &[Element], overlaps: &[Overlap]) -> String {
    let mut out = format!("Rendered Pikchr diagram: {width}×{height} px.");
    if overlaps.is_empty() {
        out.push_str("\nNo overlaps detected.");
        return out;
    }

    out.push_str(&format!(
        "\n⚠ {} overlapping pair(s) detected:",
        overlaps.len()
    ));
    for o in overlaps {
        out.push_str(&format!(
            "\n- {} overlaps {} (≈ {:.0}×{:.0} px)",
            describe_element(&elements[o.a]),
            describe_element(&elements[o.b]),
            o.overlap_w,
            o.overlap_h
        ));
    }
    out.push_str(
        "\nIf these overlaps aren't intended, call `generate_pikchr` again passing this source as \
`previous_pikchr` with a description that separates the shapes/labels — e.g. set an explicit flow \
direction, use named nodes with explicit anchors (`with .w at …`, `arrow from A.e to B.w`), give \
long labels room or shorten them, and avoid percentage-length arrows between `fit` boxes. \
Otherwise the diagram is fine to keep.",
    );
    out
}

// =============================================================================
// PNG rasterization
// =============================================================================

/// System font database, loaded once. resvg needs fonts to shape box labels.
fn font_database() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
    .clone()
}

/// Parse a rendered Pikchr SVG into a usvg tree, loading system fonts so text
/// is shaped. `run_preview` parses once and reuses the tree for both overlap
/// analysis (which reads path + text bounding boxes off it) and rasterization.
fn parse_svg_tree(svg: &str) -> Option<usvg::Tree> {
    let options = usvg::Options {
        fontdb: font_database(),
        ..Default::default()
    };
    match usvg::Tree::from_str(svg, &options) {
        Ok(tree) => Some(tree),
        Err(e) => {
            log::warn!("[pikchr_mcp] usvg failed to parse rendered SVG: {e}");
            None
        }
    }
}

/// Rasterize a parsed Pikchr SVG tree to a PNG, scaled by `scale` (clamped, and
/// reduced further if needed to stay within `MAX_RENDER_DIMENSION`). Returns the
/// PNG bytes, or `None` if rasterization fails (the caller degrades to
/// text-only).
fn rasterize_tree_to_png(tree: &usvg::Tree, scale: f32) -> Option<Vec<u8>> {
    let size = tree.size();
    let (w, h) = (size.width().max(1.0), size.height().max(1.0));

    let mut s = scale.clamp(MIN_SCALE, MAX_SCALE);
    // Scale down to fit within MAX_RENDER_DIMENSION. We let `fit` go below
    // MIN_SCALE here (rather than flooring it) so an oversized diagram is shown
    // whole — shrunk but uncropped — which keeps the overlap layout visible
    // instead of clipping it to the top-left corner.
    let fit = (MAX_RENDER_DIMENSION as f32 / w).min(MAX_RENDER_DIMENSION as f32 / h);
    if s > fit {
        s = fit;
    }

    let px_w = ((w * s).ceil() as u32).clamp(1, MAX_RENDER_DIMENSION);
    let px_h = ((h * s).ceil() as u32).clamp(1, MAX_RENDER_DIMENSION);

    let mut pixmap = tiny_skia::Pixmap::new(px_w, px_h)?;
    // White background so the diagram reads like the (light-mode) app preview
    // instead of rendering on transparency.
    pixmap.fill(tiny_skia::Color::WHITE);

    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(s, s),
        &mut pixmap.as_mut(),
    );

    match pixmap.encode_png() {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            log::warn!("[pikchr_mcp] failed to encode PNG: {e}");
            None
        }
    }
}

// =============================================================================
// Tool orchestration
// =============================================================================

/// Outcome of rendering a candidate diagram: the PNG (if rasterization
/// succeeded), a text summary of dimensions and overlaps, and whether the
/// source failed to render at all.
///
/// `pub(crate)` so the `generate_pikchr` sub-session loop can render and
/// inspect candidate diagrams through this shared render/overlap path.
pub(crate) struct PreviewOutcome {
    pub(crate) png: Option<Vec<u8>>,
    pub(crate) summary: String,
    pub(crate) is_error: bool,
}

/// Render + analyze, producing the content blocks for the tool result.
/// Synchronous and self-contained so it can run on a blocking thread and be
/// unit-tested directly. `scale` is taken as-is and clamped internally.
pub(crate) fn run_preview(source: &str, scale: f32) -> PreviewOutcome {
    if source.trim().is_empty() {
        return PreviewOutcome {
            png: None,
            summary: "Pikchr source is empty — nothing to render.".to_string(),
            is_error: true,
        };
    }
    let rendered = match render_pikchr_svg(source) {
        Ok(rendered) => rendered,
        Err(err) => {
            return PreviewOutcome {
                png: None,
                summary: format!("Pikchr could not render this diagram:\n{}", err.trim()),
                is_error: true,
            };
        }
    };

    // Parse the rendered SVG once; both overlap analysis and rasterization read
    // geometry from the same tree, and both need text laid out with the same
    // fonts. If usvg can't parse Pikchr's own output (rare), degrade to
    // dimensions only rather than failing the whole call.
    let Some(tree) = parse_svg_tree(&rendered.svg) else {
        return PreviewOutcome {
            png: None,
            summary: format!(
                "Rendered Pikchr diagram: {}×{} px.\n(Diagram analysis and preview unavailable: \
the rendered SVG could not be parsed.)",
                rendered.width, rendered.height
            ),
            is_error: false,
        };
    };

    let (elements, overlaps) = analyze_overlaps(&tree);
    let mut summary = build_summary(rendered.width, rendered.height, &elements, &overlaps);

    let png = rasterize_tree_to_png(&tree, scale);
    if png.is_none() {
        summary.push_str("\n(Image rasterization unavailable; reporting geometry only.)");
    }

    PreviewOutcome {
        png,
        summary,
        is_error: false,
    }
}

/// Persist a generated preview image in the OS temp directory and return its
/// path. Use `create_new` with a UUID-based filename so parallel tool calls
/// never overwrite each other.
fn write_png_to_temp_file(png: &[u8]) -> std::io::Result<PathBuf> {
    let temp_dir = std::env::temp_dir();
    loop {
        let path = temp_dir.join(format!("{TEMP_IMAGE_PREFIX}{}.png", uuid::Uuid::new_v4()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(png)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

#[derive(Clone)]
struct PikchrToolsHandler {
    /// Provider id the `generate_pikchr` sub-session runs under (the parent
    /// session's agent, so the sub-agent matches what the user chose).
    provider_id: String,
    /// Handle used to resolve the bundled Pikchr grammar reference for the
    /// sub-agent's prompt.
    app_handle: tauri::AppHandle,
    tool_router: ToolRouter<Self>,
}

impl PikchrToolsHandler {
    fn new(provider_id: String, app_handle: tauri::AppHandle) -> Self {
        Self {
            provider_id,
            app_handle,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl PikchrToolsHandler {
    #[tool(
        description = "Generate a validated Pikchr diagram from a natural-language description. \
An internal Pikchr specialist writes the diagram, renders it, and repairs syntax errors on its own \
before returning. Prefer this over hand-writing Pikchr. Pass a fine-grained `description` (boxes, \
arrows, labels, layout, relationships). To revise an existing diagram, also pass its current source \
as `previous_pikchr` so it is edited rather than redrawn. Returns the validated Pikchr source (drop \
it into a ```pikchr fenced code block), a filesystem path to a rendered PNG preview, and a summary that reports any \
overlapping shapes or labels. Review the preview and the summary: if the layout needs work, call \
this again passing the returned source as `previous_pikchr` with a description that adjusts it."
    )]
    async fn generate_pikchr(
        &self,
        Parameters(p): Parameters<GeneratePikchrParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let scale = p.scale.unwrap_or(DEFAULT_SCALE);
        let provider_id = self.provider_id.clone();
        // The sub-session always runs locally, so resolve a local grammar path
        // (workspace_name = None).
        let grammar_reference =
            crate::session_commands::resolve_pikchr_grammar_reference(&self.app_handle, None);

        // Cancellation token owned by *this* future (the parent MCP request).
        // The worker gets a clone; the parent keeps the token alive through a
        // `DropGuard`. If the MCP client abandons this tool call, the future is
        // dropped, the guard cancels the token, and the sub-session's provider
        // subprocess is torn down promptly — rather than running detached until
        // the wall-clock timeout. The worker arms this same token on timeout.
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let _cancel_on_drop = cancel.drop_guard();

        // The ACP driver spawns tasks via `spawn_local`, which requires a
        // `LocalSet`; the MCP server's request tasks don't run inside one. So
        // drive the whole generation loop on a dedicated thread with its own
        // current-thread runtime + LocalSet, mirroring `session_runner`.
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(Err(format!(
                        "Failed to create runtime for generate_pikchr: {e}"
                    )));
                    return;
                }
            };
            let local = tokio::task::LocalSet::new();
            let result = local.block_on(&rt, async move {
                let driver = AcpDriver::new(&provider_id)?;
                // Enforce the wall-clock cap by cancelling the sub-session's
                // token; the driver shuts its subprocess down gracefully. The
                // parent's `DropGuard` cancels this same token if the MCP
                // client abandons the call before the timeout fires.
                let timeout_cancel = worker_cancel.clone();
                tokio::task::spawn_local(async move {
                    tokio::time::sleep(GENERATE_PIKCHR_TIMEOUT).await;
                    timeout_cancel.cancel();
                });
                crate::pikchr_subsession::generate_pikchr_source(
                    &driver,
                    &grammar_reference,
                    &p.description,
                    p.previous_pikchr.as_deref(),
                    scale,
                    &worker_cancel,
                )
                .await
            });
            let _ = tx.send(result);
        });

        let outcome = rx
            .await
            .map_err(|e| {
                ErrorData::internal_error(format!("generate_pikchr worker dropped: {e}"), None)
            })?
            .map_err(|e| ErrorData::internal_error(e, None))?;

        let mut content = Vec::new();
        if let Some(png) = &outcome.png {
            let path = write_png_to_temp_file(png).map_err(|e| {
                ErrorData::internal_error(
                    format!("Failed to write Pikchr preview image to temp dir: {e}"),
                    None,
                )
            })?;
            content.push(Content::text(format!(
                "Rendered preview image path: {}",
                path.display()
            )));
        }
        content.push(Content::text(outcome.source));
        // Always hand back the render summary — "No overlaps detected." or the
        // ⚠ overlap report — so the calling agent can review the layout and
        // decide whether to keep the diagram or re-call to adjust it.
        content.push(Content::text(outcome.summary));
        Ok(CallToolResult::success(content))
    }
}

#[tool_handler]
impl ServerHandler for PikchrToolsHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

/// Start a local MCP HTTP server exposing the `generate_pikchr` tool.
///
/// Returns the bound port and a `JoinHandle`. The server runs until the handle
/// (and its parent `LocalSet`) is dropped. `provider_id` is the parent
/// session's agent, used by `generate_pikchr` to run its sub-session (an empty
/// string is tolerated — the server still starts and `generate_pikchr` then
/// fails per-call rather than failing session startup). `app_handle` resolves
/// the bundled Pikchr grammar reference for the sub-agent.
pub async fn start_pikchr_mcp_server(
    provider_id: String,
    app_handle: tauri::AppHandle,
) -> Result<(u16, JoinHandle<()>), String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind pikchr MCP listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get local address: {e}"))?
        .port();

    let service = StreamableHttpService::new(
        move || {
            Ok(PikchrToolsHandler::new(
                provider_id.clone(),
                app_handle.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let router = Router::new().route_service("/mcp", service);

    log::debug!("[pikchr_mcp] HTTP server bound on port {port}");

    let handle = tokio::task::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            log::error!("[pikchr_mcp] HTTP server error: {e}");
        }
    });

    Ok((port, handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The known-overlapping diagram from the prior investigation note
    /// (da951fca): percentage-length arrows between large `fit` boxes with no
    /// explicit flow direction, producing a diagonal cascade of colliding boxes.
    const OVERLAPPING_SOURCE: &str = r#"linerad = 4px
box "goose-internal (OPEN SOURCE)" "typed OTel catalog → TelemetrySink facade" fit fill 0xeef6ff
arrow down 35%
box "Sink = OTLP exporter (Block build)" "via Tauri native export_otel_logs (CORS)" fit fill 0xfff3d6
arrow right 60% "OTLP /v1/logs + auth" above
box "Block OTel Collector" "OTel→UAP mapping (from CDF manifest)" fit fill 0xffe6e6
arrow right 50%
box "unifiedevents/batch" "→ Snowflake (UAP unchanged)" fit fill 0xffd6d6
arrow down 30% from 1st box.s
box "Sink = NO-OP (default / external clone)" "no socket, no Block deps → builds anywhere" fit fill 0xe8f5e9"#;

    /// The corrected version from the same note: explicit flow, named nodes,
    /// explicit anchors, full-length arrows. Verified to have zero overlaps.
    const CORRECTED_SOURCE: &str = r#"linerad = 8px
boxht = 0.5

GI: box "goose-internal (OPEN SOURCE)" "typed OTel catalog → TelemetrySink facade" fit fill 0xeef6ff

OTLP: box "Sink = OTLP exporter (Block build)" "via Tauri native export_otel_logs (CORS)" fit fill 0xfff3d6 with .w at 1.0 right of GI.e + (0,0.45)
NOOP: box "Sink = NO-OP (default / external clone)" "no socket, no Block deps → builds anywhere" fit fill 0xe8f5e9 with .w at OTLP.w - (0,0.9)

arrow from GI.e to OTLP.w "Block build" above
arrow from GI.e to NOOP.w "ext. clone" below

COLL: box "Block OTel Collector" "OTel→UAP mapping (from CDF manifest)" fit fill 0xffe6e6 with .w at 0.9 right of OTLP.e
arrow from OTLP.e to COLL.w "OTLP /v1/logs + auth" above
SNOW: box "unifiedevents/batch" "→ Snowflake (UAP unchanged)" fit fill 0xffd6d6 with .w at 0.6 right of COLL.e
arrow from COLL.e to SNOW.w"#;

    /// Render `source` and parse it into a usvg tree the way `run_preview` does,
    /// so geometry tests read the exact same rectangles the tool reports.
    fn tree_for(source: &str) -> usvg::Tree {
        let rendered = render_pikchr_svg(source).expect("source should render");
        let options = usvg::Options {
            fontdb: font_database(),
            ..Default::default()
        };
        usvg::Tree::from_str(&rendered.svg, &options).expect("usvg should parse")
    }

    fn count_boxes(elements: &[Element]) -> usize {
        elements
            .iter()
            .filter(|e| e.kind == ElementKind::Box)
            .count()
    }

    /// Build a synthetic element with an explicit rectangle. Overlaps that
    /// involve a label depend on the *shaped* text rectangle usvg computes, and
    /// usvg silently drops any text it can't find a font for — so a round trip
    /// through the render/parse pipeline only reports a label collision when the
    /// host happens to have a matching font (dev macOS does; a lean CI box may
    /// not). Feeding the pure detection logic (`home_boxes` + `find_overlaps`)
    /// fixed geometry keeps these cases deterministic across environments.
    fn element(
        kind: ElementKind,
        text: &str,
        min_x: f64,
        min_y: f64,
        max_x: f64,
        max_y: f64,
    ) -> Element {
        Element {
            kind,
            bounds: BBox {
                min_x,
                min_y,
                max_x,
                max_y,
            },
            text: (!text.is_empty()).then(|| text.to_string()),
        }
    }

    #[test]
    fn box_bounds_match_raw_path_coordinates() {
        // usvg canvas coordinates line up 1:1 with Pikchr's raw SVG coordinates
        // (Pikchr emits a 1:1 viewBox), so a box's bounds equal its `<path>`
        // geometry — the invariant the overlap check relies on.
        let tree = tree_for(r#"box "Start" fit"#);
        let (elements, _) = analyze_overlaps(&tree);
        let boxes: Vec<_> = elements
            .iter()
            .filter(|e| e.kind == ElementKind::Box)
            .collect();
        assert_eq!(boxes.len(), 1, "expected a single box");
        let b = boxes[0].bounds;
        // From the emitted path `M2.16,2.16 … 55.84,32.4Z`.
        assert!((b.min_x - 2.16).abs() < 0.5, "min_x was {}", b.min_x);
        assert!((b.min_y - 2.16).abs() < 0.5, "min_y was {}", b.min_y);
        assert!((b.max_x - 55.84).abs() < 0.5, "max_x was {}", b.max_x);
        assert!((b.max_y - 32.4).abs() < 0.5, "max_y was {}", b.max_y);
    }

    #[test]
    fn arrowheads_and_shafts_are_not_counted_as_boxes() {
        // An arrow renders as an open shaft path plus a filled (stroke-less)
        // arrowhead polygon; neither is a container, so only the two boxes count.
        let tree = tree_for(r#"box "A" fit; arrow right; box "B" fit"#);
        let (elements, _) = analyze_overlaps(&tree);
        assert_eq!(count_boxes(&elements), 2, "expected exactly two boxes");
    }

    #[test]
    fn overlap_detector_flags_known_overlapping_source() {
        let tree = tree_for(OVERLAPPING_SOURCE);
        let (elements, overlaps) = analyze_overlaps(&tree);
        assert_eq!(count_boxes(&elements), 5, "expected five boxes");
        assert!(
            !overlaps.is_empty(),
            "expected overlaps for the cascade diagram, found none"
        );
    }

    #[test]
    fn overlap_detector_passes_corrected_source() {
        let tree = tree_for(CORRECTED_SOURCE);
        let (_elements, overlaps) = analyze_overlaps(&tree);
        assert!(
            overlaps.is_empty(),
            "corrected diagram should have no overlaps, found {overlaps:?}"
        );
    }

    #[test]
    fn label_inside_its_box_is_not_flagged() {
        // The common case: a box with a label centered inside it. The label
        // rectangle sits within the box, so it must not be reported as an
        // overlap even though the two rectangles intersect.
        let elements = vec![
            element(ElementKind::Box, "", 0.0, 0.0, 100.0, 40.0),
            element(ElementKind::Text, "Contained label", 20.0, 12.0, 80.0, 28.0),
        ];
        let homes = home_boxes(&elements);
        let overlaps = find_overlaps(&elements, &homes);
        assert!(
            overlaps.is_empty(),
            "a label inside its own box is not an overlap, found {overlaps:?}"
        );
    }

    #[test]
    fn colliding_free_labels_are_flagged() {
        // Two free-standing labels stacked on the same point collide. Box-vs-box
        // geometry can't see this; text bounding boxes can. The rectangles below
        // are the ones usvg shapes for these labels on a font-equipped host.
        let elements = vec![
            element(
                ElementKind::Text,
                "first wide label",
                47.0,
                10.6,
                119.0,
                23.9,
            ),
            element(
                ElementKind::Text,
                "second wide label",
                40.0,
                10.6,
                126.0,
                23.9,
            ),
        ];
        let homes = home_boxes(&elements);
        let overlaps = find_overlaps(&elements, &homes);
        assert!(
            overlaps
                .iter()
                .any(|o| elements[o.a].kind == ElementKind::Text
                    && elements[o.b].kind == ElementKind::Text),
            "expected a text-vs-text overlap, found {overlaps:?}"
        );
    }

    #[test]
    fn valid_source_produces_png_and_dimensions() {
        let outcome = run_preview("box \"hello\"", DEFAULT_SCALE);
        assert!(!outcome.is_error);
        assert!(outcome.png.is_some(), "expected a PNG for valid source");
        assert!(!outcome.png.unwrap().is_empty());
        assert!(outcome.summary.contains("px"));
    }

    #[test]
    fn writes_png_preview_to_unique_temp_path() {
        let png = b"fake png bytes";
        let temp_dir = std::env::temp_dir();
        let first = write_png_to_temp_file(png).expect("first image should write");
        let second = write_png_to_temp_file(png).expect("second image should write");

        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(temp_dir.as_path()));
        assert_eq!(std::fs::read(&first).unwrap(), png);
        assert_eq!(std::fs::read(&second).unwrap(), png);

        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }

    #[test]
    fn malformed_source_reports_error() {
        let outcome = run_preview("box \"unterminated", DEFAULT_SCALE);
        assert!(outcome.is_error);
        assert!(outcome.png.is_none());
        assert!(outcome.summary.to_lowercase().contains("pikchr"));
    }

    #[test]
    fn empty_source_reports_error() {
        let outcome = run_preview("   \n  ", DEFAULT_SCALE);
        assert!(outcome.is_error);
        assert!(outcome.png.is_none());
    }

    #[test]
    fn summary_lists_overlap_count() {
        let rendered = render_pikchr_svg(OVERLAPPING_SOURCE).unwrap();
        let tree = tree_for(OVERLAPPING_SOURCE);
        let (elements, overlaps) = analyze_overlaps(&tree);
        let summary = build_summary(rendered.width, rendered.height, &elements, &overlaps);
        assert!(summary.contains("overlapping pair"));
        assert!(summary.contains('⚠'));
    }
}
