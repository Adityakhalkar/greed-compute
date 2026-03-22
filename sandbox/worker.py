#!/usr/bin/env python3
"""
greed-compute sandbox worker.

Long-lived process that receives JSON commands on stdin,
executes user code in a restricted environment, and returns
JSON results on stdout.
"""

import json
import io
import os
import sys
import time
import signal
import builtins
import resource

# ── Blocked modules ──────────────────────────────────────────────────────────

BLOCKED_MODULES = frozenset({
    "subprocess", "shutil", "socket", "http", "urllib",
    "ftplib", "smtplib", "telnetlib", "xmlrpc",
    "ctypes", "multiprocessing", "threading",
    "code", "codeop", "compileall", "pkgutil", "zipimport",
})

# Pre-load ML libraries BEFORE installing the import hook.
# These libraries internally use blocked modules (ctypes, threading, etc.)
# which is fine — we only want to block USER code from importing them.
_preloaded_np = None
_preloaded_pd = None
try:
    import numpy as _preloaded_np
except ImportError:
    pass
try:
    import pandas as _preloaded_pd
except ImportError:
    pass
try:
    import sklearn as _preloaded_sklearn
except ImportError:
    pass
try:
    import matplotlib as _preloaded_mpl
except ImportError:
    pass
try:
    import scipy as _preloaded_scipy
except ImportError:
    pass

_original_import = builtins.__import__

# Neuter dangerous modules that were pulled in as dependencies.
# They're in sys.modules but we replace them with dummy objects so
# user code can't call their functions.
NEUTERED_MODULES = {"subprocess", "shutil", "socket", "multiprocessing"}

class _NeuteredModule:
    """A dummy module that raises on any attribute access."""
    def __init__(self, name):
        self._name = name
    def __getattr__(self, attr):
        if attr.startswith("_"):
            return object.__getattribute__(self, attr)
        raise PermissionError(
            f"Module '{self._name}' is disabled in greed-compute sandbox"
        )

for _mod_name in NEUTERED_MODULES:
    if _mod_name in sys.modules:
        sys.modules[_mod_name] = _NeuteredModule(_mod_name)


def _restricted_import(name, *args, **kwargs):
    top_level = name.split(".")[0]
    if top_level in BLOCKED_MODULES:
        raise ImportError(
            f"Module '{top_level}' is blocked in greed-compute sandbox"
        )
    return _original_import(name, *args, **kwargs)


# ── Resource limits ──────────────────────────────────────────────────────────

def apply_resource_limits():
    max_mem_mb = int(os.environ.get("GREED_MAX_MEMORY_MB", "512"))
    max_bytes = max_mem_mb * 1024 * 1024
    try:
        resource.setrlimit(resource.RLIMIT_AS, (max_bytes, max_bytes))
    except (ValueError, OSError):
        pass  # some platforms don't support RLIMIT_AS

    workspace = os.environ.get("GREED_WORKSPACE")
    if workspace:
        os.makedirs(workspace, exist_ok=True)
        os.chdir(workspace)


def get_cpu_timeout():
    return int(os.environ.get("GREED_MAX_CPU_SECONDS", "30"))


# ── Timeout handler ──────────────────────────────────────────────────────────

class ExecutionTimeout(Exception):
    pass


def _alarm_handler(signum, frame):
    raise ExecutionTimeout("Execution timed out")


# ── Output helpers ───────────────────────────────────────────────────────────

def emit(obj):
    """Write a single JSON line to stdout and flush immediately."""
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


# ── Session state ────────────────────────────────────────────────────────────

def make_session_globals():
    """Create a fresh session globals dict with numpy/pandas pre-loaded."""
    g = {"__builtins__": builtins}
    if _preloaded_np is not None:
        g["np"] = _preloaded_np
    if _preloaded_pd is not None:
        g["pd"] = _preloaded_pd
    return g


# ── Command handlers ────────────────────────────────────────────────────────

def handle_ping():
    emit({"type": "pong"})


def handle_clear(session_globals):
    session_globals.clear()
    session_globals.update(make_session_globals())
    emit({"type": "cleared"})


def handle_execute(code, session_globals):
    timeout = get_cpu_timeout()
    captured = io.StringIO()
    start = time.monotonic()
    error = None

    # Set alarm for timeout
    old_handler = signal.signal(signal.SIGALRM, _alarm_handler)
    signal.alarm(timeout)

    try:
        old_stdout = sys.stdout
        old_stderr = sys.stderr
        sys.stdout = captured
        sys.stderr = captured
        try:
            # NOTE: exec() is intentional — this is a sandbox worker whose
            # entire purpose is to run user-submitted code within a restricted
            # environment (blocked imports, resource limits, timeouts).
            exec(code, session_globals)  # noqa: S102
        finally:
            sys.stdout = old_stdout
            sys.stderr = old_stderr
    except ExecutionTimeout:
        error = f"Execution timed out after {timeout}s"
    except Exception as e:
        error = f"{type(e).__name__}: {e}"
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, old_handler)

    duration_ms = int((time.monotonic() - start) * 1000)
    emit({
        "type": "result",
        "stdout": captured.getvalue(),
        "error": error,
        "duration_ms": duration_ms,
    })


# ── Main loop ───────────────────────────────────────────────────────────────

def main():
    # 1. Apply resource limits
    apply_resource_limits()

    # 2. Install import hook
    builtins.__import__ = _restricted_import

    # 3. Pre-load numpy/pandas into session globals
    session_globals = make_session_globals()

    # 4. Signal readiness
    emit({"type": "ready"})

    # 5. Main loop — read JSON lines from stdin
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue

        try:
            msg = json.loads(line)
        except json.JSONDecodeError as e:
            emit({"type": "result", "stdout": "", "error": f"Invalid JSON: {e}", "duration_ms": 0})
            continue

        msg_type = msg.get("type")

        if msg_type == "ping":
            handle_ping()
        elif msg_type == "clear":
            handle_clear(session_globals)
        elif msg_type == "execute":
            code = msg.get("code", "")
            handle_execute(code, session_globals)
        else:
            emit({"type": "result", "stdout": "", "error": f"Unknown command type: {msg_type}", "duration_ms": 0})


if __name__ == "__main__":
    main()
