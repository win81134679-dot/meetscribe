"""跟隨播放的標準答案：用本機 Qwen3-ForcedAligner 把會議自己的逐字稿對齊到段落音訊。

2026-10-31 第二輪「跟隨播放還是跟不上」用它量出誤差（見 CLAUDE.md〈外部事實〉時間戳那條）。
不知道每 2 分鐘講了哪些字，所以滑動對齊：每窗給足夠多的字，只採信窗前段的時間，下一窗從第一個沒採信的字開始。
拉琴、唱譜那種地方對齊模型會把字硬塞進去，所以用兩組窗口參數各跑一次，兩組差不到 1 秒的字才當答案。

需要裝了 qwen-asr 與 torch（CUDA）的 Python，以及 Qwen3-ForcedAligner-0.6B 模型資料夾（約 1.8 GB 顯示記憶體）：
  <python> 工具/時間誤差/對齊標準答案.py <模型資料夾> <會議資料夾的複本>/project.json <輸出資料夾> <段落 index>...
輸出 <輸出資料夾>/<段落檔名>.json：[[字, 起, 訖], ...]（相對段落開頭，只含兩組都同意的字）。
"""
import json
import os
import sys
import types

sys.stdout.reconfigure(encoding="utf-8")
sys.modules.setdefault("nagisa", types.ModuleType("nagisa"))  # qwen_asr 的日文斷詞讀不了中文路徑，用不到
os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")
import librosa  # noqa: E402
import torch  # noqa: E402
from qwen_asr.inference.qwen3_forced_aligner import Qwen3ForcedAligner  # noqa: E402

SR = 16000
MAX_CHARS = 900  # 一次給太多字顯示記憶體會不夠


def is_cjk(c):
    return "㐀" <= c <= "鿿" or "豈" <= c <= "﫿"


def align(al, wav, chars, win, keep):
    dur = len(wav) / SR
    rate = len(chars) / dur
    out, pos, t0 = [], 0, 0.0
    while pos < len(chars) and t0 < dur - 0.5:
        w_end = min(dur, t0 + win)
        last = w_end >= dur - 0.01
        k = min(MAX_CHARS, len(chars) - pos if last else max(60, int(rate * win * 2.5)))
        piece = chars[pos:pos + k]
        items = list(al.align(audio=(wav[int(t0 * SR):int(w_end * SR)], SR), text=" ".join(piece), language="Chinese")[0].items)
        if len(items) != len(piece):
            break
        limit = w_end if last else t0 + win * keep
        taken = 0
        for ch, it in zip(piece, items):
            if t0 + it.start_time >= limit:
                break
            out.append([ch, t0 + it.start_time, t0 + it.end_time])
            taken += 1
        if taken == 0:
            t0 += win * keep
            continue
        pos += taken
        t0 = max(t0 + 5.0, out[-1][2])
    return out


def main():
    model_dir, proj_path, out_dir, *idxs = sys.argv[1:]
    os.makedirs(out_dir, exist_ok=True)
    proj = json.load(open(proj_path, encoding="utf-8"))
    al = Qwen3ForcedAligner.from_pretrained(model_dir, dtype=torch.bfloat16, device_map="cuda:0")
    for ci in map(int, idxs):
        c = next(x for x in proj["chunks"] if x["index"] == ci)
        segs = sorted((s for s in proj["segments"] if s["chunk"] == ci), key=lambda s: s["start"])
        chars = [ch for s in segs for ch in s["text"] if is_cjk(ch)]
        wav, _ = librosa.load(os.path.join(os.path.dirname(proj_path), c["file"]), sr=SR, mono=True)
        a = align(al, wav, chars, 120.0, 0.6)
        b = align(al, wav, chars, 90.0, 0.5)
        agreed = [x for x, y in zip(a, b) if abs(x[1] - y[1]) < 1.0]
        name = os.path.splitext(os.path.basename(c["file"]))[0] + ".json"
        json.dump(agreed, open(os.path.join(out_dir, name), "w", encoding="utf-8"), ensure_ascii=False)
        print(f"段落 {ci}：{len(agreed)}/{len(chars)} 字兩組一致，涵蓋到 {max((x[1] for x in agreed), default=0):.0f} 秒", flush=True)


if __name__ == "__main__":
    main()
