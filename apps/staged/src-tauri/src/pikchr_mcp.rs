//! Stateless MCP server exposing a single `preview_pikchr` tool.
//!
//! Note-writing sessions (project notes and local branch notes) use this to
//! *see* their Pikchr diagrams before shipping them: the tool renders the
//! supplied source to a PNG and reports any box overlaps it detects, so the
//! agent can catch bad layouts (overlapping boxes, colliding labels) and
//! iterate instead of silently shipping a broken diagram.
//!
//! Fidelity: rendering goes through the `pikchr` crate, which bundles the same
//! official `pikchr.c` that the frontend's `pikchr-js` compiles to WASM. The
//! geometry — the part that matters for overlap detection — is therefore
//! identical to what the user eventually sees. Native rasterization via
//! `resvg` won't reproduce the frontend's side-label gap transform or browser
//! font metrics exactly, so label spacing may differ by a hair; that is
//! acceptable for a preview.
//!
//! Unlike `project_mcp`, this handler holds no state: it never touches the
//! store, registry, or project, so it is safe to attach to any local session.

use std::sync::{Arc, OnceLock};
use tokio::task::JoinHandle;

use axum::Router;
use base64::Engine;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData, ServerHandler};

/// Frontend parity: reject oversized source like `pikchrRendering.ts`
/// (`MAX_PIKCHR_SOURCE_LENGTH`).
const MAX_PIKCHR_SOURCE_LENGTH: usize = 20_000;
/// Cap the rasterized PNG so a runaway diagram can't allocate a huge pixmap.
const MAX_RENDER_DIMENSION: u32 = 4096;
/// Default rasterization scale — 2× keeps labels legible.
const DEFAULT_SCALE: f32 = 2.0;
const MIN_SCALE: f32 = 0.5;
const MAX_SCALE: f32 = 4.0;
/// Ignore overlaps thinner than this (px in Pikchr's coordinate space) so that
/// shapes which merely share an edge or corner aren't flagged.
const MIN_OVERLAP_PX: f64 = 1.0;
/// Truncate derived shape labels in the overlap summary.
const MAX_LABEL_CHARS: usize = 48;

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct PreviewPikchrParams {
    /// The Pikchr source to render — the contents of a ```pikchr fenced code
    /// block, without the fences.
    pub source: String,
    /// Rasterization scale for the returned PNG. Higher values produce a
    /// larger image with more legible labels. Defaults to 2.0; clamped to
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
    fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    fn height(&self) -> f64 {
        self.max_y - self.min_y
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

/// A box-like shape and the label text rendered inside it (if any).
#[derive(Clone, Debug)]
struct LabeledShape {
    bounds: BBox,
    label: Option<String>,
}

/// One detected overlap between two box-like shapes.
#[derive(Clone, Debug)]
struct Overlap {
    a: usize,
    b: usize,
    overlap_w: f64,
    overlap_h: f64,
}

/// A `<text>` element with its anchor point and rendered content.
#[derive(Clone, Debug)]
struct SvgLabel {
    x: f64,
    y: f64,
    text: String,
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

/// Split an SVG path `d` attribute into command letters and numbers.
enum PathToken {
    Cmd(char),
    Num(f64),
}

fn tokenize_path(d: &str) -> Vec<PathToken> {
    let bytes = d.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_alphabetic() {
            tokens.push(PathToken::Cmd(c));
            i += 1;
        } else if c.is_ascii_digit() || c == '+' || c == '-' || c == '.' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                let n = bytes[i] as char;
                if n.is_ascii_digit() || n == '.' {
                    i += 1;
                } else if (n == '+' || n == '-') && matches!(bytes[i - 1] as char, 'e' | 'E') {
                    // sign of an exponent
                    i += 1;
                } else if n == 'e' || n == 'E' {
                    i += 1;
                } else {
                    break;
                }
            }
            if let Ok(num) = d[start..i].parse::<f64>() {
                tokens.push(PathToken::Num(num));
            }
        } else {
            // whitespace, comma, or other separator
            i += 1;
        }
    }
    tokens
}

/// Compute the bounding box of an SVG path `d` string.
///
/// Pikchr emits absolute `M`, `L`, `A` (arc) and `Z` commands. Only segment
/// endpoints contribute to the box; arc radii and flags do not, so those are
/// skipped. Control points of any (unexpected) curve commands are included,
/// which only ever *over*-estimates the box — safe for overlap detection.
fn path_bounds(d: &str) -> Option<BBox> {
    let tokens = tokenize_path(d);
    let mut bx = BBox {
        min_x: f64::MAX,
        min_y: f64::MAX,
        max_x: f64::MIN,
        max_y: f64::MIN,
    };
    let mut any = false;
    let mut add = |x: f64, y: f64| {
        bx.min_x = bx.min_x.min(x);
        bx.min_y = bx.min_y.min(y);
        bx.max_x = bx.max_x.max(x);
        bx.max_y = bx.max_y.max(y);
        any = true;
    };

    let mut i = 0;
    let mut cmd = ' ';
    while i < tokens.len() {
        match tokens[i] {
            PathToken::Cmd(c) => {
                cmd = c;
                i += 1;
            }
            PathToken::Num(_) => {
                // Collect the run of numbers following the current command.
                let mut nums = Vec::new();
                while i < tokens.len() {
                    if let PathToken::Num(n) = tokens[i] {
                        nums.push(n);
                        i += 1;
                    } else {
                        break;
                    }
                }
                match cmd.to_ascii_uppercase() {
                    'A' => {
                        // groups of 7: (rx ry rot large sweep x y) — endpoint only
                        for g in nums.chunks(7) {
                            if g.len() == 7 {
                                add(g[5], g[6]);
                            }
                        }
                    }
                    'Z' => {}
                    // M, L and any other command: treat as coordinate pairs.
                    _ => {
                        for pair in nums.chunks(2) {
                            if pair.len() == 2 {
                                add(pair[0], pair[1]);
                            }
                        }
                    }
                }
            }
        }
    }

    if any {
        Some(bx)
    } else {
        None
    }
}

/// Extract closed box-like shapes from a Pikchr SVG.
///
/// Boxes/ovals render as *closed* `<path>` elements (the `d` ends in `Z`);
/// arrow shafts are open paths and arrowheads are `<polygon>`, so both are
/// naturally excluded. Plain `<rect>` elements are included too.
fn extract_shapes(svg: &str) -> Vec<BBox> {
    use regex::Regex;
    static PATH_RE: OnceLock<Regex> = OnceLock::new();
    static RECT_RE: OnceLock<Regex> = OnceLock::new();
    let path_re = PATH_RE.get_or_init(|| Regex::new(r#"<path\b[^>]*\bd="([^"]*)"[^>]*>"#).unwrap());
    let rect_re = RECT_RE.get_or_init(|| Regex::new(r#"<rect\b([^>]*)>"#).unwrap());

    let mut shapes = Vec::new();

    for caps in path_re.captures_iter(svg) {
        let d = &caps[1];
        let closed = d.trim_end().ends_with(['Z', 'z']);
        if !closed {
            continue;
        }
        if let Some(b) = path_bounds(d) {
            if b.width() > MIN_OVERLAP_PX && b.height() > MIN_OVERLAP_PX {
                shapes.push(b);
            }
        }
    }

    for caps in rect_re.captures_iter(svg) {
        let attrs = &caps[1];
        let x = attr_f64(attrs, "x").unwrap_or(0.0);
        let y = attr_f64(attrs, "y").unwrap_or(0.0);
        let (Some(w), Some(h)) = (attr_f64(attrs, "width"), attr_f64(attrs, "height")) else {
            continue;
        };
        if w > MIN_OVERLAP_PX && h > MIN_OVERLAP_PX {
            shapes.push(BBox {
                min_x: x,
                min_y: y,
                max_x: x + w,
                max_y: y + h,
            });
        }
    }

    shapes
}

/// Extract `<text>` labels (anchor point + content) from a Pikchr SVG.
fn extract_labels(svg: &str) -> Vec<SvgLabel> {
    use regex::Regex;
    static TEXT_RE: OnceLock<Regex> = OnceLock::new();
    let text_re = TEXT_RE.get_or_init(|| Regex::new(r#"<text\b([^>]*)>([^<]*)</text>"#).unwrap());

    let mut labels = Vec::new();
    for caps in text_re.captures_iter(svg) {
        let attrs = &caps[1];
        let (Some(x), Some(y)) = (attr_f64(attrs, "x"), attr_f64(attrs, "y")) else {
            continue;
        };
        let text = decode_xml_entities(caps[2].trim());
        if text.is_empty() {
            continue;
        }
        labels.push(SvgLabel { x, y, text });
    }
    labels
}

/// Read a numeric SVG attribute value out of an attribute substring.
fn attr_f64(attrs: &str, name: &str) -> Option<f64> {
    use regex::Regex;
    let re = Regex::new(&format!(r#"\b{name}="([^"]*)""#)).ok()?;
    re.captures(attrs)?
        .get(1)?
        .as_str()
        .trim()
        .parse::<f64>()
        .ok()
}

fn decode_xml_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// Attach a label to each shape by collecting the `<text>` elements whose
/// anchor falls inside the shape, in document order.
fn label_shapes(shapes: Vec<BBox>, labels: &[SvgLabel]) -> Vec<LabeledShape> {
    shapes
        .into_iter()
        .map(|bounds| {
            let mut parts: Vec<&str> = labels
                .iter()
                .filter(|l| bounds.contains(l.x, l.y))
                .map(|l| l.text.as_str())
                .collect();
            // A label inside two overlapping boxes is most likely owned by the
            // smaller / nearer one, but keeping it on both is fine for a hint.
            let label = if parts.is_empty() {
                None
            } else {
                let mut joined = String::new();
                for (idx, p) in parts.drain(..).enumerate() {
                    if idx > 0 {
                        joined.push(' ');
                    }
                    joined.push_str(p);
                }
                Some(truncate_label(&joined))
            };
            LabeledShape { bounds, label }
        })
        .collect()
}

fn truncate_label(s: &str) -> String {
    if s.chars().count() <= MAX_LABEL_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX_LABEL_CHARS).collect();
    out.push('…');
    out
}

/// Find pairs of box-like shapes that overlap by more than a hairline.
fn find_overlaps(shapes: &[LabeledShape]) -> Vec<Overlap> {
    let mut overlaps = Vec::new();
    for a in 0..shapes.len() {
        for b in (a + 1)..shapes.len() {
            let (w, h) = shapes[a].bounds.overlap_extent(&shapes[b].bounds);
            if w > MIN_OVERLAP_PX && h > MIN_OVERLAP_PX {
                overlaps.push(Overlap {
                    a,
                    b,
                    overlap_w: w,
                    overlap_h: h,
                });
            }
        }
    }
    overlaps
}

/// Run the full overlap analysis on a rendered Pikchr SVG.
fn analyze_overlaps(svg: &str) -> (Vec<LabeledShape>, Vec<Overlap>) {
    let shapes = extract_shapes(svg);
    let labels = extract_labels(svg);
    let labeled = label_shapes(shapes, &labels);
    let overlaps = find_overlaps(&labeled);
    (labeled, overlaps)
}

/// Human-readable description of one shape for the overlap summary.
fn describe_shape(shape: &LabeledShape) -> String {
    match &shape.label {
        Some(label) => format!("\"{label}\""),
        None => {
            let (cx, cy) = shape.bounds.center();
            format!("box near ({:.0}, {:.0})", cx, cy)
        }
    }
}

/// Build the text summary returned alongside the image. Text matters because
/// not every provider forwards image content to the model, and even
/// vision-less models can act on a textual overlap report.
fn build_summary(width: i64, height: i64, shapes: &[LabeledShape], overlaps: &[Overlap]) -> String {
    let mut out = format!("Rendered Pikchr diagram: {width}×{height} px.");
    if overlaps.is_empty() {
        out.push_str("\nNo box overlaps detected.");
        return out;
    }

    out.push_str(&format!(
        "\n⚠ {} overlapping shape pair(s) detected:",
        overlaps.len()
    ));
    for o in overlaps {
        out.push_str(&format!(
            "\n- {} overlaps {} (≈ {:.0}×{:.0} px)",
            describe_shape(&shapes[o.a]),
            describe_shape(&shapes[o.b]),
            o.overlap_w,
            o.overlap_h
        ));
    }
    out.push_str(
        "\nFix overlaps by setting an explicit flow direction, using named nodes \
with explicit anchors (e.g. `with .w at …`, `arrow from A.e to B.w`), and avoiding \
percentage-length arrows between `fit` boxes.",
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

/// Rasterize a Pikchr SVG to a PNG, scaled by `scale` (clamped, and reduced
/// further if needed to stay within `MAX_RENDER_DIMENSION`). Returns the PNG
/// bytes, or `None` if rasterization fails (the caller degrades to text-only).
fn rasterize_svg_to_png(svg: &str, scale: f32) -> Option<Vec<u8>> {
    let options = usvg::Options {
        fontdb: font_database(),
        ..Default::default()
    };
    let tree = match usvg::Tree::from_str(svg, &options) {
        Ok(tree) => tree,
        Err(e) => {
            log::warn!("[pikchr_mcp] usvg failed to parse rendered SVG: {e}");
            return None;
        }
    };

    let size = tree.size();
    let (w, h) = (size.width().max(1.0), size.height().max(1.0));

    let mut s = scale.clamp(MIN_SCALE, MAX_SCALE);
    let fit = (MAX_RENDER_DIMENSION as f32 / w).min(MAX_RENDER_DIMENSION as f32 / h);
    if s > fit {
        s = fit.max(MIN_SCALE / 2.0);
    }

    let px_w = ((w * s).ceil() as u32).clamp(1, MAX_RENDER_DIMENSION);
    let px_h = ((h * s).ceil() as u32).clamp(1, MAX_RENDER_DIMENSION);

    let mut pixmap = tiny_skia::Pixmap::new(px_w, px_h)?;
    // White background so the diagram reads like the (light-mode) app preview
    // instead of rendering on transparency.
    pixmap.fill(tiny_skia::Color::WHITE);

    resvg::render(
        &tree,
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

/// Outcome of a `preview_pikchr` call, ready to turn into a `CallToolResult`.
struct PreviewOutcome {
    png: Option<Vec<u8>>,
    summary: String,
    is_error: bool,
}

/// Render + analyze, producing the content blocks for the tool result.
/// Synchronous and self-contained so it can run on a blocking thread and be
/// unit-tested directly. `scale` is taken as-is and clamped internally.
fn run_preview(source: &str, scale: f32) -> PreviewOutcome {
    if source.trim().is_empty() {
        return PreviewOutcome {
            png: None,
            summary: "Pikchr source is empty — nothing to render.".to_string(),
            is_error: true,
        };
    }
    if source.len() > MAX_PIKCHR_SOURCE_LENGTH {
        return PreviewOutcome {
            png: None,
            summary: format!(
                "Pikchr source is too large to render safely ({} chars; limit {}).",
                source.len(),
                MAX_PIKCHR_SOURCE_LENGTH
            ),
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

    let (shapes, overlaps) = analyze_overlaps(&rendered.svg);
    let mut summary = build_summary(rendered.width, rendered.height, &shapes, &overlaps);

    let png = rasterize_svg_to_png(&rendered.svg, scale);
    if png.is_none() {
        summary.push_str("\n(Image rasterization unavailable; reporting geometry only.)");
    }

    PreviewOutcome {
        png,
        summary,
        is_error: false,
    }
}

#[derive(Clone)]
struct PikchrToolsHandler {
    tool_router: ToolRouter<Self>,
}

impl PikchrToolsHandler {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl PikchrToolsHandler {
    #[tool(
        description = "Render Pikchr diagram source to an image so you can check the layout before \
finalizing a note. Pass the contents of a ```pikchr fenced code block as `source`. Returns a PNG \
preview plus a text summary with the diagram's pixel dimensions and any box overlaps detected \
(overlapping or colliding boxes are the most common Pikchr layout mistake). On a syntax or layout \
error, returns the Pikchr error message so you can fix the source and try again."
    )]
    async fn preview_pikchr(
        &self,
        Parameters(p): Parameters<PreviewPikchrParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let scale = p.scale.unwrap_or(DEFAULT_SCALE);
        let outcome = tokio::task::spawn_blocking(move || run_preview(&p.source, scale))
            .await
            .map_err(|e| {
                ErrorData::internal_error(format!("preview_pikchr task failed: {e}"), None)
            })?;

        let mut content = Vec::new();
        if let Some(png) = outcome.png {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
            content.push(Content::image(b64, "image/png"));
        }
        content.push(Content::text(outcome.summary));

        if outcome.is_error {
            Ok(CallToolResult::error(content))
        } else {
            Ok(CallToolResult::success(content))
        }
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

/// Start a local MCP HTTP server exposing the `preview_pikchr` tool.
///
/// Returns the bound port and a `JoinHandle`. The server runs until the handle
/// (and its parent `LocalSet`) is dropped. Mirrors `start_project_mcp_server`
/// but carries no state.
pub async fn start_pikchr_mcp_server() -> Result<(u16, JoinHandle<()>), String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind pikchr MCP listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get local address: {e}"))?
        .port();

    let service = StreamableHttpService::new(
        || Ok(PikchrToolsHandler::new()),
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

    #[test]
    fn tokenize_handles_commas_and_arcs() {
        // From pikchr's own box output: a closed rounded rectangle.
        let d = "M161,72L309,72A15 15 0 0 0 324 57L324,17A15 15 0 0 0 309 2L161,2A15 15 0 0 0 161 17L161,57A15 15 0 0 0 161 72Z";
        let b = path_bounds(d).expect("closed path should have bounds");
        // The arc radii (15) and flags (0) must not leak into the box bounds.
        assert!((b.min_x - 161.0).abs() < 0.5, "min_x was {}", b.min_x);
        assert!((b.min_y - 2.0).abs() < 0.5, "min_y was {}", b.min_y);
        assert!((b.max_x - 324.0).abs() < 0.5, "max_x was {}", b.max_x);
        assert!((b.max_y - 72.0).abs() < 0.5, "max_y was {}", b.max_y);
    }

    #[test]
    fn overlap_detector_flags_known_overlapping_source() {
        let rendered = render_pikchr_svg(OVERLAPPING_SOURCE).expect("source should render");
        let (shapes, overlaps) = analyze_overlaps(&rendered.svg);
        assert_eq!(shapes.len(), 5, "expected five boxes");
        assert!(
            !overlaps.is_empty(),
            "expected overlaps for the cascade diagram, found none"
        );
    }

    #[test]
    fn overlap_detector_passes_corrected_source() {
        let rendered = render_pikchr_svg(CORRECTED_SOURCE).expect("source should render");
        let (_shapes, overlaps) = analyze_overlaps(&rendered.svg);
        assert!(
            overlaps.is_empty(),
            "corrected diagram should have no overlaps, found {overlaps:?}"
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
    fn oversized_source_is_rejected() {
        let big = "box\n".repeat(MAX_PIKCHR_SOURCE_LENGTH);
        let outcome = run_preview(&big, DEFAULT_SCALE);
        assert!(outcome.is_error);
        assert!(outcome.summary.contains("too large"));
    }

    #[test]
    fn summary_lists_overlap_count() {
        let rendered = render_pikchr_svg(OVERLAPPING_SOURCE).unwrap();
        let (shapes, overlaps) = analyze_overlaps(&rendered.svg);
        let summary = build_summary(rendered.width, rendered.height, &shapes, &overlaps);
        assert!(summary.contains("overlapping shape pair"));
        assert!(summary.contains('⚠'));
    }
}
