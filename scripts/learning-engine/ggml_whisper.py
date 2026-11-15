"""The whisper.cpp model file (`ggml-large-v3.bin`) read into an MLX
Whisper model, and a copy of it with some weights replaced.

The trainer starts from the very file the app recognises with, so nothing
else has to be downloaded, and writes the new model by copying that file and
overwriting only the weights training changed. Everything else (header,
mel filters, vocabulary, the other weights) stays byte for byte the same.

The format, as whisper.cpp's convert-pt-to-ggml.py writes it:
  int32 magic 'ggml'; 11 int32 hyperparameters; int32 n_mel, n_fft and the
  mel filters (float32); int32 token count, then each token as int32
  length + bytes; then each tensor: int32 n_dims, name length, type
  (0 float32, 1 float16), the dimensions innermost first, the name, the data.
"""
import struct

import numpy as np

MAGIC = 0x67676D6C
HPARAMS = ["n_vocab", "n_audio_ctx", "n_audio_state", "n_audio_head", "n_audio_layer", "n_text_ctx", "n_text_state", "n_text_head", "n_text_layer", "n_mels", "ftype"]
DTYPES = {0: np.float32, 1: np.float16}


class Tensor:
    def __init__(self, name, shape, ftype, offset):
        self.name, self.shape, self.ftype, self.offset = name, shape, ftype, offset

    @property
    def dtype(self):
        return DTYPES[self.ftype]

    @property
    def nbytes(self):
        return int(np.prod(self.shape)) * np.dtype(self.dtype).itemsize


def read_index(path):
    """Hyperparameters and where each tensor's data is in the file"""
    with open(path, "rb") as f:
        def ints(n):
            return struct.unpack(f"<{n}i", f.read(4 * n))

        (magic,) = ints(1)
        if magic != MAGIC:
            raise ValueError("not a whisper.cpp model file")
        hp = dict(zip(HPARAMS, ints(len(HPARAMS))))
        n_mel, n_fft = ints(2)
        f.seek(4 * n_mel * n_fft, 1)
        (n_tokens,) = ints(1)
        for _ in range(n_tokens):
            (length,) = ints(1)
            f.seek(length, 1)
        tensors = {}
        while True:
            head = f.read(12)
            if len(head) < 12:
                break
            n_dims, name_len, ftype = struct.unpack("<3i", head)
            if ftype not in DTYPES:
                raise ValueError(f"tensor type {ftype} is not supported (a quantised model?)")
            dims = ints(n_dims)
            name = f.read(name_len).decode("utf-8")
            t = Tensor(name, tuple(reversed(dims)), ftype, f.tell())
            tensors[name] = t
            f.seek(t.nbytes, 1)
    return hp, tensors


def read_tensor(path, t):
    return np.fromfile(path, dtype=t.dtype, count=int(np.prod(t.shape)), offset=t.offset).reshape(t.shape)


def mlx_name(name):
    """OpenAI's names (as in the ggml file) to mlx-whisper's; None for what
    mlx-whisper computes itself"""
    if name == "encoder.positional_embedding":
        return None
    return name.replace(".mlp.0.", ".mlp1.").replace(".mlp.2.", ".mlp2.")


def load_model(path, dtype):
    """An mlx-whisper model with the weights of a whisper.cpp model file"""
    import mlx.core as mx
    from mlx.utils import tree_unflatten
    from whisper_mlx import model as whisper

    hp, tensors = read_index(path)
    dims = whisper.ModelDimensions(**{k: v for k, v in hp.items() if k != "ftype"})
    model = whisper.Whisper(dims, dtype)
    weights = []
    for name, t in tensors.items():
        target = mlx_name(name)
        if target is None:
            continue
        a = read_tensor(path, t)
        if name in ("encoder.conv1.bias", "encoder.conv2.bias"):
            a = a.reshape(-1)
        elif name in ("encoder.conv1.weight", "encoder.conv2.weight"):
            a = a.transpose(0, 2, 1)  # (out, in, kernel) → MLX's (out, kernel, in)
        weights.append((target, mx.array(a.astype(np.float32)).astype(dtype)))
    model.update(tree_unflatten(weights))
    mx.eval(model.parameters())
    return model, hp


def write_patched(base, out, replaced):
    """`out`: a copy of `base` with the tensors in `replaced` (name → numpy
    array of the same shape) overwritten, each in its own type in the file"""
    _, tensors = read_index(base)
    for name, a in replaced.items():
        if name not in tensors or tuple(a.shape) != tensors[name].shape:
            raise ValueError(f"{name}: no such tensor of shape {a.shape}")
    tmp = str(out) + ".part"
    with open(base, "rb") as src, open(tmp, "wb") as dst:
        while chunk := src.read(64 << 20):
            dst.write(chunk)
    with open(tmp, "r+b") as f:
        for name, a in replaced.items():
            t = tensors[name]
            f.seek(t.offset)
            f.write(np.ascontiguousarray(a, dtype=t.dtype).tobytes())
    import os

    os.replace(tmp, out)
