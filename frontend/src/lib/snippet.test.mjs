// Run: node --experimental-strip-types src/lib/snippet.test.mjs
import assert from 'node:assert/strict';
import { snippetRange } from './snippet.ts';

const words = [
  { start: 10.0, end: 10.4 },
  { start: 10.4, end: 11.0 },
  { start: 11.0, end: 11.5 },
  { start: 11.5, end: 11.8 },
  { start: 11.8, end: 12.6 },
];
// The neighbours' edges are close: the snippet is whole words
assert.deepEqual(snippetRange(words, 2, words[2]), { start: 10.4, end: 11.8 });
// No word edge near: the plain margin, never shorter than the word
const far = [{ start: 20, end: 21 }, { start: 30, end: 30.6 }, { start: 40, end: 41 }];
assert.deepEqual(snippetRange(far, 1, far[1]), { start: 29.5, end: 31.1 });
// The first and the last word of a line
assert.deepEqual(snippetRange(words, 0, words[0]), { start: 9.5, end: 11.0 });
assert.deepEqual(snippetRange(words, 4, words[4]), { start: 11.5, end: 13.1 });
// The very start of the recording; no times for the line
assert.deepEqual(snippetRange(undefined, -1, { start: 0.2, end: 0.6 }), { start: 0, end: 1.1 });
for (let i = 0; i < words.length; i++) {
  const r = snippetRange(words, i, words[i]);
  assert.ok(r.start < words[i].start && r.end > words[i].end);
}
console.log('ok');
