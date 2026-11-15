#!/usr/bin/env python3
"""Lists exported pieces for training and testing: the meetings named with
--test are held out (the most recent ones), all others teach."""
import argparse
import json
import pathlib
import re


def words(text):
    """Lower-case words without punctuation, as the eval compares them"""
    return re.findall(r"[\w'’ʼ-]+", text.lower().replace("’", "'").replace("ʼ", "'"))


def wer(right, heard):
    r, h = words(right), words(heard)
    row = list(range(len(h) + 1))
    for i, a in enumerate(r, 1):
        prev, row[0] = row[0], i
        for j, b in enumerate(h, 1):
            prev, row[j] = row[j], min(row[j] + 1, row[j - 1] + 1, prev + (a != b))
    return row[-1] / max(1, len(r))


def heard_by_piece(path):
    """(meeting, file) → Whisper's text, from an eval replay file"""
    out = {}
    for line in open(path, encoding="utf-8"):
        v = json.loads(line)
        out[(v["meeting"], v["file"])] = " ".join(w["w"] for w in v.get("whisper", []))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root", type=pathlib.Path, help="output folder of the eval stage «export»")
    ap.add_argument("--test", required=True, help="comma-separated meeting ids held out for testing")
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--replay", type=pathlib.Path, help="an eval replay-<tag>.jsonl with the original Whisper's words for these pieces")
    ap.add_argument("--max-wer", type=float, default=0.5, help="with --replay: leave out training pieces where the checked text differs from Whisper's in more than this share of words (rewritten or misaligned lines)")
    a = ap.parse_args()
    heard = heard_by_piece(a.replay) if a.replay else {}
    rewritten = 0
    test = {t.strip() for t in a.test.split(",") if t.strip()}
    a.out.mkdir(parents=True, exist_ok=True)
    rows = {"train": [], "test": []}
    too_long = 0
    for d in sorted(p for p in a.root.iterdir() if (p / "pieces.jsonl").exists()):
        part = "test" if d.name in test else "train"
        for line in (d / "pieces.jsonl").read_text(encoding="utf-8").splitlines():
            v = json.loads(line)
            text = " ".join(v["text"].split())
            if v["end"] - v["start"] > 30:
                too_long += 1  # Whisper hears 30 s at most; the rest of the text would be unheard
                continue
            if part == "train" and heard and (d.name, v["file"]) in heard and wer(text, heard[(d.name, v["file"])]) > a.max_wer:
                rewritten += 1
                continue
            if text:
                rows[part].append({"meeting": d.name, "audio": str(d / v["file"]), "text": text, "seconds": v["end"] - v["start"]})
    missing = test - {r["meeting"] for r in rows["test"]}
    if missing:
        raise SystemExit(f"test meetings not found: {sorted(missing)}")
    for part, items in rows.items():
        with open(a.out / f"{part}.jsonl", "w", encoding="utf-8") as f:
            for r in items:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
        hours = sum(r["seconds"] for r in items) / 3600
        meetings = len({r["meeting"] for r in items})
        print(f"{part}: {meetings} meetings, {len(items)} pieces, {hours:.2f} h")
    if too_long:
        print(f"skipped {too_long} pieces longer than 30 s")
    if heard:
        print(f"left out {rewritten} training pieces differing from Whisper in over {a.max_wer:.0%} of words")


if __name__ == "__main__":
    main()
