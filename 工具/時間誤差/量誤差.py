"""量跟隨播放的時間誤差：project.json 每句的開始 vs 對齊標準答案.py 產的標準答案。
用法：python 工具/時間誤差/量誤差.py <project.json> <標準答案資料夾> [--limit 秒] [--detail 段落index]
改了 timing.rs 之後：把會議資料夾複製一份、`cargo run -p meetscribe-core --example retime -- <複本>`，再對複本跑這支。
誤差＝程式的開始 − 真的開始；正的＝字幕比聲音晚亮（跟不上），負的＝搶先。
結束碼：任何段落 p90 |誤差| > limit（預設 3 秒）就是 1（紅）。"""
import difflib
import json
import os
import sys

sys.stdout.reconfigure(encoding="utf-8")


def norm_chars(s):
    return [c for c in s.lower() if c.isalnum()]


def evaluate(proj, gt_dir, detail=None):
    out = {}
    for c in proj["chunks"]:
        name = os.path.splitext(os.path.basename(c["file"]))[0] + ".json"
        path = os.path.join(gt_dir, name)
        if not os.path.exists(path):
            continue
        gt = json.load(open(path, encoding="utf-8"))
        g_chars, g_times = [], []
        for text, s, _e in gt:
            for ch in norm_chars(text):
                g_chars.append(ch)
                g_times.append(s)
        segs = sorted((s for s in proj["segments"] if s["chunk"] == c["index"]), key=lambda s: s["start"])
        o_chars, seg_of, seg_first = [], [], []
        for k, s in enumerate(segs):
            seg_first.append(len(o_chars))
            for ch in norm_chars(s["text"]):
                o_chars.append(ch)
                seg_of.append(k)
        sm = difflib.SequenceMatcher(None, o_chars, g_chars, autojunk=False)
        o2g = {}
        for a, b, n in sm.get_matching_blocks():
            if n >= 2:  # 單字配對太容易巧合
                for i in range(n):
                    o2g[a + i] = b + i
        errs = []
        for k, s in enumerate(segs):
            lo = seg_first[k]
            hi = seg_first[k + 1] if k + 1 < len(segs) else len(o_chars)
            hit = next((i for i in range(lo, hi) if i in o2g), None)
            if hit is None or hi - lo < 4:
                continue
            # 對到的第一個字之前還有幾個字沒對到：用平均語速往前推一點（每字 0.25 秒）
            truth = g_times[o2g[hit]] - (hit - lo) * 0.25
            mine = s["start"] - c["start"]
            errs.append((mine - truth, mine, truth, s["text"][:24], s.get("timing")))
        if not errs:
            continue
        ab = sorted(abs(e[0]) for e in errs)
        sg = sorted(e[0] for e in errs)
        pick = lambda xs, q: xs[min(len(xs) - 1, int(q * len(xs)))]  # noqa: E731
        matched = sum(1 for i in range(len(o_chars)) if i in o2g) / max(1, len(o_chars))
        out[c["index"]] = dict(
            n=len(errs), of=len(segs), matched=matched, med=pick(ab, 0.5), p90=pick(ab, 0.9),
            signed_med=pick(sg, 0.5), late3=sum(1 for e in errs if e[0] > 3) / len(errs),
            early3=sum(1 for e in errs if e[0] < -3) / len(errs), dur=c["end"] - c["start"],
        )
        if detail is not None and c["index"] == detail:
            for e in errs:
                print(f"  {e[2]:7.1f} 真 → {e[1]:7.1f} 程式  ({e[0]:+6.1f})  {e[4]}  {e[3]}")
    return out


def main():
    args = sys.argv[1:]
    limit, detail = 3.0, None
    if "--limit" in args:
        i = args.index("--limit")
        limit = float(args[i + 1])
        del args[i:i + 2]
    if "--detail" in args:
        i = args.index("--detail")
        detail = int(args[i + 1])
        del args[i:i + 2]
    proj = json.load(open(args[0], encoding="utf-8"))
    res = evaluate(proj, args[1], detail)
    red = False
    for idx, r in res.items():
        flag = "紅" if r["p90"] > limit else "綠"
        red |= r["p90"] > limit
        print(f"段落 {idx:2d}（{r['dur']:.0f} 秒）：{r['n']}/{r['of']} 句對到（字 {r['matched']:.0%}）"
              f"｜中位 {r['med']:.1f} 秒、p90 {r['p90']:.1f} 秒、帶號中位 {r['signed_med']:+.1f}"
              f"｜晚 >3 秒 {r['late3']:.0%}、早 >3 秒 {r['early3']:.0%}  {flag}")
    if not res:
        print("沒有標準答案")
        sys.exit(2)
    sys.exit(1 if red else 0)


if __name__ == "__main__":
    main()
