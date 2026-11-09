export interface SpokenLine {
  speaker?: string | null;
  text?: string | null;
  start: number;
}

/**
 * Everything one speaker said, in order: a paragraph for each run of their
 * lines (another speaker in between starts a new one). `time` puts each
 * paragraph's start in front of it.
 */
export function speakerParagraphs(lines: SpokenLine[], who: string, time?: (seconds: number) => string): string[] {
  const out: string[] = [];
  let run: string[] = [];
  let at = 0;
  const close = () => {
    if (run.length > 0) out.push(`${time ? `${time(at)} ` : ''}${run.join(' ')}`);
    run = [];
  };
  for (const line of [...lines].sort((a, b) => a.start - b.start)) {
    const text = (line.text ?? '').replace(/\s+/g, ' ').trim();
    if (!text || /^—\s*Мікрофон (вимкнено|увімкнено)\s*—$/.test(text)) continue;
    if (line.speaker === who) {
      if (run.length === 0) at = line.start;
      run.push(text);
    } else {
      close();
    }
  }
  close();
  return out;
}
