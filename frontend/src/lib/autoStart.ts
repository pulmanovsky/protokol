/**
 * «Почати запис» pressed away from the home screen: the home screen starts
 * the recording when it opens. The request holds only for a few seconds after
 * the press, so a request left over (an interrupted navigation, the first-run
 * setup) never starts a recording later by itself.
 */
const KEY = 'autoStartRecording';
const FRESH_MS = 10_000;

export function requestAutoStart(): void {
  try {
    sessionStorage.setItem(KEY, String(Date.now()));
  } catch {
    // no storage: the recording is started from the home screen by hand
  }
}

/** True once for a request made in the last few seconds; any request is cleared */
export function takeAutoStart(): boolean {
  try {
    const value = sessionStorage.getItem(KEY);
    if (value === null) return false;
    sessionStorage.removeItem(KEY);
    const age = Date.now() - Number(value);
    return Number.isFinite(age) && age >= 0 && age < FRESH_MS;
  } catch {
    return false;
  }
}
