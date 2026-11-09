// Run: node --experimental-strip-types src/lib/playerTime.test.mjs
import assert from 'node:assert/strict';
import { toPlayer, toSound } from './playerTime.ts';

const near = (a, b) => assert.ok(Math.abs(a - b) < 1e-6, `${a} vs ${b}`);
assert.equal(toPlayer([], 12.5), 12.5);
assert.equal(toSound([], 12.5), 12.5);

// Two joins, each 0.0373 s shorter in the file than in the sound
const lost = 1792 / 48000;
const j1 = 30.016;
const j2 = 60.08;
const points = [
  [j1, j1],
  [j1 + 0.0053 + lost, j1 + 0.0053],
  [j2, j2 - lost],
  [j2 + 0.0053 + lost, j2 + 0.0053 - lost],
];
near(toPlayer(points, 10), 10);
near(toPlayer(points, 45), 45 - lost);
near(toPlayer(points, 100), 100 - 2 * lost);
near(toSound(points, 100 - 2 * lost), 100);
for (const t of [0, 5, 29, 31, 59, 61, 500]) near(toSound(points, toPlayer(points, t)), t);
let last = -1;
for (let t = 0; t < 70; t += 0.01) {
  const p = toPlayer(points, t);
  assert.ok(p >= last);
  last = p;
}
console.log('ok');
