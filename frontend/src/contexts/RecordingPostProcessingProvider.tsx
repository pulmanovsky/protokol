'use client';

import React, { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useRecordingStop } from '@/hooks/useRecordingStop';

/**
 * RecordingPostProcessingProvider
 *
 * This provider handles post-processing when recording stops from any source:
 * - Tray menu stop
 * - Global keyboard shortcut
 * - Overlay stop button
 * - Main UI stop button
 *
 * It listens for the 'recording-stop-complete' event from Rust backend
 * and triggers the full post-processing flow (save to database, navigate, analytics)
 * regardless of which page the user is currently on.
 */
export function RecordingPostProcessingProvider({ children }: { children: React.ReactNode }) {
  // No-op functions since the global RecordingStateContext already handles state updates
  // These are only needed for the hook's local component state management
  const setIsRecording = () => { };
  const setIsRecordingDisabled = () => { };

  const {
    handleRecordingStop,
  } = useRecordingStop(setIsRecording, setIsRecordingDisabled);

  // The latest handler, so the one listener below always calls the current
  // one (with the current meeting list refresh, not the one from the first
  // render, when the list could not be fetched yet)
  const handlerRef = useRef(handleRecordingStop);
  useEffect(() => {
    handlerRef.current = handleRecordingStop;
  }, [handleRecordingStop]);

  // One listener for the life of the window. A listener set up again on
  // every render used to stay behind when the cleanup ran before `listen`
  // resolved; the oldest of them then saved the recording without
  // refreshing the lists.
  useEffect(() => {
    let cancelled = false;
    let unlistenFn: UnlistenFn | undefined;
    listen<boolean>('recording-stop-complete', (event) => {
      console.log('[RecordingPostProcessing] Received recording-stop-complete event:', event.payload);
      // event.payload is the callApi boolean (true for normal stops)
      handlerRef.current(event.payload);
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlistenFn = fn;
      })
      .catch((error) => console.error('[RecordingPostProcessing] Failed to set up event listener:', error));
    return () => {
      cancelled = true;
      unlistenFn?.();
    };
  }, []);

  return <>{children}</>;
}
