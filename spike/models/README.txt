Models for the spikes. The .onnx files are git-ignored; fetch them again from the pinned commit.

Silero VAD v6.2.3, MIT licence, commit 5cd7945676eb32225748052e2e6a0580e4686a08
https://raw.githubusercontent.com/snakers4/silero-vad/5cd7945676eb32225748052e2e6a0580e4686a08/src/silero_vad/data/<file>

The one that runs on tract 0.21 (spike 0(b), log entry 4):
  silero_vad_16k_sequence.onnx   1,246,165 bytes
  9ccdacc4719d8aa7e45a77536bfabec45a03ba1f2fad5e241ab4060b24238a85

Tried and refused by tract (If nodes with mismatched branch shapes):
  silero_vad_16k_op15.onnx       7ed98ddbad84ccac4cd0aeb3099049280713df825c610a8ed34543318f1b2c49
  silero_vad_op18_ifless.onnx    7671cd04b004e9076da0d4a7b1a5aec36adf161c39230c1cb94a4fd5db6bbd28
  silero_vad_half.onnx, silero_vad.onnx
