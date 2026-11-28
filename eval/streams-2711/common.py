"""Shared pieces: the app's norm/align/queued (quality/score.rs), Parakeet line-up
(audio/transcription/ensemble.rs) and sound_of (ukrainian/doubt.rs), ported 1:1."""
import json, math, re

FUNCTION_WORDS = {"й", "і", "у", "в", "з", "а", "та", "чи", "же", "ж", "би", "б", "не", "ні", "що", "як", "це"}


def _trim(s, keep):
    i, j = 0, len(s)
    while i < j and not keep(s[i]):
        i += 1
    while j > i and not keep(s[j - 1]):
        j -= 1
    return s[i:j]


def norm(word):
    w = _trim(word, lambda c: c.isalnum() or c in "'’ʼ-")
    w = w.strip("-")
    for c in "’ʼ`":
        w = w.replace(c, "'")
    return w.lower()


def bare(w):
    return _trim(w, lambda c: c.isalnum() or c in "'’-")


def queued(word, p):
    return norm(word) not in FUNCTION_WORDS or p < 0.2


def shared_start(a, b):
    n = 0
    for x, y in zip(a, b):
        if x != y:
            break
        n += 1
    return n


def align(ref, hyp):
    """quality::score::align: returns (correct[m], missing_at list)"""
    n, m = len(ref), len(hyp)
    d = [[(0, 0, 0)] * (m + 1) for _ in range(n + 1)]
    for i in range(n + 1):
        d[i][0] = (i, 0, 0)
    for j in range(m + 1):
        d[0][j] = (j, 0, 0)

    def diag(i, j):
        e, r, s = d[i - 1][j - 1]
        if ref[i - 1] == hyp[j - 1]:
            return (e, r - 1, s)
        return (e + 1, r, s - shared_start(ref[i - 1], hyp[j - 1]))

    for i in range(1, n + 1):
        for j in range(1, m + 1):
            u = d[i - 1][j]; l = d[i][j - 1]
            d[i][j] = min(diag(i, j), (u[0] + 1, u[1], u[2]), (l[0] + 1, l[1], l[2]))
    correct = [False] * m
    missing_at = []
    wrong_pairs = []
    i, j = n, m
    while i > 0 or j > 0:
        if i > 0 and j > 0 and d[i][j] == diag(i, j):
            if ref[i - 1] == hyp[j - 1]:
                correct[j - 1] = True
            else:
                wrong_pairs.append((i - 1, j - 1))
            i -= 1; j -= 1
        elif j > 0 and d[i][j] == (d[i][j - 1][0] + 1, d[i][j - 1][1], d[i][j - 1][2]):
            j -= 1
        else:
            missing_at.append(j)
            i -= 1
    missing_at.reverse(); wrong_pairs.reverse()
    return correct, missing_at, wrong_pairs


def sound_key(word):
    out = ""
    for c in norm(word):
        c = {"і": "и", "ї": "и", "ы": "и", "є": "е", "э": "е", "ё": "е", "ґ": "г"}.get(c, c)
        if c in "'ь ъ-".replace(" ", ""):
            continue
        if out.endswith(c):
            continue
        out += c
    return out


def line_up(a, b):
    n, m = len(a), len(b)
    d = [[(0, 0)] * (m + 1) for _ in range(n + 1)]
    for i in range(n + 1):
        d[i][0] = (i, 0)
    for j in range(m + 1):
        d[0][j] = (j, 0)

    def diag(i, j):
        e, r = d[i - 1][j - 1]
        return (e, r - 1) if a[i - 1] == b[j - 1] else (e + 1, r)

    for i in range(1, n + 1):
        for j in range(1, m + 1):
            d[i][j] = min(diag(i, j), (d[i - 1][j][0] + 1, d[i - 1][j][1]), (d[i][j - 1][0] + 1, d[i][j - 1][1]))
    steps = []
    i, j = n, m
    while i > 0 or j > 0:
        if i > 0 and j > 0 and d[i][j] == diag(i, j):
            steps.append(("same" if a[i - 1] == b[j - 1] else "differ", i - 1, j - 1)); i -= 1; j -= 1
        elif i > 0 and d[i][j] == (d[i - 1][j][0] + 1, d[i - 1][j][1]):
            steps.append(("first", i - 1, None)); i -= 1
        else:
            steps.append(("second", None, j - 1)); j -= 1
    steps.reverse()
    return steps


def compare(first, second):
    """ensemble::compare plus Parakeet's time for each matched Whisper word"""
    keep_a = [i for i in range(len(first)) if sound_key(first[i])]
    keep_b = [j for j in range(len(second)) if sound_key(second[j][0])]
    a = [sound_key(first[i]) for i in keep_a]
    b = [sound_key(second[j][0]) for j in keep_b]
    other = [None] * len(first); same = [False] * len(first); oprob = [0.0] * len(first)
    otime = [None] * len(first); oidx = [None] * len(first)
    for i in range(len(first)):
        if i not in keep_a:
            same[i] = True; oprob[i] = 1.0
    only_other = []
    next_a = 0
    for kind, x, y in line_up(a, b):
        if kind in ("same", "differ"):
            i, j = keep_a[x], keep_b[y]
            other[i] = second[j][0]; same[i] = kind == "same"; oprob[i] = second[j][2]
            otime[i] = second[j][1]; oidx[i] = j
            next_a = i + 1
        elif kind == "first":
            next_a = keep_a[x] + 1
        else:
            w = second[keep_b[y]]
            only_other.append((next_a, w[0], w[2]))
    return dict(other=other, same=same, oprob=oprob, only_other=only_other, otime=otime, oidx=oidx)


def sound_of(word):
    lower = bare(word).lower()
    s = ""
    i = 0
    single = {'a': "а", 'b': "б", 'v': "в", 'w': "в", 'g': "г", 'h': "г", 'ґ': "г", 'х': "г", 'd': "д", 'e': "е", 'є': "е", 'э': "е",
              'z': "з", 'i': "і", 'y': "і", 'и': "і", 'ї': "і", 'й': "і", 'ы': "і", 'k': "к", 'q': "к", 'l': "л", 'm': "м", 'n': "н",
              'o': "о", 'p': "п", 'r': "р", 's': "с", 't': "т", 'u': "у", 'f': "ф", 'c': "к", 'j': "й", 'x': "кс",
              'ь': "", "'": "", '’': "", 'ъ': "", '-': "", 'ю': "у", 'я': "а", 'щ': "ш"}
    while i < len(lower):
        c = lower[i]; nx = lower[i + 1] if i + 1 < len(lower) else None
        if c == 's' and nx == 'h':
            s += "ш"; i += 2; continue
        if c == 't' and nx == 's':
            s += "ц"; i += 2; continue
        if c == 'c' and nx == 'h':
            s += "ч"; i += 2; continue
        if c == 'z' and nx == 'h':
            s += "ж"; i += 2; continue
        if c == 'k' and nx == 'h':
            s += "г"; i += 2; continue
        s += single.get(c, c)
        i += 1
    out = ""
    for c in s:
        if not out.endswith(c):
            out += c
    return out


def levenshtein(a, b):
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a):
        cur = [i + 1]
        for j, cb in enumerate(b):
            cur.append(min(prev[j] + (ca != cb), prev[j + 1] + 1, cur[j] + 1))
        prev = cur
    return prev[len(b)]


def load(path):
    return [json.loads(l) for l in open(path)]
