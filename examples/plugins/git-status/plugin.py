#!/usr/bin/env python3
"""Generic row-decoration plugin backed by Git's NUL-separated status format."""
import json
import os
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote, urlparse


def local_path(value):
    if value.startswith("file://"):
        url = urlparse(value)
        if url.netloc not in ("", "localhost"):
            return None
        value = unquote(url.path)
    elif "://" in value:
        return None
    path = Path(value)
    if not path.is_absolute():
        return None
    path = Path(os.path.abspath(path))
    return path.parent.resolve() / path.name


def git(root, *arguments):
    return subprocess.run(["git", "--no-optional-locks", "-C", str(root), *arguments],
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=3)


def statuses(data):
    records = iter(data.split(b"\0"))
    changes = []
    for record in records:
        if not record:
            continue
        if len(record) < 4 or record[2:3] != b" ":
            raise ValueError("Git returned malformed porcelain status")
        status = record[:2].decode("ascii")
        destination = os.fsdecode(record[3:])
        changes.append((destination, status))
        if "R" in status or "C" in status:
            original = next(records, None)
            if original is None:
                raise ValueError("Git returned an incomplete rename record")
            # Rename origins matter when aggregating changes in another folder.
            if "R" in status:
                changes.append((os.fsdecode(original), status))
    return changes


def presentation(codes, count):
    if any("U" in code or code in ("AA", "DD") for code in codes):
        return "!", "danger", f"Git: merge conflict ({count} changed paths)"
    if all(code == "!!" for code in codes):
        return "!", "neutral", f"Git: ignored ({count} paths)"
    if all(code == "??" for code in codes):
        return "?", "accent", f"Git: untracked ({count} paths)"
    if count > 1:
        return "M", "warning", f"Git: {count} changed paths"
    code = codes[0]
    labels = {"M":"modified", "A":"added", "D":"deleted", "R":"renamed", "C":"copied", "T":"type changed", "?":"untracked", "!":"ignored"}
    descriptions = []
    if code[0] != " ":
        descriptions.append("staged " + labels.get(code[0], code[0]))
    if code[1] != " ":
        descriptions.append("unstaged " + labels.get(code[1], code[1]))
    return code.strip(), ("success" if code[1] == " " else "warning"), "Git: " + "; ".join(descriptions)


def decorate(params):
    request = params["request"]
    directory = local_path(request["location"])
    if directory is None:
        return []
    try:
        discovered = git(directory, "rev-parse", "--show-toplevel")
    except FileNotFoundError as error:
        raise RuntimeError("Git is unavailable; install git to enable row status") from error
    if discovered.returncode != 0:
        return []
    root = Path(os.fsdecode(discovered.stdout[:-1] if discovered.stdout.endswith(b"\n") else discovered.stdout))
    arguments = ["status", "--porcelain=v1", "-z", "--untracked-files=all"]
    if (params.get("config") or {}).get("show_ignored", False):
        arguments.append("--ignored=matching")
    result = git(root, *arguments)
    if result.returncode:
        raise RuntimeError(result.stderr.decode("utf-8", "replace").strip())
    if len(result.stdout) > 8_388_608:
        raise RuntimeError("Git status is too large for a single decoration request")
    changed = [(root / path, code) for path, code in statuses(result.stdout)]
    decorations = []
    for entry in request["entries"]:
        path = local_path(entry)
        if path is None:
            continue
        matching = [(changed_path, code) for changed_path, code in changed if changed_path == path or changed_path.is_relative_to(path)]
        if not matching:
            continue
        badge, tone, tooltip = presentation([code for _, code in matching], len(matching))
        decorations.append({"path": entry, "badge": badge, "tone": tone, "tooltip": tooltip})
    return decorations


if __name__ == "__main__":
    request = json.loads(sys.stdin.buffer.readline(1_048_577))
    response = {"jsonrpc":"2.0", "id":request["id"]}
    try:
        if request["method"] != "decorations":
            raise ValueError("Unsupported method")
        response["result"] = decorate(request["params"])
    except Exception as error:
        response["error"] = {"code":-32000, "message":str(error)}
    print(json.dumps(response))
