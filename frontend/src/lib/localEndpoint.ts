/** Ollama is only allowed on this machine */
export function isLocalEndpoint(url: string | null | undefined): boolean {
  if (!url?.trim()) return true; // default endpoint is localhost
  try {
    const host = new URL(url.trim()).hostname;
    return host === 'localhost' || host === '127.0.0.1' || host === '::1' || host === '[::1]';
  } catch {
    return false;
  }
}
