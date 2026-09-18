"""Adversarial local HTTP, CLI, subprocess and worker integration checks.

Uses only controlled local content. Does not authenticate to public providers.
"""
import argparse
import concurrent.futures
import hashlib
import http.server
import json
import pathlib
import socketserver
import subprocess
import sys
import tempfile
import threading
import time

from verify import Fixture


class EdgeFixture(Fixture):
    media = b""

    def do_GET(self):
        if self.path == "/watch":
            body = b'<html><head><title>Controlled media</title></head><body><video src="/media.mp4" controls></video></body></html>'
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path == "/media.mp4":
            self.send_response(200)
            self.send_header("Content-Type", "video/mp4")
            self.send_header("Content-Length", str(len(self.media)))
            self.end_headers()
            self.wfile.write(self.media)
            return
        super().do_GET()

    def send_header(self, keyword, value):
        if self.headers.get("Range"):
            if keyword == "Content-Range" and self.path == "/slow-bad-range.bin":
                value = value.replace("bytes ", "bytes 9", 1)
            if keyword == "Content-Range" and self.path == "/slow-short-range.bin":
                start = int(self.headers["Range"].split("=")[1].split("-")[0])
                value = f"bytes {start}-{len(self.payload)-2}/{len(self.payload)}"
            if keyword == "ETag" and self.path == "/slow-changed.bin":
                value = '"fixture-v2"'
        super().send_header(keyword, value)


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--artifacts", required=True)
    args = parser.parse_args()
    binary = str(pathlib.Path(args.binary).resolve())
    artifacts = pathlib.Path(args.artifacts).resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    root = pathlib.Path(tempfile.mkdtemp(prefix="edges-", dir=artifacts))
    profile, output = root / "profile", root / "Downloads Unicode \u4e0b\u8f7d"
    output.mkdir()
    passed, skipped = [], []
    server = Server(("127.0.0.1", 0), EdgeFixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_port}"

    def record(message):
        passed.append(message)
        print(f"PASS {message}", flush=True)

    def cli(*argv, check=True, timeout=20):
        result = subprocess.run([binary, "--data-dir", str(profile), "--json", *map(str, argv)],
                                capture_output=True, text=True, encoding="utf-8", timeout=timeout)
        records = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        if check:
            assert result.returncode == 0, (argv, result.stderr, records)
        return records, result

    def data(*argv):
        return cli(*argv)[0][-1]["data"]

    def job(job_id):
        return next(j for j in data("queue", "list") if j["id"] == job_id)

    def wait(job_id, state="completed"):
        end = time.monotonic() + 30
        while time.monotonic() < end:
            current = job(job_id)
            if current["status"] == state:
                return current
            assert current["status"] not in {"failed", "cancelled", "completed"}, current
            time.sleep(.1)
        raise AssertionError((state, current))

    def held(route):
        EdgeFixture.gates[route] = threading.Event()
        started = time.monotonic()
        job_id = data("download", base + route, "--direct", "--output", output, "--detach")[0]
        # The response must finish while the server is deliberately blocked.
        assert time.monotonic() - started < 15, "Detached CLI waited for its worker"
        current = wait(job_id, "active")
        end = time.monotonic() + 10
        while current["downloaded"] == 0 and time.monotonic() < end:
            time.sleep(.1)
            current = job(job_id)
        assert current["downloaded"] > 0, current
        return job_id

    try:
        # Simultaneous startup must converge on one profile worker.
        with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
            results = list(pool.map(lambda _: data("settings"), range(6)))
        assert len(results) == 6
        with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
            pids = list(pool.map(lambda _: data("worker", "status")["pid"], range(6)))
        assert len(set(pids)) == 1, pids
        record("Concurrent clients share one worker")

        for route in ["/slow-bad-range.bin", "/slow-short-range.bin", "/slow-changed.bin"]:
            job_id = held(route)
            data("queue", "pause", job_id)
            partial = next(pathlib.Path(job(job_id)["destination"]).glob("*.part"))
            original = partial.read_bytes()
            EdgeFixture.gates[route].set()
            data("queue", "resume", job_id)
            failed = wait(job_id, "failed")
            assert not failed["resume_supported"]
            assert partial.read_bytes() == original, "Bad response appended to a partial file"
            _, result = cli("queue", "retry", job_id, check=False)
            assert result.returncode != 0
        record("Invalid ranges and changed validators preserve partial bytes and require restart")

        job_id = held("/slow-cancel.bin")
        data("queue", "cancel", job_id)
        EdgeFixture.gates["/slow-cancel.bin"].set()
        current = wait(job_id, "cancelled")
        time.sleep(.3)
        assert job(job_id)["status"] == "cancelled"
        assert list(pathlib.Path(current["destination"]).glob("*.part"))
        assert not current["files"]
        record("Cancellation retains partials and cannot be overwritten by completion")

        job_id = held("/slow-clobber.bin")
        current = job(job_id)
        target = pathlib.Path(current["destination"]) / "slow-clobber.bin"
        target.write_bytes(b"existing user file")
        EdgeFixture.gates["/slow-clobber.bin"].set()
        wait(job_id, "failed")
        assert target.read_bytes() == b"existing user file"
        assert list(target.parent.glob("*.part"))
        record("Destination created mid-transfer is never overwritten")

        blocked = root / "not-a-directory"
        blocked.write_text("preserve", encoding="utf-8")
        records, result = cli("download", base + "/file.bin", "--direct", "--output", blocked, check=False)
        assert result.returncode != 0 and result.stderr
        assert any(r["type"] == "error" for r in records)
        assert blocked.read_text(encoding="utf-8") == "preserve"
        record("Write failures return nonzero with parseable JSON and stderr diagnostics")

        job_id = held("/slow-stop.bin")
        data("worker", "stop")
        assert not data("worker", "status")["running"]
        EdgeFixture.gates["/slow-stop.bin"].set()
        assert job(job_id)["status"] == "paused"
        data("queue", "resume", job_id)
        completed = wait(job_id)
        assert pathlib.Path(completed["files"][0]).read_bytes() == EdgeFixture.payload
        record("Stopping active worker pauses safely and resumes after reconnect")

        manifest = root / "fixture-plugin.json"
        manifest.write_text(json.dumps({"protocol": 1, "id": "fixture", "name": "Local fixture",
                                       "executable": sys.executable,
                                       "args": [str(pathlib.Path(__file__).with_name("fixture_plugin.py").resolve())],
                                       "capabilities": ["inspect", "download", "cancel"]}), encoding="utf-8")
        data("plugins", "add", manifest)
        assert data("plugins", "inspect", "fixture", base + "/fixture")["title"] == "Fixture plugin"
        job_id = data("plugins", "run", "fixture", base + "/fixture", "--output", output, "--detach")[0]
        completed = wait(job_id)
        assert pathlib.Path(completed["files"][0]).read_bytes() == b"fixture"
        for route in ["/invalid", "/escape"]:
            job_id = data("plugins", "run", "fixture", base + route, "--output", output, "--detach")[0]
            wait(job_id, "failed")
        record("Plugin discovery, metadata, execution, malformed events and output boundaries")

        for stop in ["cancel", "crash"]:
            job_id = data("plugins", "run", "fixture", base + "/hang", "--output", output, "--detach")[0]
            current = wait(job_id, "active")
            heartbeat = pathlib.Path(current["destination"]) / "heartbeat"
            deadline = time.monotonic() + 10
            while not heartbeat.exists() and time.monotonic() < deadline:
                time.sleep(.1)
            assert heartbeat.exists()
            if stop == "cancel":
                data("queue", "cancel", job_id)
                wait(job_id, "cancelled")
            else:
                import os
                pid = data("worker", "status")["pid"]
                if sys.platform == "win32":
                    subprocess.run(["taskkill", "/PID", str(pid), "/F"], check=True, capture_output=True)
                else:
                    os.kill(pid, 9)
            time.sleep(.6)
            stamp = heartbeat.read_bytes()
            time.sleep(.3)
            assert heartbeat.read_bytes() == stamp, "Dependency grandchild survived cleanup"
            if stop == "crash":
                assert job(job_id)["status"] == "paused"
        record("Plugin parent and grandchild cleanup on cancellation and worker crash")

        dependencies = data("doctor")
        ffmpeg = next((d["path"] for d in dependencies if d["name"] == "ffmpeg"), None)
        ytdlp = next((d["path"] for d in dependencies if d["name"] == "yt-dlp"), None)
        if ffmpeg and ytdlp:
            media = root / "fixture.mp4"
            subprocess.run([ffmpeg, "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                            "color=c=cyan:s=160x90:d=1", "-c:v", "libx264", "-pix_fmt", "yuv420p", str(media)], check=True)
            EdgeFixture.media = media.read_bytes()
            metadata = data("info", base + "/watch")
            assert metadata["title"]
            job_id = data("download", base + "/watch", "--output", output, "--detach")[0]
            completed = wait(job_id)
            assert hashlib.sha256(pathlib.Path(completed["files"][0]).read_bytes()).digest() == hashlib.sha256(EdgeFixture.media).digest()
            record("Real yt-dlp inspection and media download from controlled HTML")
        else:
            skipped.append("Real yt-dlp media test requires yt-dlp and FFmpeg")

        result = {"passed": len(passed), "checks": passed, "skipped": skipped, "run_dir": str(root), "platform": sys.platform}
        (artifacts / "edges.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        print(json.dumps(result, indent=2), flush=True)
    finally:
        for gate in EdgeFixture.gates.values():
            gate.set()
        cli("worker", "stop", check=False)
        server.shutdown()


if __name__ == "__main__":
    main()
