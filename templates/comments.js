// comments.js — shared utility functions for comment ordering and display.
// Works in both browser (global scope) and Node.js (CommonJS).
(function(exports) {
    'use strict';

    function orderComments(comments) {
        if (!comments || comments.length <= 1) return comments;
        var byId = {};
        comments.forEach(function(c) { byId[c.id] = true; });

        var children = {};
        var roots = [];
        comments.forEach(function(c) {
            var parent = c.inReplyTo || '';
            if (parent && byId[parent]) {
                (children[parent] = children[parent] || []).push(c);
            } else {
                roots.push(c);
            }
        });

        var byTime = function(a, b) { return a.createdAt < b.createdAt ? -1 : 1; };
        roots.sort(byTime);
        Object.keys(children).forEach(function(k) { children[k].sort(byTime); });

        var result = [];
        function walk(id) {
            (children[id] || []).forEach(function(c) {
                result.push(c);
                walk(c.id);
            });
        }
        roots.forEach(function(c) { result.push(c); walk(c.id); });
        return result;
    }

    function formatTime(isoStr) {
        if (!isoStr) return '';
        var d = new Date(isoStr);
        var now = new Date();
        var diffMs = now - d;
        var diffMins = Math.floor(diffMs / 60000);
        if (diffMins < 1) return 'just now';
        if (diffMins < 60) return diffMins + 'm ago';
        var diffHours = Math.floor(diffMins / 60);
        if (diffHours < 24) return diffHours + 'h ago';
        var diffDays = Math.floor(diffHours / 24);
        if (diffDays < 7) return diffDays + 'd ago';
        return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
    }

    function truncateText(text, maxLen) {
        if (!text) return '';
        return text.length > maxLen ? text.substring(0, maxLen) + '...' : text;
    }

    exports.orderComments = orderComments;
    exports.formatTime = formatTime;
    exports.truncateText = truncateText;

})(typeof module !== 'undefined' && module.exports ? module.exports : (this.penpal = this.penpal || {}));
