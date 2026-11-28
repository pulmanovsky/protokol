# Separate Meet streams vs the mix, on a guest call recorded by the «Слухач» guest.
#   python3 -I slots.py wav        listen/slots/slot-N.{pcm,idx} -> listen/wav/slot-N.wav + mix.wav (common wall clock)
#   python3 -I slots.py score      after eval stage «asr» on each wav (listen/asr-<name>/segments.json):
#                                  per spoken clip (timeline.jsonl): words heard in the mix vs in the clip's own stream
import json, os, sys, struct, wave, glob, re
from datetime import datetime
D = os.path.dirname(os.path.abspath(__file__)) + "/"
L = os.environ.get("SLOTS_OUT", D + "listen/")
SR = 16000
SL = os.environ.get("SLOTS_IN", L + "slots/")
TL = os.environ.get("TIMELINE", D + "timeline.jsonl")

def wall(iso):
    return datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp() * 1000

def load_slot(i):
    pcm = open(f"{SL}slot-{i}.pcm", "rb").read()
    blocks = [tuple(map(int, l.split())) for l in open(f"{SL}slot-{i}.idx")]
    return pcm, blocks

def make_wavs():
    slots = sorted(int(re.search(r"slot-(\d+)\.idx", p).group(1)) for p in glob.glob(SL + "slot-*.idx"))
    data = {i: load_slot(i) for i in slots}
    t0 = min(b[0][0] for _, b in data.values())
    t1 = max(b[-1][0] + b[-1][1] * 1000 / SR for _, b in data.values())
    n = int((t1 - t0) * SR / 1000) + SR
    os.makedirs(L + "wav", exist_ok=True)
    mix = [0] * n
    for i, (pcm, blocks) in data.items():
        out = [0] * n; off = 0
        for t, k in blocks:
            s = int((t - t0) * SR / 1000)
            vals = struct.unpack(f"<{k}h", pcm[off * 2:(off + k) * 2]); off += k
            for j, v in enumerate(vals):
                if 0 <= s + j < n:
                    out[s + j] = v
        for j in range(n):
            mix[j] += out[j]
        write(f"{L}wav/slot-{i}.wav", out)
        print(f"slot {i}: {len(blocks)} blocks, {off / SR:.0f} s of sound")
    write(f"{L}wav/mix.wav", [max(-32768, min(32767, v)) for v in mix])
    json.dump({"t0_ms": t0, "slots": slots}, open(L + "wav/clock.json", "w"))
    print(f"timeline {n / SR:.0f} s from {t0}")

def write(path, samples):
    with wave.open(path, "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(SR)
        w.writeframes(struct.pack(f"<{len(samples)}h", *samples))

def norm(w):
    w = re.sub(r"[^\w'’ʼ-]", "", w).strip("-").replace("’", "'").replace("ʼ", "'")
    return w.lower()

def words_in(segs, a, b):
    out = []
    for s in segs:
        st, en = s["start_ms"] / 1000, s["end_ms"] / 1000
        if en > a and st < b:
            out += [norm(x) for x in s["text"].split() if norm(x)]
    return out

def score():
    import difflib
    clock = json.load(open(L + "wav/clock.json"))
    seg = {}
    for name in ["mix"] + [f"slot-{i}" for i in clock["slots"]]:
        p = f"{L}asr-{name}/segments.json"
        if os.path.exists(p):
            seg[name] = json.load(open(p))
    tl = [json.loads(l) for l in open(TL)]
    tl = [r for r in tl if wall(r["from"]) >= clock["t0_ms"] - 1000]
    # which slot carried each guest: the slot with the most sound during that guest's solo clips
    rows = []
    for r in tl:
        a = (wall(r["from"]) - clock["t0_ms"]) / 1000 - 0.5
        b = a + r["seconds"] + 1.5
        ref = [norm(x) for x in r["text"].split() if norm(x)]
        def rec(h):
            m = difflib.SequenceMatcher(None, ref, h, autojunk=False)
            return sum(x.size for x in m.get_matching_blocks()) / max(len(ref), 1)
        best = max((k for k in seg if k != "mix"), key=lambda k: rec(words_in(seg[k], a, b)), default=None)
        rows.append({"turn": r["turn"], "guest": r["guest"], "kind": r["kind"], "words": len(ref),
                     "mix": rec(words_in(seg.get("mix", []), a, b)), "stream": rec(words_in(seg[best], a, b)) if best else None, "slot": best})
    json.dump(rows, open(L + "score.json", "w"), indent=1)
    for kind in ("normal", "quiet", "short", "overlap"):
        rs = [x for x in rows if x["kind"] == kind]
        if not rs: continue
        w = sum(x["words"] for x in rs)
        print(f"{kind:8} clips {len(rs):2}  words {w:4}  found in mix {sum(x['mix'] * x['words'] for x in rs) / w:.1%}  in own stream {sum((x['stream'] or 0) * x['words'] for x in rs) / w:.1%}")
    slots = {}
    for x in rows: slots.setdefault(x["guest"], {}).setdefault(x["slot"], 0); slots[x["guest"]][x["slot"]] += 1
    print("guest -> stream:", slots)

if __name__ == "__main__":
    {"wav": make_wavs, "score": score}[sys.argv[1]]()
