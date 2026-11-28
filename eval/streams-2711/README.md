# Separate Meet streams vs the mixed recording: scripts of the 27.11.2026 test calls

Synthetic guests only (macOS voice, civilian text); audio and reports stay on the test Mac.

- `guests.mjs` — Chrome guests whose microphone is a WebAudio stream: `launch <code>` (ONLY=1,2,...), `speak` (turns.tsv),
  `overlap` (overlap.tsv: group, guest, offset s, text; clips/oN.wav) → timeline.jsonl (who played what, wall clock).
- `make.py` — round 2 script: groups with interruptions (rules «before_end»/«after_start»), solo turns between them,
  5 guests by rotation; writes clips and overlap.tsv, prints the share of overlapped words.
- `slots.py wav` — `<report>.slots/slot-N.{pcm,idx}` (16 kHz s16le + «wall_ms samples» per block) → wav per stream
  on a common clock + their sum (`mix.wav`) + clock.json.
- `score.py` (round 1, 3 guests) and `score2.py <dir> <recording created_at> [asr|fine]` (round 2): the app's norm/align
  (`common.py`, a port of quality::score); WER whole call (reference by clip start, heard words by segment start),
  words lost while 2+ spoke, per-speaker WER (words given to each guest), stream owner changes, and the speaker of each
  stream segment = the name Meet marks as speaking most (100 ms frames, weight 1/number lit) while that stream has
  sound (RMS > 300) inside the segment (`name_for`).
- Recognition: the eval stage «asr» (VAD; PROTOKOL_ASR_REDEMPTION_MS=400 for «fine»), original and tuned large-v3.
