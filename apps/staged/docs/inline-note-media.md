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
as the renderer does, so an image indented under a list step is ingested.

Supported formats are PNG, JPEG, GIF, and WebP up to 10 MiB, and MP4, WebM, and
MOV up to 100 MiB. Extension and file signatures must agree. Video validation
checks the container signature, not whether every codec inside it can play in
the current webview. Older QuickTime files without an `ftyp` header are rejected.

On session completion, files are copied into the existing project `images/`
store and destinations become `staged-media://<uuid>.<ext>`. Captions and titles
are preserved. Invalid or unavailable media becomes readable text explaining
the failure. There are no schema changes or new IPC commands.

Images render inline with captions and an expand button; clicking the image
also opens its viewer. Videos have native playback controls and an expand
button. Enter/Space activates the expand button, and Escape closes the viewer.
The viewer removes its video when closed. The Markdown sanitizer remains
unchanged: only validated media references use trusted renderer HTML.

## Persistence and agent context

Note attachments have session IDs, so they do not become separate timeline or
`#image:` entries. They are also excluded from session-restart chat image inputs.
Amendments reuse previous attachments by ID or SHA-256 content. Branch moves
already relocate session-scoped image rows and files. Only the completed turn's
output is scanned for a note, so a follow-up that does not rewrite the note
leaves the saved note and its attachments untouched, even if the original
source files no longer exist.

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
placeholder with caption, filename, size, and reason. Remote ingestion bounds
output with `head` and uses positional shell arguments for paths.

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
path forms, code skipping, titles, magic/size validation, deduplication,
rollback, deletion and shared references, local materialization and missing
files, the remote video cap, project-agent child-note handoff, desktop ranges,
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
