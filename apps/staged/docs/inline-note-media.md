# Inline images and videos in notes

An agent can embed local files in a branch or project note using ordinary
Markdown image syntax, including for videos:

```markdown
![Check the updated login form](/absolute/path/login.png)
![Watch the flicker at the start](/tmp/reproduction.mp4)
```

Absolute paths, `~/` paths, `file:///` URLs, and paths relative to the session's
working directory are accepted. Use angle brackets around paths with spaces, or
percent-encode the spaces. HTTP(S) images remain external. Reference-style
images and raw HTML media are not ingested. Fenced and indented code examples
are skipped. Indentation is judged relative to the enclosing list item or quote,
as the renderer does, so an image indented under a list step is ingested. A
fence ends with its container: a line without the `>` prefix ends a quoted
fence, and a dedented ```` ``` ```` under a list item ends the item and opens
a new top-level fence rather than closing the old one, so the text that
follows stays code. Other dedented lines are absorbed by the list item, as
Marked does, unless a blank line precedes them. HTML blocks are skipped too:
`<pre>`, `<script>`, `<style>`, and `<textarea>` run to their own closing tag
(a `</script>` does not end a `<pre>` block, and with no matching closer the
block runs to the end of the note, as Marked's backreference does), `<!-- -->`
runs to `-->`, and block-level tags such as `<div>` or `<table>` run to the
next blank line. Processing instructions (`<?…?>`), declarations
(`<!DOCTYPE …>`), and `<![CDATA[…]]>` sections are skipped as well, each
ending only on its own closer (a bare `>` ends a declaration but not a CDATA
section or processing instruction). Matching Marked, these three neither
interrupt a paragraph nor end a list item from a dedented line, whereas raw
tags, comments, and block-level tags do. An arbitrary custom tag on its own
line is not recognized as an HTML block. Inline HTML spans inside a paragraph
(such as `Para <!--` continuing onto the next line) are not tracked, so an
image Marked hides inside one is still ingested.

Supported formats are PNG, JPEG, GIF, and WebP up to 10 MiB, and MP4, WebM, and
MOV up to 100 MiB. Extension and file signatures must agree. Video validation
checks the container signature, not whether every codec inside it can play in
the current webview. Older QuickTime files without an `ftyp` header are rejected.

Chat attachments (file picker, paste, drop) share the validation code but are
stored under the format their bytes actually are: browsers label `file.type`
from the filename, so a WebP saved as `shot.png` is stored as `shot.webp` with
an `image/webp` MIME rather than rejected. Only unrecognized signatures, videos,
and oversized files are refused, and the composer shows the reason for pasted,
picked, and natively dropped files alike, clearing it when the message is sent
or the attachment removed.

On session completion, files are copied into the existing project `images/`
store and destinations become `staged-media://<uuid>.<ext>`. Captions and titles
are preserved. Invalid or unavailable media becomes readable text explaining
the failure. There are no schema changes or new IPC commands.

Images render inline with captions and an expand button; clicking the image
also opens its viewer. Captions are the alt text flattened to plain text, as a
plain image's `alt` would be, so `![**Before** fix](…)` reads "Before fix", and
the trusted figure HTML is spliced back in literally, so `$&`, `` $` ``, and
`$'` in a caption or title cannot pull surrounding text into an attribute.
Videos have native playback controls and an expand button. Enter/Space activates the expand button, and Escape closes the viewer.
The viewer removes its video when closed. The Markdown sanitizer remains
unchanged: only validated media references use trusted renderer HTML.

## Persistence and agent context

Note attachments have session IDs, so they do not become separate timeline or
`#image:` entries. They are also excluded from session-restart chat image inputs.
Amendments reuse previous attachments by ID or SHA-256 content; the previous
attachments are only read and hashed once a fresh file actually needs
deduplicating, so amendments that repeat stored references read no attachment
bytes. Branch moves already relocate session-scoped image rows and files. Only
the completed turn's output is scanned for a note, so a follow-up that does not
rewrite the note leaves the saved note and its attachments untouched, even if
the original source files no longer exist. A turn that does rewrite the note
typically repeats the agent's original source paths, since the agent never saw
the stored references. When such a path no longer exists, the attachment the
previous note saved under the same basename is reused, so the rewrite keeps the
stable reference instead of deleting the attachment. The store numbers
colliding filenames across the whole branch (`shot.png`, `shot 2.png`, …),
chat uploads included, so the match accepts either the bare name or a numbered
variant. When several previous attachments match, alt text breaks ties, and a
row already claimed by an earlier reference in the same pass is not offered
again, so two screenshots that shared a basename each keep their own file. A
match that is still ambiguous becomes a placeholder. Only a missing source
triggers this fallback: a file that exists at the old path but is oversized,
mislabelled, or not a regular file is reported as such, never silently replaced
by the previous attachment. On Blox the read script exits 1 for a missing path
and 2, with a `not a regular file` message, for a directory or device at it,
and only the former falls back; an unreachable workspace is treated as a
missing source so a saved attachment is not deleted over a transient failure.
A turn that rewrites the note without a `suggested-next-steps` block keeps the
stored next steps, since only the turn's own output is scanned for the block.

Cleanup runs **after** a successful note save, and failed saves roll back new
attachments. Deleting a project note includes its child-note attachments.
References in other notes or chat attachments protect shared files. Reusing a
chat attachment or a reference from another branch/project scope creates a
note-owned copy, so cleanup and independent branch moves remain safe. Copying a
reference between notes in the same scope can share the existing attachment.

When a note is supplied to a later agent, its media is copied next to the note
in the local OS temp directory or into `/tmp` on Blox. The Markdown points to
those paths. Missing files become text placeholders. Images and videos up to
20 MiB use the existing chunked Blox transfer; larger videos produce a
placeholder with caption, filename, size, and reason. Because the remote name
`/tmp/staged-image-<id>.<ext>` is stable, each context build first checks the
remote file's size with a single `ws_exec` and skips the transfer when it
matches the stored size, so repeated session starts do not re-send every
attachment. A transfer stages its chunks in a per-transfer `.part` file beside
the destination and the last chunk's command renames it into place, so the
final path only ever holds a complete file, the size check never measures a
file another build is still writing, and a failed transfer leaves nothing at
the final path. Remote ingestion bounds output with `head` and uses positional
shell arguments for paths.

A project agent reads a child repo session's note through the
`wait_for_repo_session` and `cancel_repo_session` replies, which materialize
the note content the same way, always into the local OS temp directory because
project sessions run locally. The reply's `output` field is the child's raw
final message and still carries the child's original pre-ingest paths, which
may no longer exist (and never do on Blox), so parents should prefer
`note.content`.

## Serving and implementation choices

The desktop `staged-media` protocol resolves IDs through the store and handles
GET/HEAD plus single byte ranges, including open-ended and suffix ranges.
It returns 416 for invalid or unsatisfiable ranges and reads only the requested
slice on a blocking worker. The browser route `/api/media/{file}` uses
`ServeFile` and the same authentication predicate as `require_auth`.
`media_router` is ready for the separate mobile-web server startup restoration;
this feature does not enable the currently stubbed web server.

Compared with the original plan, responsibilities are split under `note_media/`
(parsing, file validation/storage, ingestion/cleanup, materialization, serving)
instead of one Rust file. Context materialization lives there and is called by
both note formatters and the project-agent repo-session payload. Shared media
CSS covers both notes and chat. Placeholders are escaped text rather than
broken Markdown image links. The only dependency declaration added is
**dev-only** `tower` (already present transitively) for authenticated router
`oneshot` tests.

## Validation

Automated tests cover ingestion and note completion for both note kinds,
path forms, code and HTML block skipping (checked against Marked, with a
renderer parity test), fence and container interplay, titles, magic/size
validation, sniffed chat attachment formats, deduplication and its lazy
hashing, basename reuse on rewrites (numbered filenames, one claim per
attachment, and the missing-source restriction locally and through the remote
read script's exit paths), preserved next steps on rewrites without a block,
rollback, deletion and shared references, local materialization
and missing files, the remote video cap, size-check skip, and staged rename
(through a fake workspace shell), project-agent child-note handoff, desktop ranges,
authenticated browser serving, renderer escaping, viewer targets, platform URL
resolution, and Milkdown round-tripping. Run the repository's Rust tests,
frontend tests, type check, Clippy, and formatting checks using Hermit.

Native smoke validation used a disposable Tauri/WKWebView app with the
production renderer, viewer, ingestion, and byte handler: PNG display,
MP4 playback and seeking, both expanded viewers, and Escape dismissal passed.
The handler logged 206 responses during playback/seeking. The temporary harness
was removed afterward. Live Blox transfer, the disabled web server, and
Windows-specific playback were not exercised; browser serving is covered by
authenticated router tests. Codec support remains platform-dependent.
