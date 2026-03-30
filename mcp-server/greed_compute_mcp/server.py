"""
greed-compute MCP server

Exposes greed-compute as tools for Claude Desktop, Cursor, and any MCP client.

Configuration (env vars):
  GREED_API_URL  — base URL  (default: http://localhost:8080)
  GREED_API_KEY  — your API key (required)
"""

import os
import json
import asyncio
import httpx
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp import types

# ── Config ───────────────────────────────────────────────────────────────────

API_URL = os.environ.get("GREED_API_URL", "http://localhost:8080").rstrip("/")
API_KEY = os.environ.get("GREED_API_KEY", "")

# ── HTTP helper ───────────────────────────────────────────────────────────────

def _headers() -> dict:
    return {"x-api-key": API_KEY, "Content-Type": "application/json"}


async def _request(method: str, path: str, body: dict | None = None, timeout: float = 120.0) -> dict:
    url = f"{API_URL}/v1{path}"
    async with httpx.AsyncClient(timeout=timeout) as client:
        resp = await client.request(method, url, headers=_headers(), json=body)
        resp.raise_for_status()
        return resp.json()


def _ok(data: dict) -> list[types.TextContent]:
    return [types.TextContent(type="text", text=json.dumps(data, indent=2))]


def _err(msg: str) -> list[types.TextContent]:
    return [types.TextContent(type="text", text=f"Error: {msg}")]


# ── Server ────────────────────────────────────────────────────────────────────

server = Server("greed-compute")


@server.list_tools()
async def list_tools() -> list[types.Tool]:
    return [
        types.Tool(
            name="create_session",
            description=(
                "Create a new Python execution session. Returns a session_id that "
                "you must pass to execute_code and other tools. Sessions last 15 "
                "minutes and auto-renew on each execution. Optionally restore a "
                "saved checkpoint on creation."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "checkpoint_id": {
                        "type": "string",
                        "description": "Optional: restore a saved checkpoint into the new session immediately.",
                    },
                    "packages": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional: pip install these packages before the session is ready.",
                    },
                },
            },
        ),
        types.Tool(
            name="execute_code",
            description=(
                "Execute Python code in a session. Supports numpy, pandas, "
                "matplotlib, sklearn, scipy out of the box. Returns stdout, the "
                "value of the last expression (Jupyter-style), base64 plot images, "
                "HTML DataFrames, and full tracebacks on error."
            ),
            inputSchema={
                "type": "object",
                "required": ["session_id", "code"],
                "properties": {
                    "session_id": {"type": "string", "description": "Session ID from create_session."},
                    "code": {"type": "string", "description": "Python code to execute."},
                },
            },
        ),
        types.Tool(
            name="install_packages",
            description=(
                "pip install packages into a session. GPU-heavy libraries (torch, "
                "tensorflow, jax) are blocked — use a GPU-tier session for those."
            ),
            inputSchema={
                "type": "object",
                "required": ["session_id", "packages"],
                "properties": {
                    "session_id": {"type": "string"},
                    "packages": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Package names, e.g. [\"seaborn\", \"xgboost==2.0.3\"]",
                    },
                },
            },
        ),
        types.Tool(
            name="submit_job",
            description=(
                "Submit long-running code as a background job. Returns a job_id "
                "immediately. Use get_job to poll for results. Optionally provide "
                "a webhook_url to receive results via HTTP POST when done."
            ),
            inputSchema={
                "type": "object",
                "required": ["session_id", "code"],
                "properties": {
                    "session_id": {"type": "string"},
                    "code": {"type": "string"},
                    "webhook_url": {
                        "type": "string",
                        "description": "Optional URL to POST results to when the job completes.",
                    },
                },
            },
        ),
        types.Tool(
            name="get_job",
            description="Poll the status and result of a background job submitted with submit_job.",
            inputSchema={
                "type": "object",
                "required": ["job_id"],
                "properties": {
                    "job_id": {"type": "string"},
                },
            },
        ),
        types.Tool(
            name="session_status",
            description="Get session info: TTL remaining (seconds), calls used, active status.",
            inputSchema={
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": {"type": "string"},
                },
            },
        ),
        types.Tool(
            name="terminate_session",
            description="Terminate a session and free its resources immediately.",
            inputSchema={
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": {"type": "string"},
                },
            },
        ),
        types.Tool(
            name="create_checkpoint",
            description=(
                "Save the current session state (all variables, functions, imports) "
                "to a named checkpoint. The checkpoint persists across sessions and "
                "can be restored later."
            ),
            inputSchema={
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": {"type": "string"},
                    "name": {
                        "type": "string",
                        "description": "Human-readable name for the checkpoint.",
                    },
                },
            },
        ),
        types.Tool(
            name="restore_checkpoint",
            description=(
                "Restore a saved checkpoint into a running session. All variables "
                "and functions from the checkpoint are merged into the session."
            ),
            inputSchema={
                "type": "object",
                "required": ["session_id", "checkpoint_id"],
                "properties": {
                    "session_id": {"type": "string"},
                    "checkpoint_id": {"type": "string"},
                },
            },
        ),
        types.Tool(
            name="list_checkpoints",
            description="List all saved checkpoints for your API key.",
            inputSchema={"type": "object", "properties": {}},
        ),
        types.Tool(
            name="delete_checkpoint",
            description="Delete a checkpoint and its stored file.",
            inputSchema={
                "type": "object",
                "required": ["checkpoint_id"],
                "properties": {
                    "checkpoint_id": {"type": "string"},
                },
            },
        ),
    ]


@server.call_tool()
async def call_tool(name: str, arguments: dict) -> list[types.TextContent]:
    if not API_KEY:
        return _err("GREED_API_KEY environment variable is not set.")

    try:
        if name == "create_session":
            data = await _request("POST", "/session/create", arguments or {})
            return _ok(data)

        elif name == "execute_code":
            session_id = arguments["session_id"]
            code = arguments["code"]
            data = await _request("POST", f"/session/{session_id}/execute", {"code": code})
            # Format output for readability in chat
            parts = []
            if data.get("stdout"):
                parts.append(f"stdout:\n{data['stdout'].rstrip()}")
            if data.get("result") is not None:
                parts.append(f"result: {data['result']}")
            if data.get("html"):
                parts.append("[DataFrame output — HTML table returned, render on frontend]")
            if data.get("plots"):
                parts.append(f"[{len(data['plots'])} plot(s) captured as base64 PNG]")
            if data.get("error"):
                parts.append(f"error:\n{data['error']}")
            if not parts:
                parts.append("(no output)")
            parts.append(f"\nduration: {data.get('duration_ms', 0)}ms")
            return [types.TextContent(type="text", text="\n\n".join(parts))]

        elif name == "install_packages":
            session_id = arguments["session_id"]
            packages = arguments["packages"]
            data = await _request(
                "POST", f"/session/{session_id}/install",
                {"packages": packages}, timeout=180.0
            )
            return _ok(data)

        elif name == "submit_job":
            session_id = arguments["session_id"]
            body = {"code": arguments["code"]}
            if "webhook_url" in arguments:
                body["webhook_url"] = arguments["webhook_url"]
            data = await _request("POST", f"/session/{session_id}/execute/async", body)
            return _ok(data)

        elif name == "get_job":
            job_id = arguments["job_id"]
            data = await _request("GET", f"/jobs/{job_id}")
            return _ok(data)

        elif name == "session_status":
            session_id = arguments["session_id"]
            data = await _request("GET", f"/session/{session_id}/status")
            return _ok(data)

        elif name == "terminate_session":
            session_id = arguments["session_id"]
            data = await _request("DELETE", f"/session/{session_id}")
            return _ok(data)

        elif name == "create_checkpoint":
            session_id = arguments["session_id"]
            body = {}
            if "name" in arguments:
                body["name"] = arguments["name"]
            data = await _request("POST", f"/session/{session_id}/checkpoint", body)
            return _ok(data)

        elif name == "restore_checkpoint":
            session_id = arguments["session_id"]
            checkpoint_id = arguments["checkpoint_id"]
            data = await _request("POST", f"/session/{session_id}/restore/{checkpoint_id}")
            return _ok(data)

        elif name == "list_checkpoints":
            data = await _request("GET", "/checkpoints")
            return _ok(data)

        elif name == "delete_checkpoint":
            checkpoint_id = arguments["checkpoint_id"]
            data = await _request("DELETE", f"/checkpoints/{checkpoint_id}")
            return _ok(data)

        else:
            return _err(f"Unknown tool: {name}")

    except httpx.HTTPStatusError as e:
        return _err(f"API error {e.response.status_code}: {e.response.text}")
    except httpx.ConnectError:
        return _err(f"Could not connect to greed-compute at {API_URL}")
    except Exception as e:
        return _err(str(e))


# ── Entry point ───────────────────────────────────────────────────────────────

def main():
    asyncio.run(_run())


async def _run():
    async with stdio_server() as (read_stream, write_stream):
        await server.run(read_stream, write_stream, server.create_initialization_options())


if __name__ == "__main__":
    main()
