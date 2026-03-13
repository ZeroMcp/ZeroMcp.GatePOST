import json
import sys

TOOLS = [
    {
        "name": "search",
        "description": "Search external content",
        "inputSchema": {
            "type": "object",
            "required": ["q"],
            "properties": {
                "q": {"type": "string"},
                "limit": {"type": "integer", "default": 10},
            },
        },
    },
    {
        "name": "summarize",
        "description": "Summarize a block of text",
        "inputSchema": {
            "type": "object",
            "required": ["text"],
            "properties": {
                "text": {"type": "string"},
            },
        },
    },
]


def send(message):
    sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    sys.stdout.flush()


for raw_line in sys.stdin:
    line = raw_line.strip()
    if not line:
        continue

    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")

    if method == "initialize":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "serverInfo": {"name": "mock-gatepost-mcp", "version": "1.0.0"},
                    "capabilities": {"tools": {"listChanged": False}},
                    "instructions": "Use search for lookup and summarize for short summaries.",
                },
            }
        )
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = message.get("params", {})
        name = params.get("name")
        arguments = params.get("arguments", {})
        if name == "search":
            query = arguments.get("q", "")
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": f"Top result for {query}",
                            }
                        ]
                    },
                }
            )
        elif name == "summarize":
            text = arguments.get("text", "")
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": text[:40],
                            }
                        ]
                    },
                }
            )
        else:
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {
                        "code": -32601,
                        "message": f"Unknown tool: {name}",
                    },
                }
            )
    elif method == "notifications/initialized":
        continue
    else:
        if request_id is not None:
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {
                        "code": -32601,
                        "message": f"Unknown method: {method}",
                    },
                }
            )
