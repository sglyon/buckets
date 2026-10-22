#!/usr/bin/env python3
"""A real read-only provider. Open plugin://local-provider/ in Buckets.

Virtual paths are relative to config.root. Canonical containment checks reject
.. and escaping symlinks, including a .git link to another worktree directory.
"""
import json
import codecs
import sys
from pathlib import Path


def resolve(root, name):
    if "\0" in name:
        raise ValueError("NUL in provider path")
    path = (root / name.lstrip("/")).resolve(strict=True)
    if not path.is_relative_to(root):
        raise ValueError("Path escapes provider root")
    return path


def metadata(path):
    stat = path.stat()
    return {"name": path.name, "is_dir": path.is_dir(), "is_symlink": path.is_symlink(),
            "size": stat.st_size, "modified": int(stat.st_mtime)}


def handle(method, params):
    request = params["request"]
    config = params.get("config") or {}
    root = Path(config.get("root", ".")).resolve(strict=True)
    path = resolve(root, request["path"])
    if method == "provider.list":
        entries = []
        for item in path.iterdir():
            try:
                if item.resolve().is_relative_to(root):
                    entries.append(metadata(item))
            except OSError:
                continue
        return sorted(entries, key=lambda entry: (not entry["is_dir"], entry["name"].casefold()))
    if method == "provider.stat":
        return metadata(path)
    if method == "provider.read":
        maximum = min(int(request.get("max_bytes", 65536)), 1048576)
        if maximum < 0:
            raise ValueError("max_bytes must be non-negative")
        with path.open("rb") as stream:
            content = stream.read(maximum + 1)
        if b"\0" in content:
            raise ValueError("This text-only provider cannot read binary files")
        truncated = len(content) > maximum
        try:
            # A preview may end between UTF-8 code units. Buffer that incomplete
            # final character only when truncated; never replace invalid data.
            decoder = codecs.getincrementaldecoder("utf-8")(errors="strict")
            text = decoder.decode(content[:maximum], final=not truncated)
        except UnicodeDecodeError as error:
            raise ValueError("This text-only provider requires valid UTF-8; binary transfer is unsupported") from error
        return {"text": text, "truncated": truncated}
    raise ValueError("This example provider is read-only")


if __name__ == "__main__":
    request = json.loads(sys.stdin.buffer.readline(1048577))
    response = {"jsonrpc": "2.0", "id": request["id"]}
    try:
        response["result"] = handle(request["method"], request["params"])
    except Exception as error:
        response["error"] = {"code": -32000, "message": str(error)}
    print(json.dumps(response))
