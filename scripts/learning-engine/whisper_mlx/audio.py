# Copyright © 2023 Apple Inc. From mlx-whisper 0.4.3 (MIT, see LICENSE): the
# log-Mel spectrogram only (no audio loading through ffmpeg).

import os
from functools import lru_cache
from typing import Union

import mlx.core as mx
import numpy as np

SAMPLE_RATE = 16000
N_FFT = 400
HOP_LENGTH = 160
CHUNK_LENGTH = 30
N_SAMPLES = CHUNK_LENGTH * SAMPLE_RATE  # 480000 samples in a 30-second chunk
N_FRAMES = N_SAMPLES // HOP_LENGTH  # 3000 frames in a mel spectrogram input


@lru_cache(maxsize=None)
def mel_filters(n_mels: int) -> mx.array:
    """
    load the mel filterbank matrix for projecting STFT into a Mel spectrogram.
    Allows decoupling librosa dependency; saved using:

        np.savez_compressed(
            "mel_filters.npz",
            mel_80=librosa.filters.mel(sr=16000, n_fft=400, n_mels=80),
            mel_128=librosa.filters.mel(sr=16000, n_fft=400, n_mels=128),
        )
    """
    assert n_mels in {80, 128}, f"Unsupported n_mels: {n_mels}"

    filename = os.path.join(os.path.dirname(__file__), "assets", "mel_filters.npz")
    return mx.load(filename)[f"mel_{n_mels}"]


@lru_cache(maxsize=None)
def hanning(size):
    return mx.array(np.hanning(size + 1)[:-1])


def stft(x, window, nperseg=256, noverlap=None, nfft=None, axis=-1, pad_mode="reflect"):
    if nfft is None:
        nfft = nperseg
    if noverlap is None:
        noverlap = nfft // 4

    def _pad(x, padding, pad_mode="constant"):
        if pad_mode == "constant":
            return mx.pad(x, [(padding, padding)])
        elif pad_mode == "reflect":
            prefix = x[1 : padding + 1][::-1]
            suffix = x[-(padding + 1) : -1][::-1]
            return mx.concatenate([prefix, x, suffix])
        else:
            raise ValueError(f"Invalid pad_mode {pad_mode}")

    padding = nperseg // 2
    x = _pad(x, padding, pad_mode)

    strides = [noverlap, 1]
    t = (x.size - nperseg + noverlap) // noverlap
    shape = [t, nfft]
    x = mx.as_strided(x, shape=shape, strides=strides)
    return mx.fft.rfft(x * window)


def log_mel_spectrogram(
    audio: Union[np.ndarray, mx.array],
    n_mels: int = 80,
    padding: int = 0,
):
    """
    Compute the log-Mel spectrogram of

    Parameters
    ----------
    audio: Union[str, np.ndarray, mx.array], shape = (*)
        The path to audio or either a NumPy or mlx array containing the audio waveform in 16 kHz

    n_mels: int
        The number of Mel-frequency filters, only 80 is supported

    padding: int
        Number of zero samples to pad to the right

    Returns
    -------
    mx.array, shape = (80, n_frames)
        An  array that contains the Mel spectrogram
    """
    if not isinstance(audio, mx.array):
        audio = mx.array(audio)

    if padding > 0:
        audio = mx.pad(audio, (0, padding))
    window = hanning(N_FFT)
    freqs = stft(audio, window, nperseg=N_FFT, noverlap=HOP_LENGTH)
    magnitudes = freqs[:-1, :].abs().square()

    filters = mel_filters(n_mels)
    mel_spec = magnitudes @ filters.T

    log_spec = mx.maximum(mel_spec, 1e-10).log10()
    log_spec = mx.maximum(log_spec, log_spec.max() - 8.0)
    log_spec = (log_spec + 4.0) / 4.0
    return log_spec
