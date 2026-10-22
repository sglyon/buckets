#!/usr/bin/env python3
"""Buckets protocol v1 example. No third-party Python packages required.

Uses git's own ignore engine when respect_gitignore is requested. This supports
nested .gitignore, negation, global excludes and .git/info/exclude without a
second, subtly different implementation of Git's matching rules.
"""
import json
import os
import subprocess
import sys
from pathlib import Path


def candidates(root, respect_gitignore, follow_symlinks, maximum):
    if respect_gitignore:
        if follow_symlinks:
            raise ValueError("Git-backed traversal does not follow symlinks; set follow_symlinks=false")
        # A Git worktree is required to accurately apply all of its ignore rules.
        # Refuse instead of silently searching ignored files outside a worktree.
        command = ["git", "-C", str(root), "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "."]
        result = subprocess.run(command, capture_output=True, check=True, timeout=8)
        seen = set()
        for name in result.stdout.split(b"\0"):
            if not name:
                continue
            path = (root / os.fsdecode(name)).resolve()
            if path in seen or not path.is_relative_to(root) or not path.is_file():
                continue
            seen.add(path)
            yield path
            if len(seen) >= maximum:
                break
        return
    visited = set()
    count = 0
    for directory, dirs, files in os.walk(root, followlinks=follow_symlinks):
        resolved = Path(directory).resolve()
        if resolved in visited or not resolved.is_relative_to(root):
            dirs[:] = []
            continue
        visited.add(resolved)
        dirs[:] = sorted(d for d in dirs if d != ".git")
        for name in sorted(files):
            path = (Path(directory) / name).resolve()
            if not path.is_relative_to(root) or not path.is_file():
                continue
            count += 1
            if count > maximum:
                return
            yield path


def handle(method, params):
    if method != "search":
        raise ValueError(f"Unsupported method: {method}")
    request = params["request"]
    if request.get("mode", "filename") not in ("filename", "content"):
        raise ValueError("This example supports filename and content search; install a semantic plugin for other modes")
    query = request["query"].casefold()
    limit = min(request["limit"], 10000)
    maximum = min(int((params.get("config") or {}).get("max_files", 50000)), 50000)
    matches = []
    seen = set()
    for spec in request["roots"]:
        root = Path(spec["path"]).resolve(strict=True)
        for path in candidates(root, spec.get("respect_gitignore", True), spec.get("follow_symlinks", False), maximum):
            if path in seen:
                continue
            seen.add(path)
            line, snippet = None, None
            if request.get("mode", "filename") == "content":
                if path.stat().st_size > 1_048_576:
                    continue
                try:
                    raw = path.read_bytes()
                    if b"\0" in raw:
                        continue
                    text = raw.decode("utf-8")
                except (UnicodeError, OSError):
                    continue
                for number, content in enumerate(text.splitlines(), 1):
                    if query in content.casefold():
                        line, snippet = number, content[:240]
                        break
                if line is None:
                    continue
            elif query not in str(path.relative_to(root)).casefold():
                continue
            matches.append({"path": str(path), "score": 1.0, "line": line, "snippet": snippet, "kind": "file"})
            if len(matches) >= limit:
                return matches
    return matches


def main():
    request = json.loads(sys.stdin.buffer.readline(1_048_577))
    response = {"jsonrpc": "2.0", "id": request["id"]}
    try:
        response["result"] = handle(request["method"], request["params"])
    except Exception as error:
        response["error"] = {"code": -32000, "message": str(error)}
    print(json.dumps(response, ensure_ascii=True))


if __name__ == "__main__":
    main()
