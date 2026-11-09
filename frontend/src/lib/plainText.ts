/**
 * Markdown → plain text for pasting into a message or an e-mail:
 * no #, **, tables or escape characters; speaker lines and paragraphs stay.
 */
export function markdownToPlainText(markdown: string): string {
  return markdown
    .replace(/\r\n/g, '\n')
    .split('\n')
    .filter((line) => !/^\s*\|?\s*:?-{3,}/.test(line)) // table separator rows
    .map((line) =>
      line
        .replace(/^#{1,6}\s+/, '')
        .replace(/^\s*\|(.*)\|\s*$/, (_, row: string) => row.split('|').map((c) => c.trim()).join(' — '))
        .replace(/\*\*(.+?)\*\*/g, '$1')
        .replace(/__(.+?)__/g, '$1')
        .replace(/(^|[^*])\*(?!\s)(.+?)\*(?!\*)/g, '$1$2')
        .replace(/`([^`]+)`/g, '$1')
        .replace(/\\([\\`*_{}\[\]()#+\-.!|])/g, '$1')
        .replace(/[ \t]+$/, ''),
    )
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}
