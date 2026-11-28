"""Separate Meet streams vs mixed recording on the 27.11 guest call. Numbers to stdout and results.json.
Ground truth: call/timeline.jsonl (what each guest played, when). Same norm/align as the app rescore
(../detector-2711/common.py). Each source's time offset to the wall clock is found by best word match."""
import json, sys, os, wave, array, collections
from datetime import datetime
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import norm, align

wall = lambda iso: datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp()
clock = json.load(open("call/wav/clock.json")); T0 = clock["t0_ms"] / 1000  # slot wavs start
REC0 = wall("2026-11-27T20:27:08.339621+00:00")  # the app's recording (metadata created_at)
clips = [json.loads(l) for l in open("call/timeline.jsonl")]
clips = [dict(c, a=wall(c["from"]), b=wall(c["from"]) + c["seconds"]) for c in clips if wall(c["from"]) >= T0]
turns = collections.OrderedDict()
for c in clips:
    turns.setdefault(c["turn"], []).append(c)


def words(t):
    return [n for n in (norm(x) for x in t.split()) if n]


def segs_eval(path, base):
    return [dict(a=base + s["start_ms"] / 1000, b=base + s["end_ms"] / 1000, text=s["text"]) for s in json.load(open(path))]


def best_offset(segs):
    """shift (s) that best lines segment starts up with clip starts (median of nearest gaps, refined twice)"""
    d = 0.0
    for _ in range(3):
        gaps = sorted(min((c["a"] - (x["a"] + d) for c in clips), key=abs) + d for x in segs)
        d = gaps[len(gaps) // 2]
    return d


def wer(ref, hyp):
    correct, missing, _ = align(ref, hyp)
    errs = (len(hyp) - sum(correct)) + len(missing)
    return errs


def turn_wer(segs):
    """per turn: words of segments whose middle falls in the turn's span vs the turn's clips in start order"""
    out = {}
    for t, cs in turns.items():
        a, b = min(c["a"] for c in cs) - 1, max(c["b"] for c in cs) + 2
        hyp = [w for s in sorted(segs, key=lambda s: s["a"]) if a <= (s["a"] + s["b"]) / 2 <= b for w in words(s["text"])]
        ref = [w for c in sorted(cs, key=lambda c: c["a"]) for w in words(c["text"])]
        out[t] = (wer(ref, hyp), len(ref), cs[0]["kind"])
    return out


def summarize(name, tw):
    r = {}
    for kind in ("normal", "quiet", "short", "overlap", "all"):
        xs = [v for v in tw.values() if kind == "all" or v[2] == kind]
        if xs:
            r[kind] = round(sum(e for e, _, _ in xs) / sum(n for _, n, _ in xs), 4)
    print(f"{name:34s} " + "  ".join(f"{k} {100*v:.1f}%" for k, v in r.items()))
    return r


res = {"clips": len(clips), "turns": len(turns), "speech_s": sum(c["seconds"] for c in clips)}
# ---- 1) capture: which slot carries each clip (energy), stable per guest?
A = {}
for i in clock["slots"]:
    w = wave.open(f"call/wav/slot-{i}.wav"); A[i] = array.array("h", w.readframes(w.getnframes()))
def rms(i, a, b):
    s = A[i][max(0, int((a - T0) * 16000)):int((b - T0) * 16000)][::20]
    return (sum(x * x for x in s) / max(1, len(s))) ** .5
carry = collections.defaultdict(collections.Counter)
for c in clips:
    # the guest's own part only (overlaps: the part before the other one starts, or the whole when it starts later)
    e = {i: rms(i, c["a"] + 0.2, c["b"] - 0.2) for i in A}
    top = max(e, key=e.get)
    carry[c["guest"]][top] += 1
slot_of = {g: cnt.most_common(1)[0][0] for g, cnt in carry.items()}
res["guest_to_slot"] = {g: dict(cnt) for g, cnt in carry.items()}
print("guest -> slot (clips):", {g: dict(c) for g, c in carry.items()})
# naming each slot from Meet's own speaking marks (report «lit» frames) while that slot has sound
reps = [json.loads(l) for l in open("call/report.jsonl")]
name_votes = collections.defaultdict(collections.Counter)
for r in reps:
    for t_ms, lit in zip(r.get("ft", []), r.get("lit", [])):
        t = t_ms / 1000
        if t < T0 or not lit:
            continue
        loud = [i for i in A if rms(i, t, t + 0.1) > 300]
        for i in loud:
            for k in lit:
                name_votes[i][r["names"][k] if k < len(r["names"]) else "?"] += 1
slot_name = {i: v.most_common(1)[0][0] for i, v in name_votes.items()}
res["slot_name_votes"] = {i: dict(v.most_common(4)) for i, v in name_votes.items()}
res["slot_name"] = slot_name
ok = all(slot_name.get(slot_of[g]) == f"Гість {g}" for g in slot_of)
print("slot names from Meet marks:", slot_name, "| all right:", ok)
res["names_right"] = ok

# ---- 2) recognition
rows = {}
for model in ("orig", "tuned"):
    for src in ("system", "mix"):
        segs = segs_eval(f"call/asr-{model}-{src}/segments.json", REC0 if src == "system" else T0)
        d = best_offset(segs); segs = [dict(s, a=s["a"] + d, b=s["b"] + d) for s in segs]
        rows[f"{model}: mixed {'(app recording, call audio)' if src == 'system' else '(sum of the 3 streams)'}"] = summarize(f"{model} mixed {src} (shift {d:+.2f}s)", turn_wer(segs))
    # streams: each slot recognised alone, then put together by time
    allseg = []
    for i in clock["slots"]:
        s = segs_eval(f"call/asr-{model}-slot-{i}/segments.json", T0)
        for x in s:
            x["slot"] = i
        allseg += s
    d = best_offset(allseg); allseg = [dict(s, a=s["a"] + d, b=s["b"] + d) for s in allseg]
    rows[f"{model}: separate streams"] = summarize(f"{model} streams (shift {d:+.2f}s)", turn_wer(allseg))
    # per clip from its guest's own stream only (overlaps: each voice apart)
    errs = n = 0; ov_e = ov_n = 0
    for c in clips:
        hyp = [w for s in sorted(allseg, key=lambda s: s["a"]) if s["slot"] == slot_of[c["guest"]] and c["a"] - 1 <= (s["a"] + s["b"]) / 2 <= c["b"] + 2 for w in words(s["text"])]
        e = wer(words(c["text"]), hyp)
        if c["kind"] == "overlap":
            ov_e += e; ov_n += len(words(c["text"]))
        errs += e; n += len(words(c["text"]))
    rows[f"{model}: separate streams, each voice apart"] = {"all": round(errs / n, 4), "overlap": round(ov_e / ov_n, 4)}
    print(f"{model} streams, each clip from own stream: all {100*errs/n:.1f}%, overlap clips {100*ov_e/ov_n:.1f}%")
    # wrong speaker: stream segments named by the slot's Meet name vs who really spoke
    wrong = tot = 0
    for s in allseg:
        mid = (s["a"] + s["b"]) / 2
        true = [c for c in clips if c["a"] - 0.5 <= mid <= c["b"] + 1.5]
        if not true:
            continue
        tot += 1
        wrong += all(slot_name.get(s["slot"]) != f"Гість {c['guest']}" for c in true)
    rows[f"{model}: separate streams, lines to wrong speaker"] = {"lines": tot, "wrong": wrong}
    print(f"{model} streams: lines given to the wrong speaker {wrong} of {tot}")
res["wer"] = rows

# app's own mixed transcript (diarization + Meet names), as saved by the test copy
lines = []
for l in open("app/db-lines.txt"):
    a, b, spk, _ = l.rstrip("\n").split("|", 3)
    lines.append(dict(a=REC0 + float(a), b=REC0 + float(b), spk=spk))
full = json.load(open("app/transcripts.json"))["segments"]
appsegs = [dict(a=REC0 + s["audio_start_time"], b=REC0 + s["audio_end_time"], text=s["text"]) for s in full]
d = best_offset(appsegs)
rows["app as is: mixed + its own speakers"] = summarize(f"app transcript (shift {d:+.2f}s)", turn_wer([dict(s, a=s["a"] + d, b=s["b"] + d) for s in appsegs]))
wrong = tot = 0; detail = []
for x in lines:
    mid = (x["a"] + x["b"]) / 2 + d
    true = [c for c in clips if c["a"] - 0.5 <= mid <= c["b"] + 1.5]
    if not true:
        continue
    tot += 1
    bad = all(not x["spk"].startswith(f"Гість {c['guest']}") for c in true)
    wrong += bad
    detail.append(dict(turn=true[0]["turn"], kind=true[0]["kind"], said=[c["guest"] for c in true], label=x["spk"], wrong=bad))
res["app_speakers"] = dict(lines=tot, wrong=wrong, detail=detail)
print(f"app mixed transcript: lines given to the wrong speaker {wrong} of {tot}; marked unsure «(?)»: {sum('(?)' in x['spk'] for x in lines)}")
json.dump(res, open("results.json", "w"), indent=1, ensure_ascii=False)
