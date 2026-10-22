#!/usr/bin/env python3
"""Launch developer tools through an existing native terminal application.

Every path is passed as argv or shell-quoted into a private self-removing script.
The host's process-group cleanup cannot close the independently launched app.
"""
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.parse import unquote, urlparse

COMMANDS = [
    {"id":"open_nvim", "title":"Open in Neovim", "description":"Edit selected local files in your terminal", "shortcut":"ctrl-e"},
    {"id":"open_lazygit", "title":"Open Lazygit", "description":"Open Lazygit at the current local directory", "shortcut":"ctrl-g"},
    {"id":"open_terminal", "title":"Open terminal here", "description":"Start an interactive shell at the current local directory", "shortcut":"alt-enter"},
]


def local_path(value):
    if value.startswith("file://"):
        url = urlparse(value)
        if url.netloc not in ("", "localhost"):
            raise ValueError("Developer tools require a local path")
        value = unquote(url.path)
    elif "://" in value:
        raise ValueError("Developer tools require a local directory; connect or mount this location first")
    path = Path(value)
    if not path.is_absolute():
        raise ValueError("Developer tools require an absolute local path")
    if not path.exists():
        raise ValueError(f"Local path does not exist: {path}")
    return path


def executable(value, name):
    if not isinstance(value, str) or not value:
        raise ValueError(f"Configure {name} as an executable name or absolute path")
    found = shutil.which(value)
    if not found and sys.platform == "darwin" and "/" not in value:
        # Finder-launched apps often have a minimal PATH without Homebrew.
        found = shutil.which(value, path="/opt/homebrew/bin:/usr/local/bin")
    if not found:
        raise RuntimeError(f"{name} is unavailable. Install {value}, or configure its absolute path in the native-tools plugin config")
    return found


def command_script(command, arguments, config):
    directory = local_path(arguments["location"])
    if not directory.is_dir():
        directory = directory.parent
    selected = arguments.get("selected", arguments.get("selection", arguments.get("selected_files", [])))
    if not isinstance(selected, list) or not all(isinstance(item, str) for item in selected):
        raise ValueError("selected must be an array of local paths")
    if command == "open_nvim":
        argv = [executable(config.get("nvim", "nvim"), "Neovim"), "--"]
        argv.extend(str(local_path(path)) for path in selected)
    elif command == "open_lazygit":
        argv = [executable(config.get("lazygit", "lazygit"), "Lazygit")]
    elif command == "open_terminal":
        argv = [executable(config.get("shell", os.environ.get("SHELL") or "/bin/sh"), "Shell")]
    else:
        raise ValueError(f"Unknown developer tool command: {command}")
    # Absolute cwd cannot be parsed as a cd option. shlex.quote handles spaces,
    # quotes, newlines, command substitutions and any other shell metacharacters.
    path_setup = 'export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"\n' if sys.platform == "darwin" else ""
    body = "#!/bin/sh\nrm -f -- \"$0\"\n" + path_setup + "cd " + shlex.quote(str(directory)) + " || exit 1\nexec " + shlex.join(argv) + "\n"
    descriptor, name = tempfile.mkstemp(prefix="buckets-tool-", suffix=".command")
    try:
        os.fchmod(descriptor, 0o700)
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            stream.write(body)
    except BaseException:
        try:
            os.close(descriptor)
        except OSError:
            pass
        Path(name).unlink(missing_ok=True)
        raise
    return directory, Path(name)


def launch(command, arguments, config):
    directory, script = command_script(command, arguments, config)
    try:
        terminal_argv = config.get("terminal_argv")
        if terminal_argv is not None:
            if not isinstance(terminal_argv, list) or not terminal_argv or not all(isinstance(arg, str) for arg in terminal_argv):
                raise ValueError("terminal_argv must be a nonempty array of argument strings")
            if not any("{script}" in arg for arg in terminal_argv):
                raise ValueError("terminal_argv must include a {script} placeholder")
            argv = [arg.replace("{script}", str(script)).replace("{cwd}", str(directory)) for arg in terminal_argv]
            argv[0] = executable(argv[0], "Terminal")
            # A terminal launched directly must outlive this one-request plugin.
            subprocess.Popen(argv, cwd=directory, stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             start_new_session=True)
        elif sys.platform == "darwin":
            bundle_id = config.get("terminal_bundle_id")
            application = bundle_id or config.get("terminal_app", "com.googlecode.iterm2")
            if not isinstance(application, str) or not application:
                raise ValueError("terminal_bundle_id or terminal_app must be a macOS application identifier")
            selector = "-b" if bundle_id or application == "com.googlecode.iterm2" else "-a"
            result = subprocess.run(["/usr/bin/open", selector, application, str(script)],
                                    capture_output=True, text=True, timeout=3)
            if result.returncode:
                raise RuntimeError(f"Cannot open {application}: {result.stderr.strip()}")
        else:
            raise RuntimeError("Configure terminal_argv for your terminal, e.g. [\"x-terminal-emulator\",\"-e\",\"{script}\"]")
    except BaseException:
        script.unlink(missing_ok=True)
        raise
    return {"message":f"Launched {command}", "directory":str(directory)}


def handle(method, params):
    if method == "commands.list":
        return COMMANDS
    if method == "commands.invoke":
        request = params["request"]
        return launch(request["command"], request["arguments"], params.get("config") or {})
    raise ValueError(f"Unknown method {method}")


if __name__ == "__main__":
    request = json.loads(sys.stdin.buffer.readline(1_048_577))
    response = {"jsonrpc":"2.0", "id":request["id"]}
    try:
        response["result"] = handle(request["method"], request["params"])
    except Exception as error:
        response["error"] = {"code":-32000, "message":str(error)}
    print(json.dumps(response))
