"""Captures runs from a running `board-watch serve-fake` into a replay fixture.

Kept next to the fixture it produces so a stale recording can be refreshed
rather than hand-edited:

    board-watch serve-fake --port 8192 &
    python3 examples/board-watch/fixtures/capture.py 8192 > examples/board-watch/fixtures/chunked-run.json
"""

import json
import sys
import urllib.request

PORT = sys.argv[1] if len(sys.argv) > 1 else "8090"
SCENARIOS = ["call", "chunks"]


def capture(said, run_id):
    body = json.dumps(
        {
            "threadId": "replay",
            "runId": run_id,
            "messages": [{"role": "user", "id": "m-" + run_id, "content": said}],
            "tools": [],
            "context": [],
            "state": {},
        }
    ).encode()
    request = urllib.request.Request(
        f"http://127.0.0.1:{PORT}/agent",
        data=body,
        headers={"content-type": "application/json"},
    )
    events = []
    with urllib.request.urlopen(request) as response:
        for line in response.read().decode().splitlines():
            if line.startswith("data: "):
                event = json.loads(line[6:])
                # Timestamps are wall-clock; a fixture that carries them is a
                # fixture whose diff is noise.
                event.pop("timestamp", None)
                events.append(event)
    return events


runs = [capture(said, f"replay-run-{index}") for index, said in enumerate(SCENARIOS, 1)]
print(json.dumps(runs, indent=2))
