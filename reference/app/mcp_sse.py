from app.mcp_server import server
from mcp.server.sse import SseServerTransport

sse_transport = SseServerTransport("/mcp/messages/")

async def mcp_app(scope, receive, send):
    if scope["type"] != "http":
        return
    path = scope.get("path", "")
    if path.endswith("/sse"):
        async with sse_transport.connect_sse(scope, receive, send) as streams:
            await server.run(
                streams[0], streams[1],
                server.create_initialization_options()
            )
    elif "/messages/" in path:
        await sse_transport.handle_post_message(scope, receive, send)
    else:
        await send({"type": "http.response.start", "status": 404, "headers": []})
        await send({"type": "http.response.body", "body": b"Not found"})
