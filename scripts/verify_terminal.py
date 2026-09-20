"""Windows ConPTY smoke tests with a real interactive process and mouse input.

Test dependencies: pywinpty, pyte; Pillow is optional for rendered captures.
Install in a project-local directory and expose it through PYTHONPATH.
"""
import argparse
import json
import os
import pathlib
import socket
import subprocess
import tempfile
import threading
import time

import pyte
from winpty import PtyProcess
from verify_edges import EdgeFixture, Server


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--artifacts", required=True)
    args = parser.parse_args()
    binary = str(pathlib.Path(args.binary).resolve())
    artifacts = pathlib.Path(args.artifacts).resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    root = pathlib.Path(tempfile.mkdtemp(prefix="terminal-", dir=artifacts))
    profile = root / "profile"
    server = Server(("127.0.0.1", 0), EdgeFixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_port}"
    passed = []
    proc = None
    screen = stream = None

    def cli(*argv):
        result = subprocess.run([binary, "--data-dir", str(profile), "--json", *map(str, argv)],
                                capture_output=True, text=True, encoding="utf-8", timeout=20)
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout.splitlines()[-1])["data"]

    def start(no_color=False):
        nonlocal proc, screen, stream
        environment = dict(os.environ)
        if no_color:
            environment["NO_COLOR"] = "1"
        proc = PtyProcess.spawn([binary, "--data-dir", str(profile)], dimensions=(35, 120), env=environment)
        proc.fileobj.settimeout(.1)
        screen = pyte.Screen(120, 35)
        stream = pyte.Stream(screen)
        wait_text("Paste a URL, local file")

    def pump():
        try:
            data = proc.read(65536)
            stream.feed(data)
            with (root / "session.ansi").open("a", encoding="utf-8") as log:
                log.write(data)
        except (socket.timeout, EOFError):
            pass

    def visible():
        return "\n".join(screen.display)

    def wait_text(text, timeout=12):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            pump()
            if text in visible():
                return
            assert proc.isalive(), (text, visible())
        (root / "failure.txt").write_text(visible(), encoding="utf-8")
        raise AssertionError(("Screen did not show", text, visible()))

    def send(keys):
        proc.write(keys)
        time.sleep(.15)
        pump()

    def click(label):
        for y, line in enumerate(screen.display):
            x = line.find(label)
            if x >= 0:
                send(f"\x1b[<0;{x+2};{y+1}M\x1b[<0;{x+2};{y+1}m")
                return
        raise AssertionError(("Missing click target", label, visible()))

    def capture(name):
        (root / f"{name}.txt").write_text(visible(), encoding="utf-8")
        try:
            from PIL import Image, ImageDraw, ImageFont
        except ImportError:
            return
        font = ImageFont.truetype("C:/Windows/Fonts/consola.ttf", 17)
        image = Image.new("RGB", (screen.columns * 11, screen.lines * 22), "#0b1017")
        draw = ImageDraw.Draw(image)
        for y in range(screen.lines):
            for x in range(screen.columns):
                cell = screen.buffer[y][x]
                fg = "#" + cell.fg if len(cell.fg) == 6 else "#e1eaf3"
                bg = "#" + cell.bg if len(cell.bg) == 6 else "#0b1017"
                draw.rectangle((x*11, y*22, x*11+10, y*22+21), fill=bg)
                draw.text((x*11, y*22), cell.data, font=font, fill=fg)
        image.save(root / f"{name}.png")

    def exited():
        deadline = time.monotonic() + 12
        while proc.isalive() and time.monotonic() < deadline:
            pump()
        assert not proc.isalive(), "TUI did not exit"

    try:
        cli("settings", "output", root / "downloads")
        start()
        capture("dashboard")
        click("Paste a URL, local file")
        send("\x1b[200~" + base + "/file.bin" + "\x1b[201~")
        send("\r")
        wait_text("Media preview")
        wait_text("Close")
        click("Close")
        wait_text(base)
        capture("add-download")
        click("Add to queue")
        wait_text("Added to your queue")
        passed.append("Mouse add/inspect/return/submit and bracketed paste")

        send("?")
        wait_text("Commands")
        send("\x1b")
        send("\x10")
        wait_text("View and control downloads")
        send("\x1b")
        passed.append("Ctrl+P global palette lists commands")
        proc.setwinsize(20, 60)
        screen.resize(20, 60)
        time.sleep(.3)
        pump()
        assert proc.isalive()
        capture("narrow")
        proc.setwinsize(35, 120)
        screen.resize(35, 120)
        time.sleep(.3)
        pump()
        passed.append("Keyboard help and terminal resize")

        route = "/slow-background.bin"
        EdgeFixture.gates[route] = threading.Event()
        job_id = cli("download", base + route, "--direct", "--detach")[0]
        time.sleep(.8)
        send("q")
        wait_text("[x] Keep downloads running")
        capture("exit-background")
        click("Exit")
        exited()
        assert cli("worker", "status")["running"]
        passed.append("Checked exit leaves worker running")

        start()
        wait_text("slow-background.bin")
        send("q")
        wait_text("[x] Keep downloads running")
        send(" ")
        wait_text("[ ] Keep downloads running")
        click("Exit")
        exited()
        current = next(j for j in cli("queue", "list") if j["id"] == job_id)
        assert current["status"] == "paused", current
        passed.append("Unchecked exit pauses pending downloads before closing")

        cli("queue", "resume", job_id)
        start()
        wait_text("slow-background.bin")
        proc.terminate(force=True)
        assert cli("worker", "status")["running"]
        EdgeFixture.gates[route].set()
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            current = next(j for j in cli("queue", "list") if j["id"] == job_id)
            if current["status"] == "completed":
                break
            time.sleep(.2)
        assert current["status"] == "completed", current
        passed.append("Abrupt TUI process termination preserves background transfer")
        start(no_color=True)
        wait_text("slow-background.bin")
        assert all(cell.fg == "default" and cell.bg == "default"
                   for row in screen.buffer.values() for cell in row.values())
        capture("monochrome")
        send("q")
        exited()
        passed.append("NO_COLOR keeps a usable monochrome terminal")
        result = {"passed": len(passed), "checks": passed, "run_dir": str(root),
                  "evidence": "Real Windows ConPTY; PNGs render the captured terminal buffer"}
        (artifacts / "terminal.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        print(json.dumps(result, indent=2))
    finally:
        if proc and proc.isalive():
            proc.terminate(force=True)
        for gate in EdgeFixture.gates.values():
            gate.set()
        cli("worker", "stop")
        server.shutdown()


if __name__ == "__main__":
    main()
