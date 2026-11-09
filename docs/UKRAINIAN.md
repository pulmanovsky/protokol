# Протокол: what this fork adds to Meetily

This fork turns Meetily into «Протокол»: Ukrainian meeting protocols made
entirely on the user's Mac. How it works is described in
[architecture.md](architecture.md); this page lists what differs from the
Meetily base and where to find it.

| Area | Meetily | Протокол |
| --- | --- | --- |
| Language | Auto-detect, often translated to English | Ukrainian everywhere: recognition, interface, documents |
| Recognition | Whisper, no prompt | Whisper large-v3 with a prompt of confirmed names, roles and the dictionary; surzhyk and Russian spellings put into Ukrainian; learned corrections applied |
| Speakers | None | Voices recognised on the Mac, named once and then recognised by voice; «Я» from the microphone; short voices as «Репліки» |
| Document | Free-form summary, usually in English | Meeting types: «Загальна зустріч», «Щоденна нарада», «Контрольна нарада», «Стенограма», and the user's own types; who said what, decisions, tasks with owners and the moment in the recording |
| Corrections | None | Correct a line in the transcript; the app offers to remember the fix and applies it from then on |
| Privacy | Usage analytics; cloud LLM providers offered alongside local ones | No analytics; a local 12B model by default; windows hidden from screen sharing |
| Calendar | — | Reminders from an iCal link or an .ics file, recording starts with the event's title |

## Where things are

- Recognition and Ukrainian text: `frontend/src-tauri/src/whisper_engine/`,
  `frontend/src-tauri/src/ukrainian/` (prompt, orthography, hallucinations,
  dictionary, corrections and learning, DOCX/PDF export).
- Speakers: `frontend/src-tauri/src/speakers/`.
- Documents and meeting types: `frontend/src-tauri/src/summary/`
  (`processor.rs`, `materials.rs`, `templates/`), built-in types in
  `frontend/src-tauri/templates/`.
- Calendar: `frontend/src-tauri/src/calendar/`.
- Benchmarks: `scripts/bench/`.

## Next

- Exact word alignment (Whisper DTW) as an option, for speaker turns.
- Tasks: catch more promises in long reports.
- Export into an organisation's DOCX letterhead.
- Remove the unused cloud providers and the updater package from the build.
- Windows and Linux builds.
