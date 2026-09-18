"""Test-only plugin for exercising protocol boundaries and process cleanup."""
import argparse
import json
import pathlib
import subprocess
import sys
import time

parser = argparse.ArgumentParser()
parser.add_argument("--omni-request", required=True)
request = json.loads(pathlib.Path(parser.parse_args().omni_request).read_text(encoding="utf-8"))
assert request["version"] == 1
if request["op"] == "inspect":
    print(json.dumps({"type": "metadata", "data": {"title": "Fixture plugin", "source": request["spec"]["source"],
                                                  "duration": None, "size": 7, "formats": ["bin"], "entries": []}}))
else:
    destination = pathlib.Path(request["destination"])
    if request["spec"]["source"].endswith("/hang"):
        # Grandchild survives a parent-only kill, so heartbeat verifies tree cleanup.
        heartbeat = destination / "heartbeat"
        child_code = "import pathlib,sys,time\np=pathlib.Path(sys.argv[1])\nwhile True:\n p.write_text(str(time.time()))\n time.sleep(.05)"
        child = subprocess.Popen([sys.executable, "-c", child_code, str(heartbeat)])
        (destination / "child.pid").write_text(str(child.pid))
        print(json.dumps({"type": "progress", "downloaded": 1, "total": None, "speed": 1}), flush=True)
        time.sleep(120)
    elif request["spec"]["source"].endswith("/invalid"):
        print("not JSON", flush=True)
    elif request["spec"]["source"].endswith("/escape"):
        output = pathlib.Path(request["data_dir"]) / "fixture-outside.bin"
        output.write_bytes(b"fixture")
        print(json.dumps({"type": "complete", "files": [str(output)]}), flush=True)
    else:
        output = destination / "fixture.bin"
        output.write_bytes(b"fixture")
        print(json.dumps({"type": "progress", "downloaded": 7, "total": 7, "speed": 7}), flush=True)
        print(json.dumps({"type": "complete", "files": [str(output)]}), flush=True)
