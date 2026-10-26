-- Intrinsic pixel size and density of a raster image, read from the file's
-- own metadata (PNG pHYs, JFIF/EXIF resolution) so note embeds can lay a 2x
-- screenshot out at its logical size instead of guessing. NULL for videos and
-- PDFs, for files whose headers could not be read, and for rows that predate
-- this migration; `get_media_dimensions` reads those back from disk the first
-- time a note embedding them is opened.
ALTER TABLE images ADD COLUMN width INTEGER;
ALTER TABLE images ADD COLUMN height INTEGER;
ALTER TABLE images ADD COLUMN density INTEGER;
