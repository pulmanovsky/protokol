// Run: node --experimental-strip-types src/lib/speakerText.test.mjs
import assert from 'node:assert/strict';
import { speakerParagraphs } from './speakerText.ts';

const lines = [
  { speaker: 'Спікер 2', text: 'Відповідь.', start: 20 },
  { speaker: 'Тестовий', text: 'Перше  речення.', start: 5 },
  { speaker: 'Тестовий', text: 'Друге.', start: 10 },
  { speaker: 'Тестовий', text: 'Третє.', start: 30 },
  { speaker: 'Тестовий?', text: 'Інша позначка.', start: 40 },
  { speaker: 'Тестовий', text: '— Мікрофон вимкнено —', start: 50 },
  { speaker: 'Спікер 2', text: '   ', start: 55 },
  { speaker: undefined, text: 'Без спікера.', start: 60 },
];
assert.deepEqual(speakerParagraphs(lines, 'Тестовий'), ['Перше речення. Друге.', 'Третє.']);
assert.deepEqual(speakerParagraphs(lines, 'Спікер 2'), ['Відповідь.']);
assert.deepEqual(speakerParagraphs(lines, 'Тестовий?'), ['Інша позначка.']);
assert.deepEqual(speakerParagraphs(lines, 'Нема'), []);
assert.deepEqual(speakerParagraphs(lines, 'Тестовий', (s) => `00:${String(s).padStart(2, '0')}`), ['00:05 Перше речення. Друге.', '00:30 Третє.']);
console.log('ok');
