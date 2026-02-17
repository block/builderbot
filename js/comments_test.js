const { describe, it } = require('node:test');
const assert = require('node:assert/strict');
const { orderComments, formatTime, truncateText } = require('../templates/comments.js');

// Helper: create a comment object matching the JSON shape used by the frontend.
function makeComment(id, inReplyTo, seconds) {
    // Use a fixed base date (2025-01-01T00:00:00Z) plus seconds offset,
    // mirroring the Go test helper.
    var base = new Date('2025-01-01T00:00:00Z');
    return {
        id: id,
        author: 'user',
        role: 'human',
        body: 'comment ' + id,
        createdAt: new Date(base.getTime() + seconds * 1000).toISOString(),
        inReplyTo: inReplyTo || '',
    };
}

function ids(comments) {
    return (comments || []).map(function(c) { return c.id; });
}

// ---------- orderComments tests (mirrors ordering_test.go) ----------

describe('orderComments', function() {
    it('empty input', function() {
        assert.deepEqual(orderComments(null), null);
        assert.deepEqual(orderComments([]), []);
    });

    it('single comment', function() {
        var cs = [makeComment('a', '', 1)];
        assert.deepEqual(ids(orderComments(cs)), ['a']);
    });

    it('roots only — sorted by time', function() {
        var cs = [
            makeComment('c', '', 3),
            makeComment('a', '', 1),
            makeComment('b', '', 2),
        ];
        assert.deepEqual(ids(orderComments(cs)), ['a', 'b', 'c']);
    });

    it('linear replies', function() {
        var cs = [
            makeComment('a', '', 1),
            makeComment('b', 'a', 2),
            makeComment('c', 'b', 3),
        ];
        assert.deepEqual(ids(orderComments(cs)), ['a', 'b', 'c']);
    });

    it('interleaved replies from multiple roots', function() {
        var cs = [
            makeComment('a', '', 1),
            makeComment('b', '', 2),
            makeComment('c', 'a', 3),
            makeComment('d', 'b', 4),
        ];
        assert.deepEqual(ids(orderComments(cs)), ['a', 'c', 'b', 'd']);
    });

    it('missing parent fallback — orphan becomes root', function() {
        var cs = [
            makeComment('a', '', 1),
            makeComment('b', 'nonexistent', 2),
            makeComment('c', 'a', 3),
        ];
        assert.deepEqual(ids(orderComments(cs)), ['a', 'c', 'b']);
    });

    it('deep nesting', function() {
        var cs = [
            makeComment('a', '', 1),
            makeComment('b', 'a', 2),
            makeComment('c', 'b', 3),
            makeComment('d', 'c', 4),
        ];
        assert.deepEqual(ids(orderComments(cs)), ['a', 'b', 'c', 'd']);
    });

    it('sibling sort by time', function() {
        var cs = [
            makeComment('a', '', 1),
            makeComment('d', 'a', 4),
            makeComment('b', 'a', 2),
            makeComment('c', 'a', 3),
        ];
        assert.deepEqual(ids(orderComments(cs)), ['a', 'b', 'c', 'd']);
    });
});

// ---------- truncateText tests ----------

describe('truncateText', function() {
    it('returns empty string for falsy input', function() {
        assert.equal(truncateText(null, 10), '');
        assert.equal(truncateText('', 10), '');
    });

    it('returns text unchanged when within limit', function() {
        assert.equal(truncateText('hello', 10), 'hello');
    });

    it('truncates with ellipsis when over limit', function() {
        assert.equal(truncateText('hello world', 5), 'hello...');
    });

    it('handles exact boundary', function() {
        assert.equal(truncateText('12345', 5), '12345');
        assert.equal(truncateText('123456', 5), '12345...');
    });
});

// ---------- formatTime tests ----------

describe('formatTime', function() {
    it('returns empty string for falsy input', function() {
        assert.equal(formatTime(null), '');
        assert.equal(formatTime(''), '');
    });

    it('returns "just now" for recent timestamps', function() {
        var now = new Date();
        assert.equal(formatTime(now.toISOString()), 'just now');
    });

    it('returns minutes ago', function() {
        var d = new Date(Date.now() - 5 * 60000);
        assert.equal(formatTime(d.toISOString()), '5m ago');
    });

    it('returns hours ago', function() {
        var d = new Date(Date.now() - 3 * 3600000);
        assert.equal(formatTime(d.toISOString()), '3h ago');
    });

    it('returns days ago', function() {
        var d = new Date(Date.now() - 2 * 86400000);
        assert.equal(formatTime(d.toISOString()), '2d ago');
    });

    it('returns formatted date for old timestamps', function() {
        var result = formatTime('2020-06-15T12:00:00Z');
        // Should be something like "Jun 15"
        assert.match(result, /Jun 15/);
    });
});
