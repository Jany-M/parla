#!/usr/bin/env python3
"""Regenerates resources/manifest.json with real sizes and SHA-256 hashes.

Hashes come from the hosts' own metadata (Hugging Face LFS oid, GitHub release asset digest);
small files without published hashes are downloaded and hashed. Standard library only.
Run:  python scripts/gen_manifest.py
Bump LLAMA_TAG or a repo/file below, rerun, commit the new manifest.
"""
import hashlib, json, sys, urllib.request
from pathlib import Path

LLAMA_TAG = "b12438"  # pinned llama.cpp release (the "latest" tag is not reliable)
OUT = Path(__file__).resolve().parent.parent / "resources" / "manifest.json"


def get_json(url):
    req = urllib.request.Request(url, headers={"User-Agent": "parla-manifest"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)


def hash_url(url):
    h, n = hashlib.sha256(), 0
    req = urllib.request.Request(url, headers={"User-Agent": "parla-manifest"})
    with urllib.request.urlopen(req, timeout=120) as r:
        while chunk := r.read(1 << 20):
            h.update(chunk)
            n += len(chunk)
    return h.hexdigest(), n


_hf_cache = {}


def hf_file(repo, path):
    if repo not in _hf_cache:
        _hf_cache[repo] = {s["rfilename"]: s for s in get_json(f"https://huggingface.co/api/models/{repo}?blobs=true")["siblings"]}
    s = _hf_cache[repo][path]
    url = f"https://huggingface.co/{repo}/resolve/main/{path}"
    sha = (s.get("lfs") or {}).get("sha256")
    size = s.get("size")
    if not sha:  # small non-LFS file: hash it ourselves
        sha, size = hash_url(url)
    return {"name": path.split("/")[-1], "url": url, "sha256": sha, "size": size}


def gh_asset(repo, tag, name):
    rel = get_json(f"https://api.github.com/repos/{repo}/releases/tags/{tag}")
    a = next(a for a in rel["assets"] if a["name"] == name)
    digest = (a.get("digest") or "").removeprefix("sha256:")
    if not digest:
        digest, _ = hash_url(a["browser_download_url"])
    return {"name": name, "url": a["browser_download_url"], "sha256": digest, "size": a["size"]}


def item(id, slot, label, description, license, files, platform="any"):
    return {"id": id, "slot": slot, "label": label, "description": description, "license": license, "platform": platform, "files": files}


def main():
    parakeet = "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8"
    items = [
        item("stt-parakeet-tdt-0.6b-v3-int8", "stt", "Speech recognition — Parakeet TDT 0.6B v3 (int8)",
             "Transcribes 25 European languages, including Italian, English and French. Runs on the CPU.", "CC-BY-4.0 (NVIDIA)",
             [hf_file(parakeet, f) for f in ("encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt")]),
        item("vad-silero", "vad", "Voice activity — Silero VAD", "Finds where people are speaking.", "MIT",
             [gh_asset("k2-fsa/sherpa-onnx", "asr-models", "silero_vad.onnx")]),
        item("spk-wespeaker-resnet34", "speaker", "Speaker recognition — WeSpeaker ResNet34",
             "Tells voices apart so each person gets their own label.", "Apache-2.0",
             [gh_asset("k2-fsa/sherpa-onnx", "speaker-recongition-models", "wespeaker_en_voxceleb_resnet34.onnx")]),
        item("mt-hy-mt2-1.8b-q4km", "translate", "Translation — Hy-MT2 1.8B (Q4_K_M)",
             "Live translation between 33 languages. Loaded only while translation is on.", "Apache-2.0 (Tencent)",
             [hf_file("tencent/Hy-MT2-1.8B-GGUF", "Hy-MT2-1.8B-Q4_K_M.gguf")]),
        item("sum-qwen3.5-2b-q4km", "summarize", "Summaries — Qwen3.5 2B (Q4_K_M)",
             "Compact model for 8 GB laptops.", "Apache-2.0 (Alibaba)", [hf_file("unsloth/Qwen3.5-2B-GGUF", "Qwen3.5-2B-Q4_K_M.gguf")]),
        item("sum-qwen3.5-4b-q4km", "summarize", "Summaries — Qwen3.5 4B (Q4_K_M)",
             "Better notes on laptops with 16 GB or more.", "Apache-2.0 (Alibaba)", [hf_file("unsloth/Qwen3.5-4B-GGUF", "Qwen3.5-4B-Q4_K_M.gguf")]),
        item("rt-llama-cpp-windows", "runtime", f"Language-model runtime — llama.cpp {LLAMA_TAG}",
             "Runs the translation and summary models on your CPU.", "MIT",
             [dict(gh_asset("ggml-org/llama.cpp", LLAMA_TAG, f"llama-{LLAMA_TAG}-bin-win-cpu-x64.zip"), extract=True)], platform="windows"),
    ]
    OUT.parent.mkdir(exist_ok=True)
    OUT.write_text(json.dumps({"version": 1, "items": items}, indent=2) + "\n")
    total = sum(f["size"] for i in items for f in i["files"])
    print(f"wrote {OUT} — {len(items)} items, {total / 1e9:.2f} GB if everything is installed")


if __name__ == "__main__":
    sys.exit(main())
