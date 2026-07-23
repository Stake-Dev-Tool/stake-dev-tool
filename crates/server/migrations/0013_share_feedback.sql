-- Share-link visitor feedback (the "Feedback widget tied to the exact bet
-- event" item from V2.md §Share v2). An opt-in per-link toggle injects a
-- feedback overlay into the served front bundle; visitors submit written notes
-- and/or an Excalidraw-style annotation drawn over the game, and each entry is
-- stamped with the last played round — the `(revision, mode, eventId)` triplet
-- that addresses a book line, exactly like saved rounds.

ALTER TABLE share_links
    ADD COLUMN feedback_enabled BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE share_feedback (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    share_link_id   UUID NOT NULL REFERENCES share_links (id) ON DELETE CASCADE,
    -- The visitor's raw (un-namespaced) share session id, for context/dedup.
    session_id      TEXT,
    -- Optional display name typed by the visitor.
    author_name     TEXT,
    message         TEXT NOT NULL DEFAULT '',
    -- Vector annotation shapes drawn over the game viewport, in CSS-pixel
    -- coordinates of (viewport_w, viewport_h): {"shapes":[{t,c,s,...}]}.
    drawing         JSONB,
    -- Best-effort JPEG/PNG/WebP capture of the game at annotation time. Stored
    -- inline (small, capped server-side) so ON DELETE CASCADE cleans it up and
    -- the blob GC never has to know about feedback.
    screenshot      BYTEA,
    screenshot_mime TEXT,
    -- The last played round when the feedback was submitted: the book line is
    -- addressed by (revision_number, mode, event_id). All nullable — feedback
    -- can arrive before the first spin.
    mode            TEXT,
    event_id        INTEGER,
    revision_number INTEGER,
    -- Viewport dimensions the drawing coordinates are relative to.
    viewport_w      INTEGER,
    viewport_h      INTEGER,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX share_feedback_link_created_idx
    ON share_feedback (share_link_id, created_at DESC);
