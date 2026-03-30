#!/usr/bin/env python3
"""
greed-compute sandbox worker.

Long-lived process that receives JSON commands on stdin,
executes user code in a restricted environment, and returns
JSON results on stdout.
"""

import ast
import base64 as _base64
import json
import io
import os
import re
import sys
import time
import signal
import traceback
import builtins
import resource

# Capture subprocess BEFORE it gets neutered — used internally for pip only.
# User code never gets access to this reference.
import subprocess as _pip_subprocess

# ── Blocked modules ──────────────────────────────────────────────────────────

BLOCKED_MODULES = frozenset({
    # Network access — urllib is pre-loaded and neutered below instead of blocked
    # so that packages like seaborn can import it without making real network calls
    "socket", "http", "ftplib", "smtplib", "telnetlib", "xmlrpc",
    # Process spawning
    "subprocess", "multiprocessing",
    # Low-level / escape hatches
    "ctypes", "threading",
    "code", "codeop", "compileall", "pkgutil", "zipimport",
})

# Pre-load ML libraries BEFORE installing the import hook.
# These libraries internally use blocked modules (ctypes, threading, etc.)
# which is fine — we only want to block USER code from importing them.
_preloaded_np = None
_preloaded_pd = None
_preloaded_plt = None
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
    _preloaded_mpl.use("Agg")  # non-interactive backend, must be set before pyplot import
    import matplotlib.pyplot as _preloaded_plt
except ImportError:
    pass
try:
    import scipy as _preloaded_scipy
except ImportError:
    pass

# Pre-load dill for checkpointing — must happen before the import hook.
try:
    import dill as _preloaded_dill
except ImportError:
    _preloaded_dill = None

# Pre-load urllib and neuter its network functions so packages like seaborn
# can import it (they use urllib.parse, urllib.request module structure) but
# user code cannot make real network calls.
def _network_blocked(*args, **kwargs):
    raise PermissionError("Network access is disabled in the greed-compute sandbox")

try:
    import urllib as _preloaded_urllib
    import urllib.request as _preloaded_urllib_request
    import urllib.parse   # safe — just URL string manipulation
    import urllib.error
    # Neuter every outbound-network function in urllib.request
    for _fn in ("urlopen", "urlretrieve", "urlcleanup", "install_opener",
                "build_opener", "pathname2url", "url2pathname"):
        if hasattr(_preloaded_urllib_request, _fn):
            setattr(_preloaded_urllib_request, _fn, _network_blocked)
except ImportError:
    pass

_original_import = builtins.__import__

# Neuter dangerous modules that were pulled in as dependencies.
# They're in sys.modules but we replace them with dummy objects so
# user code can't call their functions.
NEUTERED_MODULES = {"subprocess", "socket", "multiprocessing", "http"}

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

# Save the real process stdout before any redirection.
# All protocol messages (stream events, results) go here directly.
_PROTOCOL_STDOUT = sys.stdout

def emit(obj):
    """Write a single JSON line to the protocol stdout and flush immediately."""
    _PROTOCOL_STDOUT.write(json.dumps(obj) + "\n")
    _PROTOCOL_STDOUT.flush()


class _StreamingCapture:
    """
    Stdout/stderr replacement used during streaming execution.
    Each complete line is immediately emitted as a {"type":"stream"} event
    so the client sees output in real-time. The full text is also kept
    for the final {"type":"result"} message.
    """
    def __init__(self):
        self._captured = io.StringIO()
        self._linebuf = ""

    def write(self, data):
        self._captured.write(data)
        self._linebuf += data
        while "\n" in self._linebuf:
            line, self._linebuf = self._linebuf.split("\n", 1)
            emit({"type": "stream", "data": line + "\n"})

    def flush(self):
        # Flush any partial line (no trailing newline yet)
        if self._linebuf:
            emit({"type": "stream", "data": self._linebuf})
            self._linebuf = ""

    def getvalue(self):
        return self._captured.getvalue()


# ── Plot capture ─────────────────────────────────────────────────────────────

def _make_plot_show(plot_store):
    """Return a plt.show() replacement that captures figures as base64 PNGs."""
    def _show(*args, **kwargs):
        if _preloaded_plt is None:
            return
        for fig in _preloaded_plt.get_fignums():
            buf = io.BytesIO()
            _preloaded_plt.figure(fig).savefig(buf, format="png", bbox_inches="tight", dpi=100)
            buf.seek(0)
            plot_store.append(_base64.b64encode(buf.read()).decode("utf-8"))
            buf.close()
        _preloaded_plt.close("all")
    return _show


# ── Session state ────────────────────────────────────────────────────────────

def make_session_globals():
    """Create a fresh session globals dict with pre-loaded libraries."""
    g = {"__builtins__": builtins}
    if _preloaded_np is not None:
        g["np"] = _preloaded_np
    if _preloaded_pd is not None:
        g["pd"] = _preloaded_pd
    if _preloaded_plt is not None:
        g["plt"] = _preloaded_plt
    return g


# ── Command handlers ────────────────────────────────────────────────────────

def handle_ping():
    emit({"type": "pong"})


def handle_clear(session_globals):
    session_globals.clear()
    session_globals.update(make_session_globals())
    emit({"type": "cleared"})


# Only allow safe package name characters — prevents command injection.
_SAFE_PACKAGE_RE = re.compile(r'^[A-Za-z0-9_\-\.\[\]~=<>!]+$')

# GPU-heavy libraries blocked on CPU-only tier.
# These require CUDA and are too large for this instance.
_BLOCKED_PACKAGES = frozenset({
    "torch", "pytorch", "torchvision", "torchaudio", "torchtext",
    "tensorflow", "tensorflow-gpu", "tensorflow-cpu", "tf-nightly",
    "jax", "jaxlib",
    "cupy", "cupy-cuda11x", "cupy-cuda12x",
    "paddle", "paddlepaddle", "paddlepaddle-gpu",
    "mxnet", "mxnet-cu112",
    "onnxruntime-gpu",
    "nvidia-cuda-runtime-cu12", "nvidia-cublas-cu12",
})

def _base_package_name(pkg):
    """Extract bare package name from a specifier like 'numpy>=1.0' or 'torch==2.0'."""
    return re.split(r'[=<>!~\[]', pkg)[0].lower().replace("_", "-")

def handle_install(packages):
    """Install packages into the sandbox venv via pip."""
    if not packages:
        emit({"type": "install_result", "stdout": "", "error": "No packages specified"})
        return

    # Validate every package name before passing to pip
    for pkg in packages:
        if not _SAFE_PACKAGE_RE.match(pkg):
            emit({"type": "install_result", "stdout": "", "error": f"Invalid package name: '{pkg}'"})
            return
        if _base_package_name(pkg) in _BLOCKED_PACKAGES:
            emit({"type": "install_result", "stdout": "",
                  "error": f"'{_base_package_name(pkg)}' requires a GPU instance and is not available on the CPU tier. GPU support is coming soon."})
            return

    start = time.monotonic()
    try:
        proc = _pip_subprocess.run(
            [sys.executable, "-m", "pip", "install", "--quiet", "--no-warn-script-location"] + packages,
            capture_output=True,
            text=True,
            timeout=120,
        )
        output = proc.stdout + proc.stderr
        error = None if proc.returncode == 0 else f"pip exited with code {proc.returncode}"
    except _pip_subprocess.TimeoutExpired:
        output = ""
        error = "pip install timed out after 120s"
    except Exception as e:
        output = ""
        error = str(e)

    duration_ms = int((time.monotonic() - start) * 1000)
    emit({"type": "install_result", "stdout": output, "error": error, "duration_ms": duration_ms})


def handle_checkpoint(path, session_globals):
    """Serialize session state to disk using dill."""
    if _preloaded_dill is None:
        emit({"type": "checkpoint_result", "size_bytes": 0, "error": "dill not installed in venv"})
        return
    # Save everything except __builtins__ — it's always re-injected on restore
    to_save = {k: v for k, v in session_globals.items() if k != "__builtins__"}
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            _preloaded_dill.dump(to_save, f, recurse=True)
        size = os.path.getsize(path)
        emit({"type": "checkpoint_result", "size_bytes": size, "error": None})
    except Exception as e:
        emit({"type": "checkpoint_result", "size_bytes": 0, "error": str(e)})


def handle_restore(path, session_globals):
    """Deserialize session state from disk and merge into current globals."""
    if _preloaded_dill is None:
        emit({"type": "restore_result", "vars": [], "error": "dill not installed in venv"})
        return
    try:
        with open(path, "rb") as f:
            restored = _preloaded_dill.load(f)
        session_globals.update(restored)
        session_globals["__builtins__"] = builtins  # always re-inject
        user_vars = [k for k in restored if not k.startswith("_")]
        emit({"type": "restore_result", "vars": user_vars, "error": None})
    except Exception as e:
        emit({"type": "restore_result", "vars": [], "error": str(e)})


def _split_last_expr(code):
    """
    Parse code and split off the last statement if it's a bare expression.
    Returns (body_code, last_expr_code) where last_expr_code may be None.
    This mirrors Jupyter's behavior: `a=1\na` prints the value of a.
    """
    try:
        tree = ast.parse(code)
    except SyntaxError:
        return code, None

    if not tree.body:
        return code, None

    last = tree.body[-1]
    if not isinstance(last, ast.Expr):
        return code, None

    # Split the source at the last statement's line
    lines = code.splitlines(keepends=True)
    last_line = last.lineno - 1  # ast lines are 1-indexed
    body_code = "".join(lines[:last_line])
    expr_code = "".join(lines[last_line:])
    return body_code, expr_code


def handle_execute(code, session_globals, streaming=False):
    timeout = get_cpu_timeout()
    start = time.monotonic()
    error = None
    html = None
    eval_result_repr = None
    plot_store = []

    body_code, expr_code = _split_last_expr(code)

    # Patch plt.show() to capture figures into plot_store
    if _preloaded_plt is not None:
        session_globals["plt"].show = _make_plot_show(plot_store)

    # Set alarm for timeout
    old_handler = signal.signal(signal.SIGALRM, _alarm_handler)
    signal.alarm(timeout)

    # Streaming mode: emit each print line immediately as it happens.
    # Non-streaming mode: buffer everything, return in one response.
    captured = _StreamingCapture() if streaming else io.StringIO()

    try:
        old_stdout = sys.stdout
        old_stderr = sys.stderr
        sys.stdout = captured
        sys.stderr = captured
        try:
            # NOTE: exec() is intentional — this is a sandbox worker whose
            # entire purpose is to run user-submitted code within a restricted
            # environment (blocked imports, resource limits, timeouts).
            if body_code.strip():
                exec(body_code, session_globals)  # noqa: S102

            if expr_code:
                result = eval(expr_code, session_globals)  # noqa: S307
                if result is not None:
                    # DataFrame / Series → return HTML table instead of repr
                    if _preloaded_pd is not None and isinstance(
                        result, (_preloaded_pd.DataFrame, _preloaded_pd.Series)
                    ):
                        html = result.to_html()
                    else:
                        eval_result_repr = repr(result)
                        print(eval_result_repr)

            # Capture any figures that were created but show() wasn't called
            if _preloaded_plt is not None and _preloaded_plt.get_fignums():
                session_globals["plt"].show()

        finally:
            sys.stdout = old_stdout
            sys.stderr = old_stderr
    except ExecutionTimeout:
        error = f"Execution timed out after {timeout}s"
    except Exception:
        # Full traceback with line numbers
        error = traceback.format_exc().strip()
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, old_handler)

    duration_ms = int((time.monotonic() - start) * 1000)
    emit({
        "type": "result",
        "stdout": captured.getvalue(),
        "result": eval_result_repr,
        "error": error,
        "duration_ms": duration_ms,
        "plots": plot_store,
        "html": html,
    })


# ── Main loop ───────────────────────────────────────────────────────────────

def main():
    # 1. Apply resource limits
    apply_resource_limits()

    # 2. Install import hook
    builtins.__import__ = _restricted_import

    # 3. Pre-load libraries into session globals
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
            emit({"type": "result", "stdout": "", "error": f"Invalid JSON: {e}", "duration_ms": 0, "plots": [], "html": None})
            continue

        msg_type = msg.get("type")

        if msg_type == "ping":
            handle_ping()
        elif msg_type == "clear":
            handle_clear(session_globals)
        elif msg_type == "install":
            handle_install(msg.get("packages", []))
        elif msg_type == "execute":
            code = msg.get("code", "")
            streaming = msg.get("stream", False)
            handle_execute(code, session_globals, streaming=streaming)
        elif msg_type == "checkpoint":
            handle_checkpoint(msg.get("path", ""), session_globals)
        elif msg_type == "restore":
            handle_restore(msg.get("path", ""), session_globals)
        else:
            emit({"type": "result", "stdout": "", "error": f"Unknown command type: {msg_type}", "duration_ms": 0, "plots": [], "html": None})


if __name__ == "__main__":
    main()
