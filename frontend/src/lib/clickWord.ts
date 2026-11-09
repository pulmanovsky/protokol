'use client';

/** «Клік по слову — прослухати»: a click on a word while paused plays it (on by default) */
const KEY = 'protokol.clickPlaysWord';

export function clickPlaysWord(): boolean {
  try {
    return window.localStorage.getItem(KEY) !== '0';
  } catch {
    return true;
  }
}

export function setClickPlaysWord(on: boolean) {
  try {
    window.localStorage.setItem(KEY, on ? '1' : '0');
  } catch {
    // not remembered: the default stays
  }
}
