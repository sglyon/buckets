#!/usr/bin/env python3
"""Deterministic native presentation fixture; never contacts a model or user account."""
import json
import pathlib
import sys
import threading
import time

ROOT = pathlib.Path(__file__).resolve().parents[3]
FILE = ROOT / 'files' / 'README.md'
FILE.parent.mkdir(parents=True, exist_ok=True)
# Service startup follows context admission; rewriting this file would change
# its metadata and correctly invalidate the host's request fingerprint.
if not FILE.exists():
    FILE.write_text('# Streaming fixture\n\nThe workspace adapter reads real local files.\nUse these notes to exercise citations.\n')
TEXT = '''## Workspace overview

The browser keeps **navigation**, **search**, and **file operations** separate. The current workspace has a small Rust host and an extensible plugin layer.

### What matters

- `buckets-core` owns provider-aware file operations.
- `buckets-plugin` validates the language-independent protocol.
- The native pane preserves **your place** while a reply arrives.

```rust
fn open_workspace(path: &str) -> Result<Workspace> {
    let location = Location::parse(path)?;
    Workspace::open(location)
}
```

The code block can be copied without losing whitespace. A long path such as `/workspace/components/streaming/very-long-module-name-with-explicit-boundaries.rs` should remain readable in a narrow pane.

| Area | Responsibility |
| --- | --- |
| Host | Paths and permissions |
| Plugin | Models and durable context |
| Pane | Reading, composing, and navigation |

### Suggested next step

Review the **workspace notes** and open the cited file below. Unicode is intact: café, λ, 東京, and 🙂.
'''
LONG = '\n\n'.join(f'### Detail {index}\n\nThis is a longer fixture paragraph for testing scroll-follow. Read an earlier section while the current answer continues to grow. The viewport should stay where you left it.' for index in range(1, 18))
lock = threading.Lock()
cancels = {}

def emit(frame):
    with lock:
        sys.stdout.write(json.dumps(frame, ensure_ascii=False) + '\n')
        sys.stdout.flush()

def run(frame):
    rid = frame['id']
    request = frame['params']['request']
    request_id = request['request_id']
    cancel = threading.Event()
    cancels[rid] = cancel
    cancels[request_id] = cancel
    prompt = request.get('prompt', '').lower()
    storage = pathlib.Path(request['storage_dir'])
    storage.mkdir(parents=True, exist_ok=True)
    if request.get('mode') == 'memory':
        memory = storage / 'fixture-notes.txt'
        if 'memory_update' in request:
            memory.write_text(request['memory_update'])
        emit({'jsonrpc': '2.0', 'id': rid, 'result': {'answer': 'Workspace memory', 'memory': memory.read_text() if memory.exists() else '', 'citations': [], 'results': []}})
        return
    answer = TEXT + ('\n\n' + LONG if 'long' in prompt else '')
    count = 140 if 'long' in prompt or 'slow' in prompt or 'cancel' in prompt else 55
    chunks = [answer[index * len(answer) // count:(index + 1) * len(answer) // count] for index in range(count)]
    def update(sequence, text, phase, state):
        emit({'jsonrpc': '2.0', 'method': 'agent.progress', 'params': {'request_id': request_id, 'message': '', 'update': {'sequence': sequence, 'text': text, 'phase': phase, 'activities': [{'id': 'read-fixture', 'title': 'Read README.md', 'state': state}]}}})
    update(1, '', 'reading', 'running')
    time.sleep(0.35)
    text = ''
    marker = storage / 'fixture-error-seen'
    try:
        for index, chunk in enumerate(chunks):
            if cancel.is_set():
                return
            text += chunk
            update(index + 2, text, 'responding', 'complete')
            time.sleep(0.09)
            if 'error' in prompt and index == 8 and not marker.exists():
                marker.write_text('retry may succeed\n')
                emit({'jsonrpc': '2.0', 'id': rid, 'error': {'code': -32000, 'message': 'Fixture connection interrupted. Retry keeps your prompt.'}})
                return
        update(len(chunks) + 2, answer, 'verifying', 'complete')
        time.sleep(0.25)
        if not cancel.is_set():
            emit({'jsonrpc': '2.0', 'id': rid, 'result': {'answer': answer, 'citations': [{'location': str(FILE), 'line': 3, 'label': 'README.md'}], 'results': [{'location': str(FILE), 'name': 'README.md', 'line': 3, 'reason': 'Native QA fixture reference', 'score': 1}]}})
    finally:
        cancels.pop(rid, None)
        cancels.pop(request_id, None)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get('method')
    rid = frame.get('id')
    if method == 'commands.list':
        emit({'jsonrpc': '2.0', 'id': rid, 'result': [{'id': 'qa.ask', 'title': 'Streaming QA: Ask', 'description': 'Deterministic native streaming fixture'}]})
    elif method == 'commands.invoke':
        emit({'jsonrpc': '2.0', 'id': rid, 'result': {'host_action': 'agent', 'request': {'mode': 'ask', 'prompt': 'Summarize this fixture workspace'}}})
    elif method == 'agent.invoke':
        threading.Thread(target=run, args=(frame,), daemon=True).start()
    elif method in ['agent.cancel', '$/cancelRequest']:
        params = frame.get('params', {})
        cancel = cancels.get(params.get('request_id', params.get('id')))
        if cancel:
            cancel.set()
    else:
        emit({'jsonrpc': '2.0', 'id': rid, 'error': {'code': -32601, 'message': 'Unknown fixture method'}})
