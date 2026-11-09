// Run: node --experimental-strip-types src/lib/timeline.test.mjs
// Moments map to the line that holds them, also when lines are out of order
// (lines with speakers are rewritten in place) and for moments inside a line.
import assert from 'node:assert/strict';
import { segmentIndexAt } from './timeline.ts';

// Line starts of an imported 37-minute meeting (first lines, in seconds)
const lines = [0.12, 25.52, 53.34, 77.49, 103.6, 126.74, 151.26].map((timestamp) => ({ timestamp }));
assert.equal(segmentIndexAt(lines, 77), 3); // «00:01:17» → the line that starts at 77.49
assert.equal(segmentIndexAt(lines, 80), 3); // inside that line
assert.equal(segmentIndexAt(lines, 103.6), 4);
assert.equal(segmentIndexAt(lines, 0), 0);
assert.equal(segmentIndexAt(lines, 5000), 6);
assert.equal(segmentIndexAt([], 10), -1);
// A line that starts later within the same second: «30:04» is that line, not the one before
const late = [1752.87, 1779.68, 1804.83, 1813.41].map((timestamp) => ({ timestamp }));
assert.equal(segmentIndexAt(late, 1804), 2);
assert.equal(segmentIndexAt(late, 1804.83), 2);
assert.equal(segmentIndexAt(late, 1803), 1);
const shuffled = [lines[4], lines[0], lines[3], lines[1]];
assert.equal(segmentIndexAt(shuffled, 90), 2);
console.log('timeline: ok');
