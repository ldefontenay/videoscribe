The voice-activity model Video Scribe embeds in its exe (src/vad.rs, include_bytes!).

Silero VAD v6.2.3, MIT licence, commit 5cd7945676eb32225748052e2e6a0580e4686a08
https://github.com/snakers4/silero-vad
https://raw.githubusercontent.com/snakers4/silero-vad/5cd7945676eb32225748052e2e6a0580e4686a08/src/silero_vad/data/silero_vad_16k_sequence.onnx

  silero_vad_16k_sequence.onnx   1,246,165 bytes
  SHA-256 9ccdacc4719d8aa7e45a77536bfabec45a03ba1f2fad5e241ab4060b24238a85

It is the only export tract 0.21 can run (spike 0(b), log entry 4; PLAN.md §4.1). The other exports
are in spike/models, git-ignored.

Copyright (c) 2020-present Silero Team. MIT licence:

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction,
including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial
portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT
LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN
NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
