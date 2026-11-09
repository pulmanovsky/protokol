#!/usr/bin/env python3
"""Benchmark helpers for scripts/bench/run.sh (no data in the repository).

  report.py segments <speaker transcript.txt> <segments.json>
      speaker transcript lines "[hh:mm:ss] Спікер N: text" -> pipeline segments
  report.py excerpts <audio> <bench dir>
      three one-minute excerpts (early / middle / late) with the current text,
      for a person to correct once into excerpt-N.reference.txt
  report.py report <bench dir>
  report.py approved <copy of meeting_minutes.sqlite>   (approved protocols as references)
      report.md: ASR, speakers, documents, speed, memory; WER/CER when the
      excerpt references exist; task/decision precision and recall when
      reference.json exists (see REFERENCE below)

REFERENCE (optional, written by hand from the transcript, kept next to the
outputs, never committed):
  {"tasks": [{"what": "...", "who": "...", "at": "MM:SS"}],
   "decisions": [{"what": "...", "at": "MM:SS"}]}
"""
import json
import os
import re
import subprocess
import sys

RUSSIAN = set("ыэёъЫЭЁЪ")


def hms(t):
    p = [int(x) for x in t.split(":")]
    return p[0] * 3600 + p[1] * 60 + p[2] if len(p) == 3 else p[0] * 60 + p[1]


def stems(text, n=5):
    words = re.findall(r"[\w'’]+", text.lower())
    return {w[:n] for w in words if len(w) >= 4}


def cmd_segments(src, dst):
    out = []
    for line in open(src, encoding="utf-8"):
        m = re.match(r"\[(\d+:\d+:\d+)\] (.*)", line.strip())
        if m:
            s = hms(m[1]) * 1000.0
            out.append({"start_ms": s, "end_ms": s, "text": m[2], "confidence": 1.0})
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    json.dump(out, open(dst, "w", encoding="utf-8"), ensure_ascii=False)


def ffmpeg():
    here = os.path.dirname(os.path.abspath(__file__))
    for p in [os.path.join(here, "../../frontend/src-tauri/binaries/ffmpeg-aarch64-apple-darwin"), "/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"]:
        if os.path.exists(p):
            return p
    return "ffmpeg"


def cmd_excerpts(audio, out):
    segs = json.load(open(os.path.join(out, "asr", "segments.json"), encoding="utf-8"))
    total = max(s["end_ms"] for s in segs) / 1000.0
    d = os.path.join(out, "excerpts")
    os.makedirs(d, exist_ok=True)
    for i, frac in enumerate([0.1, 0.5, 0.85], 1):
        start = int(total * frac)
        # Whole segments that start inside the minute
        text = " ".join(s["text"] for s in segs if start <= s["start_ms"] / 1000.0 < start + 60)
        first = min((s["start_ms"] / 1000.0 for s in segs if start <= s["start_ms"] / 1000.0 < start + 60), default=start)
        last = max((s["end_ms"] / 1000.0 for s in segs if start <= s["start_ms"] / 1000.0 < start + 60), default=start + 60)
        subprocess.run([ffmpeg(), "-y", "-loglevel", "error", "-ss", f"{first:.2f}", "-to", f"{last:.2f}", "-i", audio, "-c:a", "aac", "-b:a", "96k", os.path.join(d, f"excerpt-{i}.m4a")], check=False)
        with open(os.path.join(d, f"excerpt-{i}.txt"), "w", encoding="utf-8") as f:
            f.write(text.strip() + "\n")
        ref = os.path.join(d, f"excerpt-{i}.reference.txt")
        if not os.path.exists(ref):
            with open(ref, "w", encoding="utf-8") as f:
                f.write("# Виправте текст нижче так, як насправді сказано у excerpt-%d.m4a, і збережіть.\n" % i)
                f.write(text.strip() + "\n")


def edit_distance(a, b):
    prev = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        cur = [i]
        for j, y in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (x != y)))
        prev = cur
    return prev[-1]


def norm(text):
    return re.sub(r"\s+", " ", re.sub(r"[^\w\s'’-]", " ", text.lower())).strip()


def time_stats(path):
    """Wall seconds and peak memory (MB) from /usr/bin/time -l"""
    if not os.path.exists(path):
        return None, None
    t = open(path, encoding="utf-8", errors="ignore").read()
    real = re.search(r"([\d.]+) real", t)
    rss = re.search(r"(\d+)\s+maximum resident set size", t)
    return (float(real[1]) if real else None), (int(rss[1]) / 1e6 if rss else None)


def match(items, ref, key="what", window=90):
    """Greedy matching by shared word stems and nearby time"""
    used, pairs = set(), []
    for r in ref:
        rs, rt = stems(r[key]), hms(r["at"]) if r.get("at") else None
        best, score = None, 0
        for i, it in enumerate(items):
            if i in used:
                continue
            sc = len(rs & stems(it[key]))
            if rt is not None and it.get("at") is not None and abs(it["at"] - rt) > window:
                continue
            if sc > score:
                best, score = i, sc
        if best is not None and score >= 2:
            used.add(best)
            pairs.append((r, items[best]))
    return pairs


def doc_items(md):
    tasks, decisions, section = [], [], None
    for line in md.splitlines():
        if line.startswith("## "):
            section = line[3:].strip()
            continue
        at = re.search(r"\[(\d+:\d+(?::\d+)?)\]", line)
        at = hms(at[1]) if at else None
        if section == "Завдання" and line.startswith("| ") and not line.startswith("| Хто") and not line.startswith("| ---"):
            cells = [c.strip() for c in line.strip("|").split("|")]
            if len(cells) >= 2:
                tasks.append({"who": cells[0], "what": cells[1], "at": at})
        if section == "Рішення" and line.startswith("- "):
            decisions.append({"what": line[2:], "at": at})
    return tasks, decisions


def cmd_report(out):
    lines = ["# Benchmark", ""]
    duration = None
    # --- ASR
    segs_path = os.path.join(out, "asr", "segments.json")
    if os.path.exists(segs_path):
        segs = json.load(open(segs_path, encoding="utf-8"))
        timings = open(os.path.join(out, "asr", "timings.txt"), encoding="utf-8").read()
        m = re.search(r"audio (\d+)s.*?\((\d+)s speech\)", timings)
        duration, speech = (int(m[1]), int(m[2])) if m else (None, None)
        covered = sum((s["end_ms"] - s["start_ms"]) / 1000.0 for s in segs)
        text = " ".join(s["text"] for s in segs)
        latin = re.findall(r"\b[A-Za-z][A-Za-z'’-]+\b", text)
        foreign = [s for s in segs if sum(c in RUSSIAN for c in s["text"]) >= 1 or (lambda w: w and len(re.findall(r"[A-Za-z]+", s["text"])) / len(w) >= 0.5)(s["text"].split())]
        lines += ["## ASR", "", "| metric | value |", "| --- | --- |"]
        lines.append(f"| audio, speech (VAD) | {duration} s, {speech} s |")
        lines.append(f"| speech covered by text | {100 * covered / speech:.0f}% |" if speech else "| speech covered by text | n/a |")
        lines.append(f"| segments in another language (proxy) | {len(foreign)} of {len(segs)} ({100 * len(foreign) / max(1, len(segs)):.1f}%) |")
        lines.append(f"| Russian-only letters | {sum(c in RUSSIAN for c in text)} |")
        lines.append(f"| Latin words | {len(latin)} |")
        gl = os.path.expanduser("~/Library/Application Support/ua.protokol.app/glossary.txt")
        if os.path.exists(gl):
            terms = [l.split(" — ")[0].strip() for l in open(gl, encoding="utf-8") if l.strip() and not l.startswith("#")]
            found = sum(1 for t in terms if t.lower() in text.lower())
            lines.append(f"| dictionary terms heard exactly | {found} of {len(terms)} |")
        # WER/CER on corrected excerpts
        for i in (1, 2, 3):
            ref_p = os.path.join(out, "excerpts", f"excerpt-{i}.reference.txt")
            hyp_p = os.path.join(out, "excerpts", f"excerpt-{i}.txt")
            if os.path.exists(ref_p) and os.path.exists(hyp_p):
                ref = norm("\n".join(l for l in open(ref_p, encoding="utf-8") if not l.startswith("#")))
                hyp = norm(open(hyp_p, encoding="utf-8").read())
                if ref == hyp:
                    lines.append(f"| excerpt {i} WER / CER | not corrected yet |")
                    continue
                wer = edit_distance(ref.split(), hyp.split()) / max(1, len(ref.split()))
                cer = edit_distance(list(ref), list(hyp)) / max(1, len(ref))
                lines.append(f"| excerpt {i} WER / CER | {100 * wer:.1f}% / {100 * cer:.1f}% |")
        lines.append("")
    # --- Speakers
    sp = os.path.join(out, "speakers", "timings.txt")
    if os.path.exists(sp):
        t = open(sp, encoding="utf-8").read()
        m = re.search(r"(\d+) speakers, (\d+) turns; speech per speaker: (.*)", t)
        lines += ["## Speakers", "", "| metric | value |", "| --- | --- |"]
        if m:
            secs = sorted((int(x) for x in re.findall(r"(\d+)s", m[3])), reverse=True)
            total = max(1, sum(secs))
            lines.append(f"| voices, turns | {m[1]}, {m[2]} |")
            lines.append(f"| talk time in top 3 / top 5 voices | {100 * sum(secs[:3]) / total:.0f}% / {100 * sum(secs[:5]) / total:.0f}% |")
            lines.append(f"| voices under 10 s | {sum(1 for s in secs if s < 10)} |")
        close = re.search(r"closest voices: (.*)", t)
        if close:
            sims = [float(x) for x in re.findall(r" (0\.\d+)", close[1])]
            lines.append(f"| suspected split voices (similarity 0.55–0.65) | {sum(1 for s in sims if 0.55 <= s < 0.65)} |")
        tr = os.path.join(out, "speakers", "transcript.txt")
        if os.path.exists(tr):
            rows = open(tr, encoding="utf-8").read().splitlines()
            unnamed = sum(1 for r in rows if "] Спікер " in r)
            lines.append(f"| speaker changes labelled «Спікер N» | {unnamed} of {len(rows)} |")
        lines.append("")
    # --- Documents
    ref_path = os.path.join(out, "reference.json")
    ref = json.load(open(ref_path, encoding="utf-8")) if os.path.exists(ref_path) else None
    for name in ("doc_daily", "doc_general"):
        md_p = os.path.join(out, name, "protocol.md")
        if not os.path.exists(md_p):
            continue
        md = open(md_p, encoding="utf-8").read()
        tasks, decisions = doc_items(md)
        lines += [f"## Document: {name}", "", "| metric | value |", "| --- | --- |"]
        lines.append(f"| reports in «Хід наради» | {md.count('**ВИСТУПИЛИ:**')} |")
        lines.append(f"| tasks, with an owner | {len(tasks)}, {sum(1 for t in tasks if t['who'] not in ('—', '-', ''))} |")
        lines.append(f"| decisions | {len(decisions)} |")
        if ref:
            for kind, items in (("tasks", tasks), ("decisions", decisions)):
                pairs = match(items, ref.get(kind, []))
                p = len(pairs) / max(1, len(items))
                r = len(pairs) / max(1, len(ref.get(kind, [])))
                lines.append(f"| {kind} precision / recall | {100 * p:.0f}% / {100 * r:.0f}% |")
                if kind == "tasks":
                    owners = sum(1 for a, b in pairs if a.get("who") and stems(a["who"], 4) & stems(b["who"], 4))
                    lines.append(f"| task owner correct (of matched) | {owners} of {len(pairs)} |")
        lines.append("")
    # --- Speed and memory
    lines += ["## Speed and memory", "", "| step | wall | per hour of audio | peak memory |", "| --- | --- | --- | --- |"]
    for step in ("asr", "speakers", "doc_daily", "doc_general"):
        real, mb = time_stats(os.path.join(out, f"{step}.time"))
        if real is None:
            continue
        per_hour = f"{real / 60 / (duration / 3600):.1f} min" if duration else "n/a"
        lines.append(f"| {step} | {real / 60:.1f} min | {per_hour} | {mb:.0f} MB |" if mb else f"| {step} | {real / 60:.1f} min | {per_hour} | n/a |")
    lines += ["", "Times include building the test binary when it was out of date."]
    open(os.path.join(out, "report.md"), "w", encoding="utf-8").write("\n".join(lines) + "\n")
    print("\n".join(lines))


def cmd_approved(db):
    """Every protocol the secretary approved is a reference: how close the
    app's draft was (tasks precision/recall), and the error rate of
    recognition on transcript lines the secretary corrected by hand.
    Run on a COPY of the app database; prints numbers only."""
    import sqlite3
    con = sqlite3.connect(db)
    try:
        rows = con.execute("SELECT meeting_id, meeting_type, draft_md, final_md FROM protocol_revisions WHERE final_md IS NOT NULL").fetchall()
    except sqlite3.OperationalError:
        rows = []  # a database from before 0.2.9: nothing approved to learn from yet
    print("| meeting type | draft tasks | approved tasks | precision | recall |")
    print("| --- | --- | --- | --- | --- |")
    for _, mtype, draft, final in rows:
        d, _ = doc_items(draft)
        f, _ = doc_items(final)
        pairs = match(d, [{**x, "at": None} for x in f], window=10**6)
        p = 100 * len(pairs) / max(1, len(d))
        r = 100 * len(pairs) / max(1, len(f))
        print(f"| {mtype} | {len(d)} | {len(f)} | {p:.0f}% | {r:.0f}% |")
    if not rows:
        print("| (no approved protocols yet) | | | | |")
    try:
        edited = con.execute("SELECT original_transcript, transcript FROM transcripts WHERE original_transcript IS NOT NULL").fetchall()
    except sqlite3.OperationalError:
        edited = []
    words = sum(len(norm(c).split()) for _, c in edited)
    errors = sum(edit_distance(norm(c).split(), norm(o).split()) for o, c in edited)
    print()
    print(f"Corrected transcript lines: {len(edited)}; word error rate on them {100 * errors / max(1, words):.1f}% ({errors} of {words} words)")


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "segments":
        cmd_segments(sys.argv[2], sys.argv[3])
    elif cmd == "excerpts":
        cmd_excerpts(sys.argv[2], sys.argv[3])
    elif cmd == "report":
        cmd_report(sys.argv[2])
    elif cmd == "approved":
        cmd_approved(sys.argv[2])
