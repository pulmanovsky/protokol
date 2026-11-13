# Third-party notices

Model files used by «Протокол» that are not part of its source code. They
run locally; nothing is sent over the network.

## Word timing (where each word sounds)

### Yehor/w2v-xls-r-uk (Ukrainian wav2vec2 XLS-R)

- Use: placing each recognised word at the moment it sounds in the recording
  (CTC forced alignment), so the transcript is highlighted in step with the
  audio. Shipped with the app as `aligner-model/`.
- Author: Yehor Smoliakov; fine-tuned from Meta AI's
  `facebook/wav2vec2-xls-r-300m` (Apache-2.0).
- Source: https://huggingface.co/Yehor/w2v-xls-r-uk
- Licence: Apache License 2.0, https://www.apache.org/licenses/LICENSE-2.0 .
- Modifications: converted to ONNX and quantised to int8 (dynamic
  quantisation) for local use; the vocabulary is unchanged.

```
Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

## Speaker recognition (who spoke when)

### pyannote segmentation-3.0

- Use: finding where speakers talk and change (speaker diarization).
- Authors: Hervé Bredin, Alexis Plaquet (pyannote, CNRS, France).
- Source: https://huggingface.co/pyannote/segmentation-3.0 ; ONNX export by
  k2-fsa: https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-segmentation-models
  (`sherpa-onnx-pyannote-segmentation-3-0`).
- Licence: MIT.

```
MIT License

Copyright (c) 2022 CNRS

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Voice embedding model (one of the two; the bundled one is named in the release notes)

**WeSpeaker ResNet34-LM (VoxCeleb)**

- Use: a voice "fingerprint" per piece of speech, to tell voices apart and
  recognise named voices in later meetings.
- Authors: the WeSpeaker project (WeNet open-source community).
- Source: https://github.com/wenet-e2e/wespeaker (docs/pretrained.md); ONNX
  export by k2-fsa: https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models
  (`wespeaker_en_voxceleb_resnet34_LM.onnx`).
- Training data: VoxCeleb1/2.
- Licence: Creative Commons Attribution 4.0 International (CC BY 4.0),
  https://creativecommons.org/licenses/by/4.0/ . Used unmodified.

**NVIDIA NeMo TitaNet-small**

- Use: as above.
- Authors: NVIDIA.
- Source: https://catalog.ngc.nvidia.com/orgs/nvidia/teams/nemo/models/titanet_small ;
  ONNX export by k2-fsa (`nemo_en_titanet_small.onnx`).
- Licence: Creative Commons Attribution 4.0 International (CC BY 4.0),
  https://creativecommons.org/licenses/by/4.0/ . Used unmodified.

## Speech recognition

### Whisper large-v3 (whisper.cpp format)

- Downloaded on first run. Model: OpenAI Whisper, MIT. Runtime: whisper.cpp, MIT.

### NVIDIA Parakeet TDT 0.6B (optional)

- Downloaded only when chosen in Settings, from
  https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx (ONNX export of
  `nvidia/parakeet-tdt-0.6b-v3`). Licence: Creative Commons Attribution 4.0
  International (CC BY 4.0), https://creativecommons.org/licenses/by/4.0/ .

## Document

### MamayLM Gemma 3 12B

- Downloaded on first run from https://huggingface.co/INSAIT-Institute . Built on Gemma 3 and subject to the Gemma Terms of Use, https://ai.google.dev/gemma/terms .

## Google Meet window

### Vexa

- Use: the script that runs inside a Google Meet page opened in the app's own
  window (`frontend/src-tauri/src/meet/probe.js`) finds each participant's
  audio stream and the participant tile lit as speaking. Parts of it are
  adapted from Vexa's `core/meetings/modules/gmeet-capture`
  (`gmeet-capture.ts`, `gmeet-speakers.ts`, `pcm-capture.ts`).
- Copyright 2025-2026 Vexa.ai Inc. and the Vexa contributors.
- Source: https://github.com/Vexa-ai/vexa
- Licence: Apache License 2.0, https://www.apache.org/licenses/LICENSE-2.0 .
- Modifications: one shared audio context with a loudness meter per stream
  instead of copying the audio, tile classes recorded for analysis, reports
  handed to the app on the same Mac instead of a network connection.

```
Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

## FFmpeg

The macOS app carries an `ffmpeg` program (Contents/MacOS/ffmpeg) that
records to .mp4/.m4a and reads imported files. It is FFmpeg 8.1.3, built from
the official source without GPL parts and without external libraries (it
uses only macOS system libraries), so it is licensed under the GNU Lesser General Public
License, version 2.1 or later (Contents/Resources/licenses/FFmpeg-LGPL-2.1.txt
inside the app).

- Source: each release carries `ffmpeg-8.1.3.tar.xz`, the unchanged source
  archive from https://ffmpeg.org/releases/ , and the build is made by
  `frontend/scripts/build-ffmpeg-macos.sh` in this repository (the full
  configuration is there).
- The program runs as a separate process; it can be replaced with another
  FFmpeg build of the same name.
- FFmpeg is a trademark of Fabrice Bellard, originator of the FFmpeg project.

## Libraries

The app is built with whisper.cpp and llama.cpp (MIT), ONNX Runtime (MIT),
Tauri (MIT or Apache-2.0), Next.js and React (MIT) and the other Rust and
JavaScript packages listed in `Cargo.lock` and `frontend/pnpm-lock.yaml`,
each under its own licence.
