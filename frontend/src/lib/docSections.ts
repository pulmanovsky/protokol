/** A person's part of the document, or its overall summary */
export interface DocSection {
  kind: 'speaker' | 'overall';
  title: string;
  lines: string[];
}

const unescape = (s: string) => s.replace(/\\([[\]()*_.!#-])/g, '$1');
const isHeading = (s: string) => /^#{1,6}\s/.test(s);
const isBoldLabel = (s: string) => /^\*\*[^*]+\*\*:?$/.test(s);
const bullet = (s: string) => s.match(/^[-*•–]\s+(.*)$/)?.[1];

/** The speakers' blocks and the overall summary, in order */
export function documentSections(markdown: string): DocSection[] {
  const out: DocSection[] = [];
  let open: DocSection | null = null;
  for (const raw of markdown.split('\n')) {
    const line = unescape(raw.trim());
    if (!line) continue;
    const speaker = line.match(/^\*\*ВИСТУПИЛИ:\*\*\s*(.+?)(?:\s*\[[^\]]*\].*)?$/);
    if (speaker) {
      open = { kind: 'speaker', title: speaker[1].trim(), lines: [] };
      out.push(open);
    } else if (/^#{1,6}\s*Загальний підсумок\s*$/.test(line)) {
      open = { kind: 'overall', title: 'Загальний підсумок', lines: [] };
      out.push(open);
    } else if (isHeading(line) || isBoldLabel(line)) {
      open = null;
    } else if (open) {
      open.lines.push(line);
    }
  }
  return out;
}

const sourceTime = /\s*(?:⟨(\d{1,2}:\d{2}(?:\s*·\s*\d{1,2}:\d{2})*)⟩|\[(\d{1,2}:\d{2})\])/g;

function plainLine(line: string, withTimes: boolean): string {
  const text = line.replace(/\s*⟨\?⟩/g, '').replace(sourceTime, (_, a, b) => (withTimes ? ` [${a ?? b}]` : ''));
  return text.replace(/\*\*/g, '').trim();
}

/** A section as text to paste: its name, then the points as «– …» */
export function sectionPlain(section: DocSection, withTimes: boolean): string {
  const body = section.lines.map((l) => {
    const point = bullet(l);
    return point !== undefined ? `– ${plainLine(point, withTimes)}` : plainLine(l, withTimes);
  });
  return [section.title, ...body].join('\n');
}

/** The times after the points of a whole document (the blocks' own stay) */
export function withoutPointTimes(markdown: string): string {
  return markdown
    .split('\n')
    .map((l) => (/^\s*[-*•]\s/.test(l) ? l.replace(/\s*\\?\[\d{1,2}:\d{2}\\?\]/g, '') : l))
    .join('\n');
}
