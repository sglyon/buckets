#!/usr/bin/env python3
"""Real FFF SDK integration (https://github.com/dmtrKovalenko/fff).

Protocol v1 is invocation-scoped, so this example rebuilds its index per search.
An eventual persistent protocol may reuse an index; no warm-index performance
claim is made here. FFF owns matching, scoring and .gitignore handling.
"""
import json
import sys
from pathlib import Path


def search(params):
    try:
        from fff import FileFinder
    except ImportError as error:
        raise RuntimeError("FFF dependency missing; run: uv pip install --python .venv/bin/python -r examples/plugins/fff-search/requirements.txt") from error
    request = params["request"]
    mode = request.get("mode", "filename")
    if mode not in ("filename", "content"):
        raise ValueError("FFF supports filename and content search; use a semantic plugin for semantic/hybrid modes")
    config = params.get("config") or {}
    matches = []
    seen = set()
    for spec in request["roots"]:
        if not spec.get("respect_gitignore", True):
            raise ValueError("FFF's Python SDK always follows ignore rules; use another engine to include ignored files")
        root = Path(spec["path"]).resolve(strict=True)
        with FileFinder(str(root), watch=False, enable_mmap_cache=False,
                        enable_content_indexing=mode == "content",
                        follow_symlinks=spec.get("follow_symlinks", False),
                        enable_home_dir_scanning=True) as finder:
            if not finder.wait_for_scan_blocking(timeout_ms=int(config.get("scan_timeout_ms", 8000))):
                raise TimeoutError(f"FFF index did not finish scanning {root}")
            remaining = request["limit"] - len(matches)
            if mode == "content":
                result = finder.grep(request["query"], mode="plain", page_limit=remaining,
                                     max_file_size=1_048_576, time_budget_ms=3000, enforce_time_budget=True)
                items = [(item, 1.0) for item in result.items]
            else:
                result = finder.search(request["query"], page_size=remaining)
                items = [(item, float(score.total)) for item, score in zip(result.items, result.scores)]
            for item, score in items:
                path = (root / item.relative_path).resolve()
                line = getattr(item, "line_number", None)
                key = (path, line)
                if key in seen or not path.is_relative_to(root):
                    continue
                seen.add(key)
                matches.append({"path": str(path), "score": score, "line": line,
                                "snippet": getattr(item, "line_content", None), "kind": "file"})
                if len(matches) >= request["limit"]:
                    return matches
    return matches


if __name__ == "__main__":
    request = json.loads(sys.stdin.buffer.readline(1_048_577))
    response = {"jsonrpc": "2.0", "id": request["id"]}
    try:
        if request["method"] != "search":
            raise ValueError(f"Unknown method {request['method']}")
        response["result"] = search(request["params"])
    except Exception as error:
        response["error"] = {"code": -32000, "message": str(error)}
    print(json.dumps(response))
