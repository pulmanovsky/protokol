"""Round 2 (5 guests over Meet's 3 streams, frequent overlaps): streams vs mixed recording.
Usage: python3 score2.py <round dir> <recording created_at ISO>
<dir>/call: wav/slot-N.wav + clock.json, asr-<model>-<src>/segments.json, report.jsonl, timeline.jsonl
<dir>/app: transcripts.json, db-lines.txt. Numbers to stdout and <dir>/results.json."""
import json, sys, os, wave, array, collections
from datetime import datetime
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import norm, align

R = sys.argv[1]; C = R + "/call/"
wall = lambda iso: datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp()
clock = json.load(open(C + "wav/clock.json")); T0 = clock["t0_ms"] / 1000
REC0 = wall(sys.argv[2])
clips = [json.loads(l) for l in open(C + "timeline.jsonl")]
import os
DROP = {int(x) for x in os.environ.get("DROP_GUESTS", "").split(",") if x}
clips = [dict(c, a=wall(c["from"]), b=wall(c["from"]) + c["seconds"], id=k) for k, c in enumerate(clips) if wall(c["from"]) >= T0 and c["guest"] not in DROP]
turns = collections.OrderedDict()
for c in clips:
    turns.setdefault(c["turn"], []).append(c)
words = lambda t: [n for n in (norm(x) for x in t.split()) if n]
GUESTS = sorted(set(c["guest"] for c in clips))

# which reference words were spoken while someone else spoke too (words spread evenly over their clip)
for c in clips:
    ws = words(c["text"]); n = len(ws)
    c["ov"] = [any(o is not c and o["a"] <= c["a"] + (c["b"] - c["a"]) * (i + .5) / n <= o["b"] for o in clips) for i in range(n)]
res = {"clips": len(clips), "turns": len(turns), "words": sum(len(c["ov"]) for c in clips), "overlapped_words": sum(sum(c["ov"]) for c in clips)}
t = min(c["a"] for c in clips); end = max(c["b"] for c in clips); one = two = 0
while t < end:
    k = sum(1 for c in clips if c["a"] <= t <= c["b"]); one += k >= 1; two += k >= 2; t += 0.05
res.update(speech_s=round(one * .05, 1), overlap_s=round(two * .05, 1))
print(f"{res['clips']} clips, {res['words']} words, overlapped words {res['overlapped_words']} ({100*res['overlapped_words']/res['words']:.1f}%), speech {res['speech_s']} s, 2+ at once {res['overlap_s']} s")


def segs_eval(path, base):
    return [dict(a=base + s["start_ms"] / 1000, b=base + s["end_ms"] / 1000, text=s["text"]) for s in json.load(open(path))]


def best_offset(segs):
    d = 0.0
    for _ in range(3):
        gaps = sorted(min((c["a"] - (x["a"] + d) for c in clips), key=abs) + d for x in segs)
        d = gaps[len(gaps) // 2]
    return d


def ref_flags(ref, hyp):
    """for each reference word: recognised right (symmetric alignment, reference as the 'heard' side)"""
    correct, _, _ = align(hyp, ref)
    return correct


def score_turns(segs):
    """whole call: all reference words by clip start vs all heard words by segment start (VAD pieces are short)"""
    order = sorted(clips, key=lambda c: c["a"])
    ref = [w for c in order for w in words(c["text"])]
    ov = [f for c in order for f in c["ov"]]
    hyp = [w for s in sorted(segs, key=lambda s: s["a"]) for w in words(s["text"])]
    correct, missing, _ = align(ref, hyp)
    e = (len(hyp) - sum(correct)) + len(missing)
    rf = ref_flags(ref, hyp)
    ov_err = sum(1 for f, r in zip(ov, rf) if f and not r)
    solo_err = sum(1 for f, r in zip(ov, rf) if not f and not r)
    return dict(all=round(e / len(ref), 4), overlapped_words_missed=round(ov_err / max(1, sum(ov)), 4),
                solo_words_missed=round(solo_err / max(1, len(ov) - sum(ov)), 4), heard_words=len(hyp))


def per_speaker(labelled):
    """[(a, b, text, label)]: for each guest, his reference words vs the words given to him"""
    tot_e = tot_n = 0; out = {}
    for g in GUESTS:
        ref = [w for c in sorted(clips, key=lambda c: c["a"]) if c["guest"] == g for w in words(c["text"])]
        hyp = [w for a, b, t, lab in sorted(labelled) if lab == f"Гість {g}" for w in words(t)]
        correct, missing, _ = align(ref, hyp)
        e = (len(hyp) - sum(correct)) + len(missing)
        out[g] = round(e / len(ref), 4); tot_e += e; tot_n += len(ref)
    return dict(all=round(tot_e / tot_n, 4), per_guest=out)


# ---- capture: which stream carries each clip, how often a stream changes owner
A = {}
for i in clock["slots"]:
    w = wave.open(f"{C}wav/slot-{i}.wav"); A[i] = array.array("h", w.readframes(w.getnframes()))
def rms(i, a, b):
    s = A[i][max(0, int((a - T0) * 16000)):max(0, int((b - T0) * 16000))][::20]
    return (sum(x * x for x in s) / max(1, len(s))) ** .5
def solo_parts(c):
    """pieces of a clip with nobody else speaking"""
    t, out, cur = c["a"], [], None
    while t < c["b"]:
        alone = not any(o is not c and o["a"] <= t <= o["b"] for o in clips)
        if alone and cur is None: cur = t
        if not alone and cur is not None: out.append((cur, t)); cur = None
        t += 0.1
    if cur is not None: out.append((cur, c["b"]))
    return [(a, b) for a, b in out if b - a >= 0.5]
carrier = {}
for c in clips:
    parts = solo_parts(c) or [(c["a"], c["b"])]
    e = {i: sum(rms(i, a, b) * (b - a) for a, b in parts) for i in A}
    top = max(e, key=e.get)
    carrier[c["id"]] = top if e[top] > 200 * sum(b - a for a, b in parts) else None
missing = [c["id"] for c in clips if carrier[c["id"]] is None]
owners = collections.defaultdict(list)
for c in sorted(clips, key=lambda c: c["a"]):
    if carrier[c["id"]] is not None:
        owners[carrier[c["id"]]].append(c["guest"])
changes = {i: sum(1 for x, y in zip(o, o[1:]) if x != y) for i, o in owners.items()}
res["capture"] = dict(clips_on_no_stream=len(missing), owner_changes=changes, clips_per_stream={i: len(o) for i, o in owners.items()},
                      guests_per_stream={i: len(set(o)) for i, o in owners.items()})
print(f"capture: clips on no stream {len(missing)}; stream owner changes {changes}; guests per stream {res['capture']['guests_per_stream']}")

# Meet's speaking marks per 100 ms frame (names per report)
marks = []
for l in open(C + "report.jsonl"):
    r = json.loads(l)
    for t_ms, lit in zip(r.get("ft", []), r.get("lit", [])):
        marks.append((t_ms / 1000, [r["names"][k] for k in lit if k < len(r["names"])]))
def name_for(slot, a, b):
    """the name Meet marks as speaking most while this stream has sound in [a, b]"""
    v = collections.Counter()
    for t, names in marks:
        if a <= t <= b and names and rms(slot, t, t + 0.1) > 300:
            for n in names:
                v[n] += 1 / len(names)
    return v.most_common(1)[0][0] if v else None

P = sys.argv[3] if len(sys.argv) > 3 else "asr"
rows = {}
import os
for model in ("orig", "tuned"):
    segs = segs_eval(f"{C}{P}-{model}-system/segments.json", REC0); d = best_offset(segs)
    rows[f"{model}: mixed (app recording)"] = score_turns([dict(s, a=s["a"] + d, b=s["b"] + d) for s in segs])
    if os.path.exists(f"{C}{P}-{model}-mix/segments.json"):
        segs = segs_eval(f"{C}{P}-{model}-mix/segments.json", T0); d = best_offset(segs)
        rows[f"{model}: sum of streams"] = score_turns([dict(s, a=s["a"] + d, b=s["b"] + d) for s in segs])
    allseg = []
    for i in clock["slots"]:
        for x in segs_eval(f"{C}{P}-{model}-slot-{i}/segments.json", T0):
            allseg.append(dict(x, slot=i))
    d = best_offset(allseg); allseg = [dict(s, a=s["a"] + d, b=s["b"] + d) for s in allseg]
    rows[f"{model}: separate streams"] = score_turns(allseg)
    # speaker per segment from Meet marks, vs who really spoke (by the guests' log, speech of that stream)
    right = wrong = none = 0
    for s in allseg:
        true = [c for c in clips if carrier[c["id"]] == s["slot"] and min(c["b"], s["b"]) - max(c["a"], s["a"]) > 0.3]
        if not true:
            continue
        got = name_for(s["slot"], s["a"] - d, s["b"] - d)
        if got is None:
            none += 1
        elif any(got == f"Гість {c['guest']}" for c in true):
            right += 1
        else:
            wrong += 1
    rows[f"{model}: separate streams"].update(segments=right + wrong + none, speaker_right=right, speaker_wrong=wrong, speaker_unknown=none)
    rows[f"{model}: separate streams, per speaker"] = per_speaker([(s["a"], s["b"], s["text"], name_for(s["slot"], s["a"] - d, s["b"] - d)) for s in allseg])
for k, v in rows.items():
    print(f"{k:34s} {v}")
res["wer"] = rows

# the app's own transcript and speakers
full = json.load(open(R + "/app/transcripts.json"))["segments"]
appsegs = [dict(a=REC0 + s["audio_start_time"], b=REC0 + s["audio_end_time"], text=s["text"]) for s in full]
d = best_offset(appsegs)
res["app_transcript"] = score_turns([dict(s, a=s["a"] + d, b=s["b"] + d) for s in appsegs])
wrong = tot = unsure = 0
for l in open(R + "/app/db-lines.txt"):
    a, b, spk, _ = l.rstrip("\n").split("|", 3)
    a, b = REC0 + float(a) + d, REC0 + float(b) + d
    true = [c for c in clips if min(c["b"], b) - max(c["a"], a) > 0.3]
    if not true:
        continue
    tot += 1; unsure += "(?)" in spk
    # main speaker of the line: the guest with the most time in it
    main = max(true, key=lambda c: min(c["b"], b) - max(c["a"], a))
    wrong += not spk.startswith(f"Гість {main['guest']}")
res["app_speakers"] = dict(lines=tot, wrong_main_speaker=wrong, unsure=unsure)
lab = {round(float(l.split("|")[0]), 2): l.split("|")[2] for l in open(R + "/app/db-lines.txt")}
res["app_per_speaker"] = per_speaker([(REC0 + s["audio_start_time"], REC0 + s["audio_end_time"], s["text"], lab.get(round(s["audio_start_time"], 2), "").replace(" (?)", "")) for s in full])
print("app transcript:", res["app_transcript"], res["app_speakers"], res["app_per_speaker"])
json.dump(res, open(R + f"/results-{P}{os.environ.get('DROP_GUESTS') and '-drop' + os.environ['DROP_GUESTS'] or ''}.json", "w"), indent=1, ensure_ascii=False)
