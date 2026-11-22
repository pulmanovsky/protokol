# Privacy

Protokol is built to keep meetings on your computer.

**Stays on your Mac:** recordings, transcripts, speaker voices, the dictionary, corrections and documents. Recognition, speaker separation and the document are made by local models.

**No telemetry:** the app sends no usage data.

**Network use:**
- downloading the models on first run (from Hugging Face), unless they are installed from a folder;
- checking for updates once a day (on by default, can be turned off in Settings); the check asks GitHub for the latest version number and sends nothing about you;
- calendar reminders, only if you add a calendar address.

**Journal and problem reports:** the app keeps a technical journal on your Mac (in `~/Library/Logs/ua.protokol.app`, the last 7 days, at most 50 MB) and a short journal next to each recording. They hold no speech, transcript or document text, meeting titles, participant names or sound device names. Nothing is sent: when you choose «Повідомити про проблему…», the app gathers the journal into one zip file on your Desktop, shows every file before saving, and you pass it on yourself if you wish. The journal can be emptied in Settings → Конфіденційність.

You can delete any meeting and its files in the app.
