"""模擬 OpenRouter＋Google AI Studio，給 Android 模擬器上的 MeetScribe 做閉環測試。

模擬器內用 http://10.0.2.2:18765 連到這台電腦的 127.0.0.1:18765。
收到的轉錄音訊存成 received/stt_<n>.mp3（之後用 ffprobe 驗證手機端編出來的 MP3），
每個請求記一行到 received/requests.log。

用法：python -I android/test/mock_api.py <輸出資料夾> [轉錄延遲秒數]
App 那邊：debug 版用 run-as 把 files/data/settings.json 的 api_base 改成 http://10.0.2.2:18765、
google_api_base 改成 http://10.0.2.2:18765/gemini，重開 App，金鑰隨便填（sk-or-v1-…、AIza…）。
"""
import base64
import json
import re
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

OUT = Path(sys.argv[1] if len(sys.argv) > 1 else "received")
OUT.mkdir(parents=True, exist_ok=True)
LOCK = threading.Lock()
COUNT = {"stt": 0}

STT_TEXT = (
    "大家好，我是王經理，今天的會議主要討論第四季的行銷預算。"
    "Thanks. I am Linda from the finance team. Our total budget for the fourth quarter is two hundred thousand dollars. "
    "好的，我建議把一半的預算放在社群廣告，另外一半用在展覽活動。"
    "That works for me. Please send me the detailed plan by next Friday. "
    "沒問題，我下週五之前把企劃書寄給你。那今天的會議就到這裡，謝謝大家。"
)

SUMMARY = """# 第四季行銷預算會議

## 決議
- 第四季總預算 **20 萬美元**
- 一半用於社群廣告、一半用於展覽活動

## 待辦
- 王經理：下週五前寄出企劃書給 Linda
"""


def strings(v):
    if isinstance(v, str):
        yield v
    elif isinstance(v, dict):
        for x in v.values():
            yield from strings(x)
    elif isinstance(v, list):
        for x in v:
            yield from strings(x)


def log(line):
    with LOCK, open(OUT / "requests.log", "a", encoding="utf-8") as f:
        f.write(line + "\n")


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def send(self, obj, code=200):
        body = json.dumps(obj, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass

    def do_GET(self):
        p = self.path.split("?")[0]
        log(f"GET {p}")
        if p == "/key":
            return self.send({"data": {"label": "mock", "usage": 1.25, "limit": None, "limit_remaining": None, "is_free_tier": False}})
        if p == "/credits":
            return self.send({"data": {"total_credits": 10.0, "total_usage": 1.25}})
        if p.startswith("/models/") and p.endswith("/endpoints"):
            return self.send({"data": {"endpoints": [{"provider_name": "DeepInfra", "supports_diarization": False,
                                                       "pricing": {"prompt": "0.0000333"}}]}})
        if p == "/models":
            return self.send({"data": [{"id": "nvidia/nemotron-3-ultra-550b-a55b:free", "name": "x", "context_length": 1000000,
                                        "pricing": {"prompt": "0", "completion": "0"},
                                        "architecture": {"input_modalities": ["text"], "output_modalities": ["text"]}}]})
        if p == "/gemini/models":
            return self.send({"models": [{"name": "models/gemini-3.8-flash", "supportedGenerationMethods": ["generateContent"]}]})
        self.send({"error": {"code": 404, "message": f"mock: no route {p}"}}, 404)

    def do_POST(self):
        p = self.path.split("?")[0]
        n = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(n)
        try:
            body = json.loads(raw)
        except Exception:
            body = {}
        text = "\n".join(strings(body))
        log(f"POST {p} {n} bytes")

        if p == "/audio/transcriptions":
            # 第二個參數：轉錄延遲秒數（觀察通知列的前景服務用）
            if len(sys.argv) > 2:
                import time
                time.sleep(float(sys.argv[2]))
            audio = body.get("input_audio", {})
            with LOCK:
                COUNT["stt"] += 1
                k = COUNT["stt"]
            (OUT / f"stt_{k}.{audio.get('format', 'bin')}").write_bytes(base64.b64decode(audio.get("data", "")))
            log(f"  stt format={audio.get('format')} model={body.get('model')} language={body.get('language')}")
            return self.send({"text": STT_TEXT, "usage": {"cost": 0.0013, "seconds": 38.0}})

        if p.startswith("/gemini/models/") and p.endswith(":generateContent"):
            if "說話者標註員" in text:
                units = re.findall(r"^\[(\d+)\] (.*)$", text, re.M)
                has_audio = "inline_data" in raw.decode("utf-8", "ignore")
                log(f"  attribution units={len(units)} audio={has_audio}")
                a, t = [], 0.0
                for i, u in units:
                    who = "N1" if re.search(r"[一-鿿]", u) else "N2"
                    a.append([int(i), who, round(t, 1)])
                    t += max(1.0, len(u) / 6)
                out = {"speakers": [{"id": "N1", "name": "王經理", "note": "女聲中文"},
                                    {"id": "N2", "name": "Linda", "note": "財務，英文"}], "a": a}
                reply = "```json\n" + json.dumps(out, ensure_ascii=False) + "\n```"
            elif "說話者清單" in text:
                reply = json.dumps({"merges": [], "names": []})
            else:
                log("  summary")
                reply = "```markdown\n" + SUMMARY + "```"
            return self.send({"candidates": [{"content": {"parts": [{"text": reply}]}, "finishReason": "STOP"}],
                              "usageMetadata": {"promptTokenCount": 3000, "candidatesTokenCount": 200}})

        if p == "/chat/completions":
            reply = json.dumps({"merges": [], "names": []}) if "說話者清單" in text else "```markdown\n" + SUMMARY + "```"
            return self.send({"choices": [{"message": {"content": reply}, "finish_reason": "stop"}],
                              "usage": {"prompt_tokens": 100, "completion_tokens": 20, "cost": 0}})

        self.send({"error": {"code": 404, "message": f"mock: no route {p}"}}, 404)


if __name__ == "__main__":
    srv = ThreadingHTTPServer(("127.0.0.1", 18765), H)
    print("mock listening on 127.0.0.1:18765", flush=True)
    srv.serve_forever()
