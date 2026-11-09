// Run: node --experimental-strip-types src/lib/docSections.test.mjs
import assert from 'node:assert/strict';
import { documentSections, sectionPlain, withoutPointTimes } from './docSections.ts';

const doc = `# Нарада

**Дата:** 08.11.2026

## Хід наради

**ВИСТУПИЛИ:** Тестовий Довгий [00:14] · 2 хв

- Перше [00:14]
- Друге з числом 5 [01:02]. ⟨?⟩

**ВИСТУПИЛИ:** Спікер 3 \\[03:48\\] · 1 хв

* Третє ⟨02:14 · 12:06⟩

**Оголошення**

Текст не належить нікому.

## Загальний підсумок

- Підсумок один
- Підсумок два [05:00]
`;
const s = documentSections(doc);
assert.deepEqual(s.map((x) => [x.kind, x.title, x.lines.length]), [
  ['speaker', 'Тестовий Довгий', 2],
  ['speaker', 'Спікер 3', 1],
  ['overall', 'Загальний підсумок', 2],
]);
assert.equal(sectionPlain(s[0], false), 'Тестовий Довгий\n– Перше\n– Друге з числом 5.');
assert.equal(sectionPlain(s[0], true), 'Тестовий Довгий\n– Перше [00:14]\n– Друге з числом 5 [01:02].');
assert.equal(sectionPlain(s[1], true), 'Спікер 3\n– Третє [02:14 · 12:06]');
assert.equal(sectionPlain(s[2], false), 'Загальний підсумок\n– Підсумок один\n– Підсумок два');
// The old layout of a block
const old = documentSections('**ВИСТУПИЛИ:** Тестовий [00:14; 14:17]\n\n- Зроблено: робота ⟨14:17⟩\n- Не враховано: «фраза»');
assert.equal(sectionPlain(old[0], false), 'Тестовий\n– Зроблено: робота\n– Не враховано: «фраза»');
assert.equal(withoutPointTimes('**ВИСТУПИЛИ:** Х [00:14] · 2 хв\n\n- Перше [00:14]\n- Друге \\[01:02\\]'), '**ВИСТУПИЛИ:** Х [00:14] · 2 хв\n\n- Перше\n- Друге');
console.log('ok');
