# /// script
# requires-python = ">=3.10"
# dependencies = ["cryptography>=42"]
# ///
"""發佈「語音轉錄」新版，讓已安裝的電腦與手機自己更新（2026-10-31 使用者：「支援推送升級功能且每台裝置有識別碼可以獨立推送更新」）。

  uv run 工具/publish.py new-key --generate-password   只做一次：產生簽章金鑰（密碼隨機、存在 keys/）
  uv run 工具/publish.py publish --notes "修正…"        dist/ 裡這一版的安裝檔＋APK → GitHub Release v<版本>（給所有裝置）
  uv run 工具/publish.py publish --devices 王老師,K7Q2-9XMA   只推給這些裝置（識別碼或 工具/devices.json 的名字）
  uv run 工具/publish.py publish --only windows          只發電腦版（或 android）
  uv run 工具/publish.py open v1.2.0                     把只推給部分裝置的版本開放給所有裝置
  uv run 工具/publish.py publish --dry-run               只在 target/publish/ 簽好，不上傳

一個 Release 帶：安裝檔（MeetScribe-Setup-v<版本>.exe）、APK（MeetScribe-Android-v<版本>.apk）、update.json（版本、說明、
各平台檔名／大小／SHA-256、devices＝null 或識別碼清單）、update.sig（ECDSA P-256／SHA-256 簽 update.json 的原始位元組，DER、base64）。
程式（meetscribe-core/src/update.rs）只收簽章對得上公鑰 packaging/update.pub（編進程式）的 Release。

私鑰 keys/update-signing.pem（不進版控、靠每週完整備份；另外抄一份到隨身碟）。**遺失或重產＝已安裝的每一台都再也收不到更新**，
只能手動重裝。密碼：環境變數 MEETSCRIBE_KEY_PASSWORD（測試），否則 keys/update-signing-password.txt。
裝置名字對照：工具/devices.json（不進版控），{"王老師的筆電": "K7Q2-9XMA", …}；識別碼在程式「設定 → 關於與更新」。
"""
import argparse
import base64
import getpass
import hashlib
import json
import os
import re
import secrets
import subprocess
import sys
from pathlib import Path

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec

ROOT = Path(__file__).resolve().parent.parent
KEY = ROOT / "keys" / "update-signing.pem"
PASSWORD_FILE = ROOT / "keys" / "update-signing-password.txt"
PUBLIC = ROOT / "packaging" / "update.pub"
DEVICES = ROOT / "工具" / "devices.json"
OUT = ROOT / "target" / "publish"
REPO = "win81134679-dot/meetscribe"
DEVICE_ID = re.compile(r"^[2-9A-HJKMNP-TV-Z]{4}-[2-9A-HJKMNP-TV-Z]{4}$")


def cargo_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    return re.search(r'^version = "(.+)"', text, re.M).group(1)


def password(confirm: bool) -> bytes:
    if os.environ.get("MEETSCRIBE_KEY_PASSWORD"):
        return os.environ["MEETSCRIBE_KEY_PASSWORD"].encode()
    if not confirm and PASSWORD_FILE.exists():
        return PASSWORD_FILE.read_text(encoding="utf-8").strip().encode()
    first = getpass.getpass("簽章金鑰密碼：")
    if confirm and getpass.getpass("再輸入一次：") != first:
        raise SystemExit("FAIL：兩次密碼不同")
    if len(first) < 8:
        raise SystemExit("FAIL：至少 8 個字元")
    return first.encode()


def new_key(args) -> None:
    key_path, pub_path = Path(args.key), Path(args.public)
    if key_path.exists():
        raise SystemExit(f"FAIL：{key_path} 已存在；換新金鑰會讓已安裝的每一台收不到更新。什麼都沒改。")
    if args.generate_password:
        if PASSWORD_FILE.exists():
            raise SystemExit(f"FAIL：{PASSWORD_FILE} 已存在。什麼都沒改。")
        secret = secrets.token_urlsafe(24)
        PASSWORD_FILE.parent.mkdir(parents=True, exist_ok=True)
        PASSWORD_FILE.write_text(secret + "\n", encoding="ascii")
        pw = secret.encode()
    else:
        pw = password(confirm=True)
    key = ec.generate_private_key(ec.SECP256R1())
    pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                            serialization.BestAvailableEncryption(pw))
    key_path.parent.mkdir(parents=True, exist_ok=True)
    key_path.write_bytes(pem)
    pub_path.parent.mkdir(parents=True, exist_ok=True)
    pub_path.write_bytes(key.public_key().public_bytes(serialization.Encoding.PEM,
                                                       serialization.PublicFormat.SubjectPublicKeyInfo))
    print(f"私鑰：{key_path}（抄一份到隨身碟）\n公鑰：{pub_path}（進版控，編進程式）")


def load_key(path: Path):
    if not path.exists():
        raise SystemExit(f"FAIL：{path} 沒有簽章金鑰；先跑 uv run 工具/publish.py new-key --generate-password")
    try:
        return serialization.load_pem_private_key(path.read_bytes(), password=password(confirm=False))
    except ValueError:
        raise SystemExit("FAIL：密碼不對")


def sign(key, data: bytes, public: Path) -> bytes:
    signature = key.sign(data, ec.ECDSA(hashes.SHA256()))
    # 一定要能用程式內建的公鑰驗過，否則沒有一台會收
    pub = serialization.load_pem_public_key(public.read_bytes())
    try:
        pub.verify(signature, data, ec.ECDSA(hashes.SHA256()))
    except InvalidSignature:
        raise SystemExit(f"FAIL：私鑰跟 {public} 不是一對；已安裝的程式會拒收")
    return base64.b64encode(signature) + b"\n"


def resolve_devices(text: str | None) -> list[str] | None:
    if not text:
        return None
    names = json.loads(DEVICES.read_text(encoding="utf-8")) if DEVICES.exists() else {}
    out = []
    for item in (s.strip() for s in text.split(",") if s.strip()):
        did = names.get(item, item).upper()
        if not DEVICE_ID.match(did):
            raise SystemExit(f"FAIL：{item!r} 不是識別碼（XXXX-XXXX），也不是 {DEVICES.name} 裡的名字")
        out.append(did)
    return sorted(set(out))


def asset(path: Path) -> dict:
    data = path.read_bytes()
    return {"file": path.name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def build_manifest(version: str, notes: str, devices, files: dict) -> bytes:
    doc = {"version": version, "notes": notes, "devices": devices,
           "windows": asset(files["windows"]) if "windows" in files else None,
           "android": asset(files["android"]) if "android" in files else None}
    return (json.dumps(doc, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def gh(*args: str, check=True) -> subprocess.CompletedProcess:
    return subprocess.run(["gh", *args], check=check, capture_output=not check, text=True)


def release_body(version: str, notes: str, devices) -> str:
    who = "所有裝置" if devices is None else "指定裝置：" + "、".join(devices)
    lines = [f"## 語音轉錄 {version}", "", notes or "（沒有說明）", "",
             f"推送對象：{who}。已安裝的電腦版與手機版會自己檢查並更新（約一天內），也可在「設定 → 關於與更新」按「檢查更新」。",
             "", "第一次安裝：電腦下載 `MeetScribe-Setup-v*.exe`；Android 下載 `MeetScribe-Android-v*.apk`。"]
    return "\n".join(lines)


def publish(args) -> None:
    version = args.version or cargo_version()
    dist = Path(args.dist)
    wanted = {"windows": dist / f"MeetScribe-Setup-v{version}.exe",
              "android": dist / f"MeetScribe-Android-v{version}.apk"}
    if args.only:
        wanted = {k: v for k, v in wanted.items() if k == args.only}
    files = {k: v for k, v in wanted.items() if v.exists()}
    missing = [str(v) for k, v in wanted.items() if k not in files]
    if missing and (args.only or not files):
        raise SystemExit("FAIL：找不到 " + "、".join(missing))
    for m in missing:
        print(f"注意：沒有 {m}，這版只發 {'、'.join(files)}")
    devices = resolve_devices(args.devices)
    manifest = build_manifest(version, args.notes, devices, files)
    signature = sign(load_key(Path(args.key)), manifest, Path(args.public))
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "update.json").write_bytes(manifest)
    (OUT / "update.sig").write_bytes(signature)
    tag = "v" + version
    who = "所有裝置" if devices is None else ", ".join(devices)
    for k, f in files.items():
        print(f"{tag} {k}: {f.name} ({f.stat().st_size / 1e6:.1f} MB)")
    print(f"對象：{who}")
    if args.dry_run:
        print(f"dry run：簽好的檔在 {OUT}，沒有上傳")
        return
    uploads = [str(f) for f in files.values()] + [str(OUT / "update.json"), str(OUT / "update.sig")]
    exists = gh("release", "view", tag, "--repo", args.repo, check=False).returncode == 0
    if exists:
        gh("release", "upload", tag, "--repo", args.repo, "--clobber", *uploads)
    else:
        gh("release", "create", tag, "--repo", args.repo, "--title", f"語音轉錄 {version}",
           "--notes", release_body(version, args.notes, devices), *uploads)
    print(f"已發佈 {tag}；裝好的程式一天內會自己更新")


def open_release(args) -> None:
    tag = args.tag
    OUT.mkdir(parents=True, exist_ok=True)
    for f in ("update.json", "update.sig"):
        (OUT / f).unlink(missing_ok=True)
    gh("release", "download", tag, "--repo", args.repo, "--pattern", "update.json", "--dir", str(OUT))
    doc = json.loads((OUT / "update.json").read_text(encoding="utf-8"))
    if doc.get("devices") is None:
        print(f"{tag} 本來就給所有裝置")
        return
    doc["devices"] = None
    manifest = (json.dumps(doc, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    signature = sign(load_key(Path(args.key)), manifest, Path(args.public))
    (OUT / "update.json").write_bytes(manifest)
    (OUT / "update.sig").write_bytes(signature)
    gh("release", "upload", tag, "--repo", args.repo, "--clobber", str(OUT / "update.json"), str(OUT / "update.sig"))
    print(f"{tag} 現在給所有裝置")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--key", default=str(KEY))
    ap.add_argument("--public", default=str(PUBLIC))
    ap.add_argument("--repo", default=REPO)
    sub = ap.add_subparsers(dest="cmd", required=True)
    n = sub.add_parser("new-key")
    n.add_argument("--generate-password", action="store_true", help="隨機密碼存在 keys/，之後發佈不再問")
    p = sub.add_parser("publish")
    p.add_argument("--devices", help="識別碼或 devices.json 的名字，逗號分隔；不給＝所有裝置")
    p.add_argument("--notes", default="", help="這版改了什麼（程式裡會顯示）")
    p.add_argument("--only", choices=["windows", "android"])
    p.add_argument("--version", help="預設讀根 Cargo.toml")
    p.add_argument("--dist", default=str(ROOT / "dist"))
    p.add_argument("--dry-run", action="store_true")
    o = sub.add_parser("open")
    o.add_argument("tag")
    args = ap.parse_args()
    {"new-key": new_key, "publish": publish, "open": open_release}[args.cmd](args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
