//! MCP server exposing the `generate_pikchr` tool.
//!
//! Note-writing sessions (project notes and local branch notes) use it to
//! author and validate their Pikchr diagrams before shipping them:
//!
//! `generate_pikchr` turns a natural-language description into validated Pikchr
//! by running a focused internal agent sub-session (via
//! [`crate::pikchr_subsession`]) before returning the final source plus a path
//! to a saved preview image. Revisions pass the current diagram's source back
//! in so the sub-agent edits real Pikchr rather than re-describing from
//! scratch.
//! The specialist iterates in its own session through the `render_pikchr` tool
//! served by [`PikchrPreviewHandler`]: each call renders and analyzes a
//! candidate through the internal [`run_preview`] path — the same engine the
//! tool ultimately hands back — returning the rendered image plus a layout
//! report, and recording every successful render in a shared last-render slot.
//! The specialist accepts by ending its turn with the
//! [`crate::pikchr_subsession::ACCEPT_SENTINEL`] token, and the host returns
//! the slot's contents — so unvalidated source can never reach the caller.
//! While the specialist runs, `generate_pikchr` ticks MCP progress
//! notifications back to its caller (when the request carries a progress
//! token) so client-side idle timers don't abort a call whose run outlasts
//! them.
//!
//! Fidelity: rendering goes through the `pikchr` crate, which bundles the same
//! official `pikchr.c` that the frontend's `pikchr-js` compiles to WASM. The
//! shape geometry — the part that matters most for layout analysis — is
//! therefore identical to what the user eventually sees. Overlap and
//! out-of-bounds detection then read the shape and text rectangles straight off
//! the `usvg` tree that also rasterizes the preview, so labels are checked with
//! real, shaped extents rather than bare anchor points. `usvg`/`resvg` lay text
//! out with native system fonts, not the frontend's browser metrics, so text
//! extents differ by a hair (and font fallback can differ); label thresholds
//! are kept forgiving to match, and this is acceptable for a preview.
//!
//! Unlike `project_mcp`, this handler touches no project. It carries only the
//! provider id, app handle, shared store, and session registry that
//! `generate_pikchr` needs to spin up, persist, and cancel its sub-session, so
//! it remains safe to attach to any local session.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use acp_client::{McpServer, McpServerHttp};
use axum::Router;
use base64::Engine as _;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{
    CallToolResult, Content, ProgressNotificationParam, ProgressToken, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData, Peer, RoleServer, ServerHandler};

use crate::agent::AcpDriver;
use crate::pikchr_subsession::{CancelReason, GenOutcome, LastRenderSlot, ACCEPT_SENTINEL};
use crate::session_runner::SessionRegistry;
use crate::store::{AcpMessageMetadata, CompletionReason, Session, SessionStatus, Store};

/// Wall-clock cap for one `generate_pikchr` call. Each call spins a provider
/// subprocess and runs several turns; the cap keeps a stuck sub-agent from
/// running indefinitely. Enforced by cancelling the sub-session's token.
const GENERATE_PIKCHR_TIMEOUT: Duration = Duration::from_secs(1200);

/// Interval between MCP progress keep-alives sent to the caller while
/// `generate_pikchr` waits on its specialist run. Without them the whole run
/// is one silent request, and MCP clients cut those off long before
/// [`GENERATE_PIKCHR_TIMEOUT`]: Claude Code aborts any tool call that produces
/// no response or progress notification for 300 s, orphaning a worker that
/// then finishes into the void. 30 s keeps a generous margin under that (and
/// any comparable client-side idle timer) at negligible cost.
const PROGRESS_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

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
const PIKCHR_CHILD_SESSION_PROMPT: &str = "Generate Pikchr diagram";
/// Hidden ACP metadata event written to the *parent* session's transcript the
/// moment `generate_pikchr` creates its child diagram session. The tool result
/// only names the child session once the specialist finishes, so this early
/// announcement is what lets the UI offer "open diagram session" mid-run.
const PIKCHR_SESSION_STARTED_EVENT: &str = "pikchr_session_started";

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

/// One element extending past the diagram's drawing area, by index into the
/// element list. Each field is the distance (px in Pikchr's coordinate space)
/// the element crosses that edge, zeroed for edges it stays within (or crosses
/// by less than the detection threshold).
#[derive(Clone, Debug)]
struct OutOfBounds {
    element: usize,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
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
// Layout analysis (overlaps + bounds)
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

/// Find elements that stick out past the diagram's drawing area.
///
/// Pikchr sizes the canvas from its own text-width estimates, so a label laid
/// out with real fonts can spill past the edge and get clipped when the diagram
/// is displayed — the case this catches. The per-kind thresholds mirror overlap
/// detection: text uses the forgiving [`MIN_TEXT_OVERLAP_PX`] because usvg's
/// font metrics differ by a hair from the frontend's.
fn find_out_of_bounds(elements: &[Element], diagram: &BBox) -> Vec<OutOfBounds> {
    elements
        .iter()
        .enumerate()
        .filter_map(|(i, element)| {
            let threshold = match element.kind {
                ElementKind::Box => MIN_OVERLAP_PX,
                ElementKind::Text => MIN_TEXT_OVERLAP_PX,
            };
            let past = |amount: f64| if amount > threshold { amount } else { 0.0 };
            let oob = OutOfBounds {
                element: i,
                left: past(diagram.min_x - element.bounds.min_x),
                top: past(diagram.min_y - element.bounds.min_y),
                right: past(element.bounds.max_x - diagram.max_x),
                bottom: past(element.bounds.max_y - diagram.max_y),
            };
            (oob.left > 0.0 || oob.top > 0.0 || oob.right > 0.0 || oob.bottom > 0.0).then_some(oob)
        })
        .collect()
}

/// Run the full layout analysis — overlap detection plus out-of-bounds
/// detection against the diagram's drawing area — on a parsed Pikchr SVG tree.
fn analyze_layout(tree: &usvg::Tree) -> (Vec<Element>, Vec<Overlap>, Vec<OutOfBounds>) {
    let mut elements = extract_elements(tree);
    let homes = home_boxes(&elements);
    assign_box_labels(&mut elements, &homes);
    let overlaps = find_overlaps(&elements, &homes);
    // Pikchr emits a 0-origin 1:1 viewBox, so the drawing area in usvg canvas
    // coordinates runs from (0, 0) to the tree's size.
    let diagram = BBox {
        min_x: 0.0,
        min_y: 0.0,
        max_x: tree.size().width() as f64,
        max_y: tree.size().height() as f64,
    };
    let out_of_bounds = find_out_of_bounds(&elements, &diagram);
    (elements, overlaps, out_of_bounds)
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

/// Human-readable description of how far an element crosses each diagram edge,
/// e.g. "≈ 6 px past the right edge".
fn describe_overhang(oob: &OutOfBounds) -> String {
    [
        (oob.left, "left"),
        (oob.top, "top"),
        (oob.right, "right"),
        (oob.bottom, "bottom"),
    ]
    .iter()
    .filter(|(amount, _)| *amount > 0.0)
    .map(|(amount, edge)| format!("≈ {amount:.0} px past the {edge} edge"))
    .collect::<Vec<_>>()
    .join(", ")
}

/// Build the layout-warning portion of the render analysis — overlap and
/// out-of-bounds reports with repair guidance — or `None` when the layout is
/// clean. Text matters because vision-less models can act on a textual layout
/// report.
fn build_warnings(
    elements: &[Element],
    overlaps: &[Overlap],
    out_of_bounds: &[OutOfBounds],
) -> Option<String> {
    if overlaps.is_empty() && out_of_bounds.is_empty() {
        return None;
    }
    let mut out = String::new();

    if !overlaps.is_empty() {
        out.push_str(&format!(
            "⚠ {} overlapping pair(s) detected:",
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
            "\nAdjust the diagram to separate the overlapping shapes/labels — e.g. set an explicit \
flow direction, use named nodes with explicit anchors (`with .w at …`, `arrow from A.e to B.w`), \
give long labels room or shorten them, and avoid percentage-length arrows between `fit` boxes.",
        );
    }

    if !out_of_bounds.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "⚠ {} element(s) extend beyond the diagram bounds:",
            out_of_bounds.len()
        ));
        for oob in out_of_bounds {
            out.push_str(&format!(
                "\n- {} sticks out {}",
                describe_element(&elements[oob.element]),
                describe_overhang(oob)
            ));
        }
        out.push_str(
            "\nContent outside the bounds is clipped when the diagram is displayed. Bring every \
element back inside the drawing area — e.g. shorten the offending labels or widen the boxes \
holding them, keep free-standing text away from the edges, and add canvas margin \
(`margin = 0.25in`) if the content needs breathing room.",
        );
    }
    Some(out)
}

/// Compose the full analysis summary: dimensions plus the warning report (or
/// an all-clear line).
fn build_summary(width: i64, height: i64, warnings: Option<&str>) -> String {
    match warnings {
        None => {
            format!("Rendered Pikchr diagram: {width}×{height} px.\nNo layout issues detected.")
        }
        Some(warnings) => format!("Rendered Pikchr diagram: {width}×{height} px.\n{warnings}"),
    }
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
/// succeeded), a text summary of dimensions and layout warnings, and whether
/// the source failed to render at all. Layout warnings live inside `summary`
/// for the specialist to weigh; they are not carried past acceptance.
pub(crate) struct PreviewOutcome {
    pub(crate) png: Option<Vec<u8>>,
    pub(crate) summary: String,
    pub(crate) is_error: bool,
}

/// Render + analyze a candidate Pikchr source.
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

    // Parse the rendered SVG once; both layout analysis and rasterization read
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

    let (elements, overlaps, out_of_bounds) = analyze_layout(&tree);
    let warnings = build_warnings(&elements, &overlaps, &out_of_bounds);
    let mut summary = build_summary(rendered.width, rendered.height, warnings.as_deref());

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
    /// Session whose transcript hosts the `generate_pikchr` tool calls —
    /// child-session announcements are written into it so the UI can link to
    /// the diagram session while the specialist is still running.
    parent_session_id: String,
    /// Handle used to load the bundled Pikchr grammar text inlined into the
    /// sub-agent's prompt.
    app_handle: tauri::AppHandle,
    /// Shared app store used to persist the child diagram session.
    store: Arc<Store>,
    /// Session registry the child diagram session is registered in while it
    /// runs, so the Stop control in its UI cancels the actual worker instead
    /// of falling back to a bare store write the worker never observes.
    registry: Arc<SessionRegistry>,
    tool_router: ToolRouter<Self>,
}

impl PikchrToolsHandler {
    fn new(
        provider_id: String,
        parent_session_id: String,
        app_handle: tauri::AppHandle,
        store: Arc<Store>,
        registry: Arc<SessionRegistry>,
    ) -> Self {
        Self {
            provider_id,
            parent_session_id,
            app_handle,
            store,
            registry,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl PikchrToolsHandler {
    #[tool(
        description = "Generate a validated Pikchr diagram from a natural-language description. \
An internal Pikchr specialist writes the diagram, then renders and visually reviews it in its own \
session, iterating until it is satisfied. Prefer this over hand-writing Pikchr. Pass a fine-grained \
`description` (boxes, arrows, labels, layout, relationships). To revise an existing diagram, also \
pass its current source as `previous_pikchr` so it is edited rather than redrawn. Returns the \
validated Pikchr source (drop it into a ```pikchr fenced code block) and a filesystem path to a \
rendered PNG preview you may open as an optional final check."
    )]
    async fn generate_pikchr(
        &self,
        Parameters(p): Parameters<GeneratePikchrParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let scale = p.scale.unwrap_or(DEFAULT_SCALE);
        // Resolve the agent/model/effort the diagram sub-session runs under. The
        // user can point it at a specific agent — and that agent's model and
        // effort — via General settings → Diagram generation, distinct from the
        // session that invoked this tool. When unset, fall back to the invoking
        // session's agent (the field this handler was built with) at its
        // default model/effort, reproducing the pre-setting behaviour.
        //
        // An override run additionally carries the invoking session's agent as
        // a fallback (`fallback_provider_id`): the stored preference can drift
        // stale between runs — agent uninstalled, model/effort id dropped by an
        // agent update, agent without the HTTP MCP support the render tool
        // needs — and a stale override degrades to the no-override behaviour
        // instead of failing every call until the setting is fixed. The stored
        // preference stays untouched; the settings UI surfaces its stale state.
        let diagram_config = crate::acp_config::read_diagram_subsession_config();
        let (provider_id, config_options, fallback_provider_id) = match diagram_config.provider_id()
        {
            Some(configured) => (
                configured.to_string(),
                diagram_config.config_options(),
                Some(self.provider_id.clone()),
            ),
            None => (self.provider_id.clone(), Vec::new(), None),
        };
        let session = create_pikchr_child_session(&self.store, &provider_id)
            .map_err(|e| ErrorData::internal_error(e, None))?;
        let inner_session_id = session.id.clone();
        announce_pikchr_child_session(&self.store, &self.parent_session_id, &inner_session_id);
        let store = Arc::clone(&self.store);
        // The full grammar text is inlined into the sub-agent's prompt rather
        // than referenced by file path (the sub-session has no repo access).
        // Remote note sessions keep referencing an uploaded grammar file —
        // that path is resolved separately in `session_commands`. `None`
        // (bundled grammar missing or unreadable) makes the prompt fall back
        // to naming the public grammar URL.
        let grammar = crate::session_commands::bundled_pikchr_grammar_text(&self.app_handle);

        // Cancellation token owned by *this* future (the parent MCP request).
        // The worker gets a clone; the parent keeps the token alive through a
        // `DropGuard`. If the MCP client abandons this tool call, the future is
        // dropped, the guard cancels the token, and the sub-session's provider
        // subprocess is torn down promptly — rather than running detached until
        // the wall-clock timeout. The worker arms this same token on timeout,
        // recording the reason first so the cancelled child session can say
        // "timed out" rather than a bare cancel; a guard-driven cancel records
        // nothing and reads as the caller abandoning the call.
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let worker_cancel_reason = Arc::new(CancelReason::new());
        let _cancel_on_drop = cancel.drop_guard();

        // Register the child session in the SessionRegistry under its own
        // token so the Stop control in the opened diagram session terminates
        // the actual work: `cancel_session` fires the registered token, which
        // the worker forwards onto its own token (recording the reason first)
        // — instead of taking the fallback path that just writes Cancelled to
        // a store row this worker never re-reads. The registration guard
        // deregisters when this call ends, however it ends.
        let registration = self.registry.register_external(&inner_session_id);
        let user_cancel = registration.token().clone();

        // The ACP driver spawns tasks via `spawn_local`, which requires a
        // `LocalSet`; the MCP server's request tasks don't run inside one. So
        // drive the whole generation loop on a dedicated thread with its own
        // current-thread runtime + LocalSet, mirroring `session_runner`.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let worker_store = Arc::clone(&store);
        let worker_session_id = inner_session_id.clone();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let message = format!("Failed to create runtime for generate_pikchr: {e}");
                    mark_pikchr_child_session_error(&worker_store, &worker_session_id, &message);
                    let _ = tx.send(Err(message));
                    return;
                }
            };
            let local = tokio::task::LocalSet::new();
            let result = local.block_on(&rt, async move {
                // The last-render slot the specialist's `render_pikchr` tool
                // writes and the host loop takes from on acceptance. The
                // preview server's handle drops with this worker's runtime.
                let slot = Arc::new(LastRenderSlot::new());
                let (preview_port, _preview_server) =
                    match start_pikchr_preview_mcp_server(scale, Arc::clone(&slot)).await {
                        Ok(started) => started,
                        Err(e) => {
                            mark_pikchr_child_session_error(&worker_store, &worker_session_id, &e);
                            return Err(e);
                        }
                    };
                let preview_mcp_server = || {
                    vec![McpServer::Http(McpServerHttp::new(
                        "pikchr-preview",
                        format!("http://127.0.0.1:{preview_port}/mcp"),
                    ))]
                };
                // Providers without HTTP MCP support fail the required-
                // transport check inside the run. For a no-override run that
                // errors the call — acceptable, since the parent session
                // already requires an MCP-capable provider to have
                // `generate_pikchr` at all. An override run instead falls back
                // to that same invoking agent (below), which the same argument
                // makes a safe harbour.
                let mut config_options = config_options;
                let mut fallback_provider_id = fallback_provider_id;
                let driver = match AcpDriver::new(&provider_id) {
                    Ok(driver) => driver,
                    // The configured diagram agent no longer resolves (e.g.
                    // uninstalled since it was chosen): fall back to the
                    // invoking session's agent up front rather than after a
                    // doomed run.
                    Err(e) => match fallback_provider_id.take() {
                        Some(parent_provider) => match AcpDriver::new(&parent_provider) {
                            Ok(driver) => {
                                log::warn!(
                                    "[pikchr_mcp] configured diagram agent unavailable ({e}); \
falling back to the invoking session's agent"
                                );
                                if let Err(store_error) = worker_store
                                    .set_session_provider(&worker_session_id, &parent_provider)
                                {
                                    log::warn!(
                                        "[pikchr_mcp] failed to move Pikchr session \
{worker_session_id} to the fallback provider: {store_error}"
                                    );
                                }
                                config_options = Vec::new();
                                driver
                            }
                            Err(parent_error) => {
                                mark_pikchr_child_session_error(
                                    &worker_store,
                                    &worker_session_id,
                                    &parent_error,
                                );
                                return Err(parent_error);
                            }
                        },
                        None => {
                            mark_pikchr_child_session_error(&worker_store, &worker_session_id, &e);
                            return Err(e);
                        }
                    },
                };
                let driver = driver.with_mcp_servers(preview_mcp_server());
                // The fallback driver for override failures that only surface
                // inside the run (stale model/effort selection, unsupported
                // MCP transport) — see generate_pikchr_source. An invoking
                // agent that itself doesn't resolve simply leaves those
                // failures as the hard errors they were.
                let fallback_driver = fallback_provider_id.as_deref().and_then(|parent| {
                    match AcpDriver::new(parent) {
                        Ok(driver) => Some(driver.with_mcp_servers(preview_mcp_server())),
                        Err(e) => {
                            log::warn!(
                                "[pikchr_mcp] invoking session's agent unavailable as the \
diagram fallback: {e}"
                            );
                            None
                        }
                    }
                });
                let fallback = fallback_driver
                    .as_ref()
                    .zip(fallback_provider_id.as_deref())
                    .map(
                        |(driver, provider_id)| crate::pikchr_subsession::DiagramFallback {
                            driver,
                            provider_id,
                        },
                    );
                // Enforce the wall-clock cap by cancelling the sub-session's
                // token; the driver shuts its subprocess down gracefully. The
                // parent's `DropGuard` cancels this same token if the MCP
                // client abandons the call before the timeout fires.
                let timeout_cancel = worker_cancel.clone();
                let timeout_reason = Arc::clone(&worker_cancel_reason);
                tokio::task::spawn_local(async move {
                    tokio::time::sleep(GENERATE_PIKCHR_TIMEOUT).await;
                    timeout_reason.record(format!(
                        "generate_pikchr hit its {}-minute time limit before the specialist \
accepted a render, so the diagram run was cancelled.",
                        GENERATE_PIKCHR_TIMEOUT.as_secs() / 60
                    ));
                    timeout_cancel.cancel();
                });
                // A Stop pressed in the child diagram session fires the token
                // registered in the SessionRegistry; forward it to the
                // worker's token so the run actually terminates. Both watcher
                // tasks are dropped with this LocalSet.
                tokio::task::spawn_local(forward_user_cancel(
                    user_cancel,
                    Arc::clone(&worker_cancel_reason),
                    worker_cancel.clone(),
                ));
                crate::pikchr_subsession::generate_pikchr_source(
                    &driver,
                    worker_store,
                    &worker_session_id,
                    grammar.as_deref(),
                    &p.description,
                    p.previous_pikchr.as_deref(),
                    &config_options,
                    fallback,
                    &slot,
                    &worker_cancel,
                    &worker_cancel_reason,
                )
                .await
            });
            let _ = tx.send(result);
        });

        // Await the worker while ticking progress keep-alives back to the
        // caller so its idle timer doesn't sever a long run. The keep-alive
        // loop never completes; the select ends when the worker reports (or
        // this future is dropped, which also stops the keep-alives).
        let received = tokio::select! {
            received = rx => received,
            _ = send_progress_keepalives(&ctx.peer, ctx.meta.get_progress_token()) => {
                unreachable!("the progress keep-alive loop never completes")
            }
        };
        let outcome = received
            .map_err(|e| {
                let message = format!("generate_pikchr worker dropped: {e}");
                mark_pikchr_child_session_error(&store, &inner_session_id, &message);
                ErrorData::internal_error(message, None)
            })?
            .map_err(|e| ErrorData::internal_error(e, None))?;

        let preview_image_path = if let Some(png) = &outcome.png {
            // A temp-file failure here is the parent's bookkeeping problem, not
            // the specialist's: the sub-session already succeeded and recorded
            // its own terminal status, so leave that intact and let this tool
            // result's error explain the failure to the caller.
            let path = write_png_to_temp_file(png).map_err(|e| {
                ErrorData::internal_error(
                    format!("Failed to write Pikchr preview image to temp dir: {e}"),
                    None,
                )
            })?;
            Some(path.display().to_string())
        } else {
            None
        };

        Ok(build_generate_pikchr_result(
            &inner_session_id,
            preview_image_path.as_deref(),
            &outcome.source,
        ))
    }
}

/// Build the progress keep-alive notification sent `elapsed_secs` into a
/// `generate_pikchr` run. Progress reports elapsed seconds with no total:
/// monotonically increasing, as the spec asks of an unbounded operation.
fn progress_keepalive(
    progress_token: ProgressToken,
    elapsed_secs: u64,
) -> ProgressNotificationParam {
    ProgressNotificationParam {
        progress_token,
        progress: elapsed_secs as f64,
        total: None,
        message: Some(format!(
            "Diagram specialist still working ({elapsed_secs}s elapsed)."
        )),
    }
}

/// Tick an MCP progress notification to the caller every
/// [`PROGRESS_KEEPALIVE_INTERVAL`] for as long as this future is polled.
/// Never completes — run it under `select!` against the awaited work so it
/// stops when the work does. Progress notifications may only reference a
/// token the caller provided, so when the request carries none this pends
/// forever (rather than returning, which the caller treats as unreachable)
/// and the call proceeds without keep-alives.
async fn send_progress_keepalives(peer: &Peer<RoleServer>, progress_token: Option<ProgressToken>) {
    let Some(progress_token) = progress_token else {
        return std::future::pending().await;
    };
    let started = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(PROGRESS_KEEPALIVE_INTERVAL).await;
        let elapsed_secs = started.elapsed().as_secs();
        if let Err(e) = peer
            .notify_progress(progress_keepalive(progress_token.clone(), elapsed_secs))
            .await
        {
            // A failed keep-alive usually means the caller is gone; the
            // worker result (or this future being dropped) settles the call.
            log::debug!("[pikchr_mcp] failed to send generate_pikchr progress keep-alive: {e}");
        }
    }
}

/// Cancel reason recorded when the user stops the child diagram session from
/// its own UI, distinguishing a deliberate stop from caller abandonment on
/// both the cancelled session row and the parent tool error.
const USER_STOP_CANCEL_MESSAGE: &str =
    "The diagram session was stopped before the specialist accepted a render, so the \
generate_pikchr call was cancelled.";

/// Wait for a user Stop on the child diagram session — `cancel_session` fires
/// `user_cancel`, the token registered in the SessionRegistry — and forward it
/// to the worker's own token, recording the reason first so the cancelled
/// session and the parent tool error read as a deliberate stop.
async fn forward_user_cancel(
    user_cancel: CancellationToken,
    reason: Arc<CancelReason>,
    worker_cancel: CancellationToken,
) {
    user_cancel.cancelled().await;
    reason.record(USER_STOP_CANCEL_MESSAGE.to_string());
    worker_cancel.cancel();
}

fn create_pikchr_child_session(store: &Store, provider_id: &str) -> Result<Session, String> {
    let mut session = Session::new_running(PIKCHR_CHILD_SESSION_PROMPT, &std::env::temp_dir());
    if !provider_id.is_empty() {
        session = session.with_provider(provider_id);
    }
    store
        .create_session(&session)
        .map_err(|e| format!("Failed to create Pikchr child session: {e}"))?;
    Ok(session)
}

/// Write a hidden metadata row into the parent session's transcript naming the
/// just-created child diagram session, so the UI can attach an "open diagram
/// session" button to the running `generate_pikchr` tool card. A successful
/// tool result remains the authoritative id source once the call completes; a
/// failed call carries no id in its result, so this announcement is also what
/// keeps the diagram session (which records the failure) reachable from the
/// failed tool card. A write failure here is non-fatal — the button is simply
/// missing until (and unless) the call completes successfully.
fn announce_pikchr_child_session(store: &Store, parent_session_id: &str, child_session_id: &str) {
    let metadata = AcpMessageMetadata {
        acp_event_kind: Some(PIKCHR_SESSION_STARTED_EVENT.to_string()),
        acp_content: Some(serde_json::json!({ "innerSessionId": child_session_id })),
        ..Default::default()
    };
    if let Err(e) = store.add_acp_metadata_message(parent_session_id, &metadata) {
        log::warn!(
            "Failed to announce Pikchr child session {child_session_id} in parent \
             session {parent_session_id}: {e}"
        );
    }
}

fn mark_pikchr_child_session_error(store: &Store, session_id: &str, message: &str) {
    // `transition_from_running` so a status the child session already reached
    // — a concurrent user cancel, or the terminal status recorded by
    // `generate_pikchr_source` — is not clobbered by this bookkeeping write.
    if let Err(e) = store.transition_from_running(
        session_id,
        SessionStatus::Error,
        Some(message),
        Some(&CompletionReason::Crashed),
    ) {
        log::error!("Failed to mark Pikchr child session {session_id} errored: {e}");
    }
}

fn build_generate_pikchr_result(
    inner_session_id: &str,
    preview_image_path: Option<&str>,
    source: &str,
) -> CallToolResult {
    let mut content = Vec::new();
    if let Some(path) = preview_image_path {
        content.push(Content::text(format!(
            "Rendered preview image path: {path}"
        )));
    }
    content.push(Content::text(source.to_string()));

    let mut structured = serde_json::Map::new();
    structured.insert(
        "innerSessionId".to_string(),
        serde_json::Value::String(inner_session_id.to_string()),
    );
    if let Some(path) = preview_image_path {
        structured.insert(
            "previewImagePath".to_string(),
            serde_json::Value::String(path.to_string()),
        );
    }
    structured.insert(
        "source".to_string(),
        serde_json::Value::String(source.to_string()),
    );

    let mut result = CallToolResult::success(content);
    result.structured_content = Some(serde_json::Value::Object(structured));
    result
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
/// fails per-call rather than failing session startup). `parent_session_id` is
/// the session this server is attached to; child diagram sessions are
/// announced into its transcript as they start. `app_handle` loads the
/// bundled Pikchr grammar text for the sub-agent. `registry` holds each
/// child diagram session while it runs so a user Stop reaches its worker.
pub async fn start_pikchr_mcp_server(
    provider_id: String,
    parent_session_id: String,
    app_handle: tauri::AppHandle,
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
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
                parent_session_id.clone(),
                app_handle.clone(),
                Arc::clone(&store),
                Arc::clone(&registry),
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

// =============================================================================
// render_pikchr — the specialist sub-session's preview tool
// =============================================================================

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct RenderPikchrParams {
    /// The candidate Pikchr source to render, without code fences.
    pub pikchr: String,
}

/// Handler for the specialist sub-session's `render_pikchr` tool. Separate
/// from [`PikchrToolsHandler`] so the sub-session sees only this tool and
/// cannot recurse into `generate_pikchr`. Every connection shares the parent
/// call's rasterization scale and last-render slot.
#[derive(Clone)]
struct PikchrPreviewHandler {
    scale: f32,
    slot: Arc<LastRenderSlot>,
    tool_router: ToolRouter<Self>,
}

impl PikchrPreviewHandler {
    fn new(scale: f32, slot: Arc<LastRenderSlot>) -> Self {
        Self {
            scale,
            slot,
            tool_router: Self::tool_router(),
        }
    }
}

/// Standing instruction ending every successful `render_pikchr` result.
fn accept_instruction() -> String {
    format!(
        "When you are satisfied with this render, accept it by ending your message with \
`{ACCEPT_SENTINEL}` as its own final line. Otherwise revise the source and render again."
    )
}

#[tool_router]
impl PikchrPreviewHandler {
    #[tool(
        description = "Render candidate Pikchr source and inspect the result. Returns the rendered \
image plus a layout analysis (dimensions, overlapping elements, content extending beyond the \
diagram bounds). Each successful render replaces the previous one as the current candidate; \
ending your message with `AcceptLastRender` as its own final line accepts the most recent \
successful render."
    )]
    async fn render_pikchr(
        &self,
        Parameters(p): Parameters<RenderPikchrParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let preview = run_preview(&p.pikchr, self.scale);
        if preview.is_error {
            // The slot keeps the previous successful render, so acceptance
            // after a failed attempt is informed: say what it would accept.
            let slot_note = if self.slot.is_empty() {
                "No successful render is stored yet — fix the source and render again."
            } else {
                "The previous successful render is still stored; accepting with `AcceptLastRender` \
now would accept that earlier version, not this source."
            };
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "{}\n{slot_note}",
                preview.summary
            ))]));
        }

        let mut content = vec![Content::text(preview.summary)];
        if let Some(png) = &preview.png {
            content.push(Content::image(
                base64::engine::general_purpose::STANDARD.encode(png),
                "image/png",
            ));
        }
        content.push(Content::text(accept_instruction()));

        self.slot.store(GenOutcome {
            source: p.pikchr,
            png: preview.png,
        });

        Ok(CallToolResult::success(content))
    }
}

#[tool_handler]
impl ServerHandler for PikchrPreviewHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

/// Start a local MCP HTTP server exposing the `render_pikchr` tool for one
/// `generate_pikchr` call's sub-session.
///
/// Returns the bound port and a `JoinHandle`. The server lives as long as the
/// worker thread's runtime that spawned it; the caller keeps the handle for
/// the duration of the call and both drop with the worker. All connections
/// share `scale` and `slot`, so the host loop reads the same last-render slot
/// the tool writes.
async fn start_pikchr_preview_mcp_server(
    scale: f32,
    slot: Arc<LastRenderSlot>,
) -> Result<(u16, JoinHandle<()>), String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind pikchr preview MCP listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get local address: {e}"))?
        .port();

    let service = StreamableHttpService::new(
        move || Ok(PikchrPreviewHandler::new(scale, Arc::clone(&slot))),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let router = Router::new().route_service("/mcp", service);

    log::debug!("[pikchr_mcp] preview HTTP server bound on port {port}");

    let handle = tokio::task::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            log::error!("[pikchr_mcp] preview HTTP server error: {e}");
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

    #[test]
    fn create_pikchr_child_session_persists_running_provider_session() {
        let store = Store::in_memory().expect("in-memory store");

        let session =
            create_pikchr_child_session(&store, "fake-agent").expect("create child session");

        assert_eq!(session.prompt, PIKCHR_CHILD_SESSION_PROMPT);
        assert_eq!(session.status, SessionStatus::Running);
        assert_eq!(session.provider.as_deref(), Some("fake-agent"));
        assert!(!session.working_dir.is_empty());

        let persisted = store
            .get_session(&session.id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(persisted.prompt, PIKCHR_CHILD_SESSION_PROMPT);
        assert_eq!(persisted.status, SessionStatus::Running);
        assert_eq!(persisted.provider.as_deref(), Some("fake-agent"));
    }

    #[test]
    fn progress_keepalive_reports_elapsed_seconds_with_no_total() {
        let token = ProgressToken(rmcp::model::NumberOrString::Number(7));

        let notification = progress_keepalive(token.clone(), 90);

        assert_eq!(notification.progress_token, token);
        // Elapsed seconds as the progress value keeps successive keep-alives
        // monotonically increasing, and an unbounded run reports no total.
        assert_eq!(notification.progress, 90.0);
        assert_eq!(notification.total, None);
        assert!(notification.message.expect("message").contains("90s"));
    }

    #[tokio::test]
    async fn forward_user_cancel_records_reason_and_arms_worker_token() {
        let user_cancel = CancellationToken::new();
        let reason = Arc::new(CancelReason::new());
        let worker_cancel = CancellationToken::new();
        user_cancel.cancel();

        forward_user_cancel(user_cancel, Arc::clone(&reason), worker_cancel.clone()).await;

        assert!(worker_cancel.is_cancelled());
        assert_eq!(reason.resolve(), USER_STOP_CANCEL_MESSAGE);
    }

    /// Stands in for a live specialist turn during which the user presses Stop
    /// in the child diagram session's UI: `cancel_session` fires the token
    /// registered for the session, and the driver — like a real one — winds
    /// down once its own cancellation token is armed.
    struct RegistryStopDriver {
        registry: Arc<SessionRegistry>,
    }

    #[async_trait::async_trait(?Send)]
    impl crate::agent::AgentDriver for RegistryStopDriver {
        async fn run(
            &self,
            session_id: &str,
            _prompt: &str,
            _images: &[(String, String)],
            _working_dir: &std::path::Path,
            _store: &Arc<dyn acp_client::Store>,
            _writer: &Arc<dyn acp_client::MessageWriter>,
            cancel_token: &CancellationToken,
            _agent_session_id: Option<&str>,
            _config_options: &[acp_client::AcpSessionConfigOptionSelection],
        ) -> Result<acp_client::AgentRunOutcome, String> {
            assert!(
                self.registry.cancel(session_id),
                "the child session should be registered while the worker runs"
            );
            cancel_token.cancelled().await;
            Ok(acp_client::AgentRunOutcome::Cancelled)
        }
    }

    /// A Stop pressed in the child diagram session mid-run must terminate the
    /// specialist and read as a deliberate stop — not caller abandonment — on
    /// both the session row and the tool error.
    #[tokio::test]
    async fn registry_stop_terminates_the_run_and_reads_as_a_user_stop() {
        let registry = Arc::new(SessionRegistry::new());
        let store = Arc::new(Store::in_memory().expect("in-memory store"));
        let session =
            create_pikchr_child_session(&store, "fake-agent").expect("create child session");

        let registration = registry.register_external(&session.id);
        let worker_cancel = CancellationToken::new();
        let reason = Arc::new(CancelReason::new());
        let driver = RegistryStopDriver {
            registry: Arc::clone(&registry),
        };
        let slot = LastRenderSlot::new();

        let local = tokio::task::LocalSet::new();
        let result = local
            .run_until(async {
                tokio::task::spawn_local(forward_user_cancel(
                    registration.token().clone(),
                    Arc::clone(&reason),
                    worker_cancel.clone(),
                ));
                crate::pikchr_subsession::generate_pikchr_source(
                    &driver,
                    Arc::clone(&store),
                    &session.id,
                    Some("test grammar body"),
                    "a friendly box",
                    None,
                    &[],
                    None,
                    &slot,
                    &worker_cancel,
                    &reason,
                )
                .await
            })
            .await;

        assert_eq!(result.err().as_deref(), Some(USER_STOP_CANCEL_MESSAGE));

        let persisted = store
            .get_session(&session.id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(persisted.status, SessionStatus::Cancelled);
        assert_eq!(
            persisted.error_message.as_deref(),
            Some(USER_STOP_CANCEL_MESSAGE)
        );
        assert_eq!(
            persisted.completion_reason.as_ref(),
            Some(&CompletionReason::Interrupted)
        );
    }

    #[test]
    fn announce_pikchr_child_session_writes_hidden_parent_metadata_row() {
        let store = Store::in_memory().expect("in-memory store");
        let parent = Session::new_running("parent prompt", &std::env::temp_dir());
        store
            .create_session(&parent)
            .expect("create parent session");

        announce_pikchr_child_session(&store, &parent.id, "child-session-1");

        let rows = store
            .get_session_acp_metadata_messages(&parent.id)
            .expect("load metadata rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].acp.acp_event_kind.as_deref(),
            Some(PIKCHR_SESSION_STARTED_EVENT)
        );
        assert_eq!(
            rows[0].acp.acp_content.as_ref().expect("content")["innerSessionId"],
            "child-session-1"
        );
        assert_eq!(
            rows[0].content, "",
            "announcement rows must stay hidden from the visible transcript"
        );
    }

    #[test]
    fn generate_pikchr_result_preserves_text_and_adds_structured_session_metadata() {
        let result = build_generate_pikchr_result(
            "child-session-1",
            Some("/tmp/staged-pikchr-preview.png"),
            "box \"Clean\" fit",
        );

        let texts: Vec<String> = result
            .content
            .iter()
            .filter_map(|content| content.as_text().map(|text| text.text.clone()))
            .collect();
        assert_eq!(
            texts,
            vec![
                "Rendered preview image path: /tmp/staged-pikchr-preview.png".to_string(),
                "box \"Clean\" fit".to_string(),
            ]
        );

        let structured = result
            .structured_content
            .as_ref()
            .expect("structured content");
        assert_eq!(structured["innerSessionId"], "child-session-1");
        assert_eq!(
            structured["previewImagePath"],
            "/tmp/staged-pikchr-preview.png"
        );
        assert_eq!(structured["source"], "box \"Clean\" fit");
    }

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
        let (elements, _, _) = analyze_layout(&tree);
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
        let (elements, _, _) = analyze_layout(&tree);
        assert_eq!(count_boxes(&elements), 2, "expected exactly two boxes");
    }

    #[test]
    fn overlap_detector_flags_known_overlapping_source() {
        let tree = tree_for(OVERLAPPING_SOURCE);
        let (elements, overlaps, _) = analyze_layout(&tree);
        assert_eq!(count_boxes(&elements), 5, "expected five boxes");
        assert!(
            !overlaps.is_empty(),
            "expected overlaps for the cascade diagram, found none"
        );
    }

    #[test]
    fn overlap_detector_passes_corrected_source() {
        let tree = tree_for(CORRECTED_SOURCE);
        let (_elements, overlaps, _) = analyze_layout(&tree);
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
        assert!(outcome.summary.contains("No layout issues detected"));
        assert!(outcome.png.is_some(), "expected a PNG for valid source");
        assert!(!outcome.png.unwrap().is_empty());
        assert!(outcome.summary.contains("px"));
    }

    #[test]
    fn overlapping_source_reports_warnings() {
        let outcome = run_preview(OVERLAPPING_SOURCE, DEFAULT_SCALE);
        assert!(!outcome.is_error);
        assert!(outcome.summary.contains('⚠'));
        assert!(outcome.summary.contains("overlapping pair"));
    }

    #[test]
    fn out_of_bounds_source_reports_warnings() {
        // A negative margin shrinks Pikchr's computed canvas below its content,
        // so the box geometry (font-independent, unlike spilling text) crosses
        // the diagram edges on every host.
        let outcome = run_preview("margin = -0.2in\nbox \"Out\"", DEFAULT_SCALE);
        assert!(!outcome.is_error);
        assert!(outcome.summary.contains('⚠'));
        assert!(outcome.summary.contains("beyond the diagram bounds"));
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
        let (elements, overlaps, out_of_bounds) = analyze_layout(&tree);
        let warnings = build_warnings(&elements, &overlaps, &out_of_bounds);
        let summary = build_summary(rendered.width, rendered.height, warnings.as_deref());
        assert!(summary.contains("overlapping pair"));
        assert!(summary.contains('⚠'));
    }

    #[test]
    fn element_past_diagram_edge_is_flagged() {
        let diagram = BBox {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 100.0,
            max_y: 50.0,
        };
        let elements = vec![
            element(ElementKind::Box, "inside", 5.0, 5.0, 95.0, 45.0),
            element(ElementKind::Text, "spilling label", 60.0, 10.0, 110.0, 24.0),
        ];
        let oob = find_out_of_bounds(&elements, &diagram);
        assert_eq!(oob.len(), 1, "only the spilling label is out: {oob:?}");
        assert_eq!(oob[0].element, 1);
        assert!(
            (oob[0].right - 10.0).abs() < 1e-6,
            "right overhang was {}",
            oob[0].right
        );
        assert_eq!(oob[0].left, 0.0);
        assert_eq!(oob[0].top, 0.0);
        assert_eq!(oob[0].bottom, 0.0);
    }

    #[test]
    fn hairline_overhang_is_not_flagged() {
        let diagram = BBox {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 100.0,
            max_y: 50.0,
        };
        // A label 2 px past the edge is within text-metric noise, and a box
        // half a pixel past is a rounding artifact — neither is a real spill.
        let elements = vec![
            element(ElementKind::Text, "nearly out", 60.0, 10.0, 102.0, 24.0),
            element(ElementKind::Box, "", -0.5, 5.0, 95.0, 45.0),
        ];
        let oob = find_out_of_bounds(&elements, &diagram);
        assert!(oob.is_empty(), "hairline overhangs are noise: {oob:?}");
    }

    #[test]
    fn summary_reports_out_of_bounds_elements() {
        let diagram = BBox {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 100.0,
            max_y: 50.0,
        };
        let elements = vec![element(
            ElementKind::Text,
            "wide label",
            60.0,
            10.0,
            130.0,
            24.0,
        )];
        let oob = find_out_of_bounds(&elements, &diagram);
        let warnings = build_warnings(&elements, &[], &oob);
        let summary = build_summary(100, 50, warnings.as_deref());
        assert!(summary.contains('⚠'));
        assert!(summary.contains("beyond the diagram bounds"));
        assert!(summary.contains("label \"wide label\""));
        assert!(summary.contains("right edge"));
    }

    // -------------------------------------------------------------------------
    // render_pikchr tool (the specialist sub-session's preview tool)
    // -------------------------------------------------------------------------

    async fn call_render(handler: &PikchrPreviewHandler, pikchr: &str) -> CallToolResult {
        handler
            .render_pikchr(Parameters(RenderPikchrParams {
                pikchr: pikchr.to_string(),
            }))
            .await
            .expect("render_pikchr should not fail at the protocol level")
    }

    fn result_texts(result: &CallToolResult) -> Vec<String> {
        result
            .content
            .iter()
            .filter_map(|content| content.as_text().map(|text| text.text.clone()))
            .collect()
    }

    #[tokio::test]
    async fn render_tool_returns_summary_image_and_fills_slot() {
        let slot = Arc::new(LastRenderSlot::new());
        let handler = PikchrPreviewHandler::new(DEFAULT_SCALE, Arc::clone(&slot));

        let result = call_render(&handler, "box \"hello\"").await;

        assert_ne!(result.is_error, Some(true));
        let texts = result_texts(&result);
        assert!(texts[0].contains("Rendered Pikchr diagram"));
        assert!(
            texts.last().unwrap().contains(ACCEPT_SENTINEL),
            "the result ends with the acceptance instruction"
        );
        let images: Vec<_> = result
            .content
            .iter()
            .filter_map(|content| content.as_image())
            .collect();
        assert_eq!(images.len(), 1, "one rendered PNG");
        assert_eq!(images[0].mime_type, "image/png");
        assert!(!images[0].data.is_empty());

        let stored = slot.take().expect("slot holds the render");
        assert_eq!(stored.source, "box \"hello\"");
        assert!(stored.png.is_some());
    }

    #[tokio::test]
    async fn render_tool_parse_failure_reports_error_and_keeps_prior_render() {
        let slot = Arc::new(LastRenderSlot::new());
        let handler = PikchrPreviewHandler::new(DEFAULT_SCALE, Arc::clone(&slot));

        call_render(&handler, "box \"first\"").await;
        let result = call_render(&handler, "box \"unterminated").await;

        assert_eq!(result.is_error, Some(true));
        let texts = result_texts(&result);
        assert!(texts[0].contains("Pikchr could not render"));
        assert!(texts[0].contains("previous successful render is still stored"));

        let stored = slot.take().expect("slot keeps the earlier render");
        assert_eq!(stored.source, "box \"first\"");
    }

    #[tokio::test]
    async fn render_tool_parse_failure_with_empty_slot_says_so() {
        let slot = Arc::new(LastRenderSlot::new());
        let handler = PikchrPreviewHandler::new(DEFAULT_SCALE, Arc::clone(&slot));

        let result = call_render(&handler, "box \"unterminated").await;

        assert_eq!(result.is_error, Some(true));
        assert!(result_texts(&result)[0].contains("No successful render is stored yet"));
        assert!(slot.is_empty());
    }

    #[tokio::test]
    async fn render_tool_second_success_overwrites_slot() {
        let slot = Arc::new(LastRenderSlot::new());
        let handler = PikchrPreviewHandler::new(DEFAULT_SCALE, Arc::clone(&slot));

        call_render(&handler, "box \"first\"").await;
        call_render(&handler, "box \"second\"").await;

        let stored = slot.take().expect("slot holds the latest render");
        assert_eq!(stored.source, "box \"second\"");
    }

    #[tokio::test]
    async fn render_tool_stores_overlapping_render_despite_warnings() {
        let slot = Arc::new(LastRenderSlot::new());
        let handler = PikchrPreviewHandler::new(DEFAULT_SCALE, Arc::clone(&slot));

        let result = call_render(&handler, OVERLAPPING_SOURCE).await;

        // Warnings don't gate the slot: the render succeeds and lands, with
        // the report visible in the summary for the specialist to weigh.
        assert_ne!(result.is_error, Some(true));
        assert!(result_texts(&result)[0].contains("overlapping pair"));

        let stored = slot.take().expect("slot holds the flagged render");
        assert_eq!(stored.source, OVERLAPPING_SOURCE);
    }
}
