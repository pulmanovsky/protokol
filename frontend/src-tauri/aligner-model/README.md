# Word aligner model

Bundled with the app (not kept in git): `w2v-uk-int8.onnx` and `vocab.json`,
the Ukrainian speech model [Yehor/w2v-xls-r-uk](https://huggingface.co/Yehor/w2v-xls-r-uk)
(Apache-2.0) converted to ONNX and quantised to int8. It places each
transcribed word where it sounds (CTC forced alignment). Without these
files the app keeps the recogniser's own word times.

The app looks first in `<app data>/models/aligner/`, then here.
