"""Check real solver frames through HTTP using disposable, isolated sessions.

This checks the wire protocol, not browser/WebGL rendering. No page automation.
"""
import argparse
import base64
import gzip
import hashlib
import json
import statistics
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default="http://127.0.0.1:18794/map")
    parser.add_argument("--out", default="artifacts/live-acceptance")
    parser.add_argument("--shape", choices=["sphere", "plane"], default="sphere")
    args = parser.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    base = args.base.rstrip("/") + "/api/v1"
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    tokens = []
    report = {"shape": args.shape, "frames": [], "result": "incomplete"}

    def call(path, token=None, method="GET", data=None, expected=200):
        headers = {"Content-Type": "application/json"}
        if token:
            headers["Authorization"] = "Bearer " + token
        payload = json.dumps(data).encode() if data is not None else None
        request = urllib.request.Request(base + path, data=payload, headers=headers, method=method)
        try:
            with opener.open(request, timeout=120) as response:
                status, body = response.status, response.read()
        except urllib.error.HTTPError as exc:
            status, body = exc.code, exc.read()
        assert status == expected, (method, path, status, body[:200])
        return json.loads(body) if status == 200 else None

    def events(job_id, token, compressed=True):
        encoding = "&frame_encoding=gzip" if compressed else ""
        request = urllib.request.Request(
            base + "/events?job_id=" + job_id + encoding,
            headers={"Authorization": "Bearer " + token},
        )
        with opener.open(request, timeout=120) as response:
            event, data = "message", []
            for line in response:
                line = line.decode().strip()
                if line.startswith("event:"):
                    event = line[6:].strip()
                elif line.startswith("data:"):
                    data.append(line[5:].strip())
                elif not line and data:
                    yield event, json.loads("\n".join(data))
                    event, data = "message", []

    def decode(frame):
        packed = base64.b64decode(frame["data"], validate=True)
        pixels = gzip.decompress(packed) if frame["fmt"].startswith("gzip-") else packed
        assert len(pixels) == frame["w"] * frame["h"] * 5
        return pixels, len(packed)

    try:
        tokens.extend(call("/session", method="POST")["token"] for _ in range(2))
        token, other = tokens
        world = call("/worlds", token, "POST", {
            "name": "Live stream acceptance " + args.shape,
            "seed": 42,
            "params": {"world_shape": args.shape},
        })
        result = call("/worlds/" + world["id"] + "/generate", token, "POST")
        job_id = result["job_id"]
        call("/events?job_id=" + job_id, other, expected=404)
        started = time.monotonic()
        stream = events(job_id, token)
        latest_tick = -1
        last_progress = -1.0
        replay_checks = []
        replay_errors = []

        def reconnect(min_tick):
            try:
                begin = time.monotonic()
                replay = events(job_id, token, compressed=False)
                try:
                    for kind, value in replay:
                        if kind == "frame":
                            pixels, size = decode(value)
                            assert value["fmt"].startswith("raw-") and value["tick"] >= min_tick
                            replay_checks.append({"tick": value["tick"], "seconds": time.monotonic() - begin,
                                                  "raw_bytes": size, "hash": hashlib.sha256(pixels).hexdigest()})
                            return
                finally:
                    replay.close()
            except Exception as exc:
                replay_errors.append(repr(exc))

        replay_thread = None
        try:
            for kind, frame in stream:
                assert time.monotonic() - started < 900, "generation timeout"
                if kind == "frame":
                    if frame["tick"] <= latest_tick:
                        continue  # Cached replay may race with the live queue.
                    latest_tick = frame["tick"]
                    pixels, size = decode(frame)
                    meta = frame["meta"]
                    assert meta["progress"] + 1e-6 >= last_progress, (last_progress, meta)
                    last_progress = meta["progress"]
                    item = {"seconds": round(time.monotonic() - started, 4), "tick": latest_tick,
                            "compressed_bytes": size, "raw_bytes": len(pixels), "meta": meta,
                            "height_hash": hashlib.sha256(pixels[0::5] + pixels[1::5]).hexdigest(),
                            "pixels_hash": hashlib.sha256(pixels).hexdigest()}
                    report["frames"].append(item)
                    if len(report["frames"]) == 3:
                        replay_thread = threading.Thread(target=reconnect, args=(latest_tick,))
                        replay_thread.start()
                    if len(report["frames"]) in (1, 5) or meta["year"] is not None and meta["month"] == 6:
                        print(json.dumps({"frame": len(report["frames"]), **item}, ensure_ascii=False), flush=True)
                elif kind in ("done", "failed"):
                    assert kind == "done", frame
                    break
        finally:
            stream.close()
        if replay_thread:
            replay_thread.join(5)
        assert replay_checks and not replay_errors, (replay_checks, replay_errors)
        job = call("/jobs/" + job_id, token)
        assert job["state"] == "done", job
        node = call("/maps/" + result["map_id"], token)
        assert node["status"] == "done" and node["grid"]["type"] == args.shape, node
        frames = report["frames"]
        tectonics = [f for f in frames if f["meta"]["elapsed_myr"] is not None]
        assert len(tectonics) >= 3 and len({f["height_hash"] for f in tectonics}) >= 3, tectonics
        assert all(a["meta"]["elapsed_myr"] < b["meta"]["elapsed_myr"] for a, b in zip(tectonics, tectonics[1:]))
        assert any(f["meta"]["month"] is not None for f in frames), "missing in-solver climate frames"
        assert len(frames) >= 8
        gaps = [b["seconds"] - a["seconds"] for a, b in zip(frames, frames[1:])]
        report.update({"seconds": round(time.monotonic() - started, 3), "cells": node["cells"],
                       "replay": replay_checks, "frame_count": len(frames), "tectonic_frame_count": len(tectonics),
                       "median_gap_seconds": statistics.median(gaps), "maximum_gap_seconds": max(gaps),
                       "compressed_bytes": sum(f["compressed_bytes"] for f in frames),
                       "raw_bytes": sum(f["raw_bytes"] for f in frames)})
        # Refinement uses inherited climate rather than simulating a second planet.
        child = call("/maps/" + node["id"] + "/children", token, "POST", {
            "kind": "continent", "center_lat": node["grid"].get("center_lat", 0),
            "center_lon": node["grid"].get("center_lon", 0), "width_m": 128000,
        })
        child_stream = events(child["job_id"], token)
        child_frames = []
        try:
            for kind, value in child_stream:
                if kind == "frame":
                    decode(value)
                    assert value["grid"]["type"] != "sphere"
                    child_frames.append({"tick": value["tick"], "meta": value["meta"]})
                elif kind in ("done", "failed"):
                    assert kind == "done", value
                    break
        finally:
            child_stream.close()
        assert len(child_frames) >= 2, child_frames
        assert call("/jobs/" + child["job_id"], token)["state"] == "done"
        report["child_frames"] = child_frames
        # Cancellation while previewing must finish as cancelled, without saving a completed map.
        pending = call("/worlds/" + world["id"] + "/generate", token, "POST")
        cancel_stream = events(pending["job_id"], token)
        try:
            for kind, frame in cancel_stream:
                if kind == "frame":
                    assert call("/jobs/" + pending["job_id"] + "/cancel", token, "POST")["ok"]
                    break
        finally:
            cancel_stream.close()
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            cancelled = call("/jobs/" + pending["job_id"], token)
            if cancelled["state"] == "cancelled":
                break
            assert cancelled["state"] != "done", cancelled
            time.sleep(.1)
        assert cancelled["state"] == "cancelled", cancelled
        assert call("/maps/" + pending["map_id"], token)["status"] == "cancelled"
        for terminal_id, expected_kind in [(job_id, "done"), (pending["job_id"], "cancelled")]:
            terminal_events = list(events(terminal_id, token))
            assert terminal_events and terminal_events[-1][0] == expected_kind, terminal_events
        report["result"] = "passed"
        print(json.dumps({k: v for k, v in report.items() if k != "frames"}, ensure_ascii=False, indent=2))
    finally:
        for token in tokens:
            try:
                call("/session", token, "DELETE")
            except Exception:
                pass
        (out / (args.shape + ".json")).write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
