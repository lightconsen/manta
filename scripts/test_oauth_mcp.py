#!/usr/bin/env python3
"""
Mock OAuth MCP Server for testing syscity's Remote MCP + OAuth flow.

Usage:
    python scripts/test_oauth_mcp.py

This starts a test MCP server with OAuth on localhost:9999.
Add a preset to mcps.toml:
    [test_oauth]
    display_name = "Test OAuth"
    description = "Local OAuth test server"
    logo_url = "https://cdn.simpleicons.org/testinglibrary"
    url = "http://localhost:9999/mcp"
    transport = "streamable_http"
    auth_type = "oauth2"
    client_id = "test-client"
"""

import json
import hashlib
import base64
import secrets
import urllib.parse
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from urllib.request import Request, urlopen

# ── In-memory token store ──
tokens = {}  # code -> access_token
auth_codes = {}  # code -> state


class OAuthMcpHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)

        # ── OAuth discovery ──
        if parsed.path == "/.well-known/oauth-authorization-server":
            origin = f"http://{self.headers.get('Host', 'localhost:9999')}"
            self._json(200, {
                "issuer": origin,
                "authorization_endpoint": f"{origin}/auth",
                "token_endpoint": f"{origin}/token",
            })
            return

        # ── MCP endpoint (Streamable HTTP) ──
        if parsed.path == "/mcp":
            auth = self.headers.get("Authorization", "")
            if not auth.startswith("Bearer "):
                self._json(401, {"error": "unauthorized"})
                return

            token = auth[len("Bearer "):]
            if token not in tokens.values():
                self._json(401, {"error": "invalid_token"})
                return

            # Return a simple MCP server info
            self._json(200, {
                "jsonrpc": "2.0",
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {
                            "listChanged": False
                        }
                    },
                    "serverInfo": {
                        "name": "test-oauth-mcp",
                        "version": "1.0.0"
                    }
                },
                "id": 1
            })
            return

        # ── Authorization page ──
        if parsed.path == "/auth":
            qs = urllib.parse.parse_qs(parsed.query)
            client_id = qs.get("client_id", [""])[0]
            redirect_uri = qs.get("redirect_uri", [""])[0]
            state = qs.get("state", [""])[0]
            code_challenge = qs.get("code_challenge", [""])[0]
            code_challenge_method = qs.get("code_challenge_method", [""])[0]

            print(f"\n  [OAuth] Authorization request received:")
            print(f"    client_id          = {client_id}")
            print(f"    redirect_uri       = {redirect_uri}")
            print(f"    state              = {state}")
            print(f"    code_challenge     = {code_challenge[:20]}...")
            print(f"    code_challenge_method = {code_challenge_method}")

            # Generate auth code
            code = secrets.token_hex(16)
            auth_codes[code] = state

            # Simulate user clicking "Authorize" by redirecting back
            self.send_response(302)
            redirect_url = f"{redirect_uri}?code={code}&state={state}"
            self.send_header("Location", redirect_url)
            self.end_headers()
            print(f"  [OAuth] Redirecting back to: {redirect_url}")
            return

        self._json(404, {"error": "not_found"})

    def do_POST(self):
        parsed = urllib.parse.urlparse(self.path)

        # ── MCP endpoint (Streamable HTTP) ──
        if parsed.path == "/mcp":
            auth = self.headers.get("Authorization", "")
            if not auth.startswith("Bearer "):
                self._json(401, {"error": "unauthorized"})
                return
            token = auth[len("Bearer "):]
            if token not in tokens.values():
                self._json(401, {"error": "invalid_token"})
                return

            content_length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_length).decode()
            try:
                req = json.loads(body)
            except json.JSONDecodeError:
                self._json(400, {"error": "bad_request"})
                return

            method = req.get("method", "")
            req_id = req.get("id")

            if method == "initialize":
                result = {
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {"listChanged": False}},
                        "serverInfo": {"name": "test-oauth-mcp", "version": "1.0.0"},
                    },
                }
            elif method == "tools/list":
                result = {
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": {
                        "tools": [{
                            "name": "echo_oauth",
                            "description": "Echo the input back",
                            "parameters": {
                                "type": "object",
                                "properties": {"message": {"type": "string"}},
                            },
                        }]
                    },
                }
            else:
                result = {"jsonrpc": "2.0", "id": req_id, "result": {}}

            # SSE-framed response as required by streamable_http clients
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Access-Control-Allow-Origin", "*")
            self.end_headers()
            self.wfile.write(f"data: {json.dumps(result)}\n\n".encode())
            self.wfile.flush()
            return

        # ── Token exchange ──
        if parsed.path == "/token":
            content_length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_length).decode()
            params = urllib.parse.parse_qs(body)

            grant_type = params.get("grant_type", [""])[0]
            code = params.get("code", [""])[0]
            redirect_uri = params.get("redirect_uri", [""])[0]
            client_id = params.get("client_id", [""])[0]
            code_verifier = params.get("code_verifier", [""])[0]

            print(f"\n  [OAuth] Token exchange request:")
            print(f"    grant_type     = {grant_type}")
            print(f"    code           = {code}")
            print(f"    redirect_uri   = {redirect_uri}")
            print(f"    client_id      = {client_id}")
            print(f"    code_verifier  = {code_verifier[:20]}...")

            # Verify the code verifier against what we'd expect
            expected_challenge = base64.urlsafe_b64encode(
                hashlib.sha256(code_verifier.encode()).digest()
            ).rstrip(b"=").decode()

            # Check state match
            expected_state = auth_codes.get(code)
            if expected_state is None:
                self._json(400, {"error": "invalid_grant"})
                return

            access_token = secrets.token_hex(32)
            refresh_token = secrets.token_hex(32)
            tokens[code] = access_token

            print(f"  [OAuth] Token issued: {access_token[:20]}...")
            self._json(200, {
                "access_token": access_token,
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": refresh_token,
            })
            return

        self._json(404, {"error": "not_found"})

    def _json(self, status, data):
        body = json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        print(f"  [HTTP] {fmt % args}")


def main():
    # Threading server so a slow/stuck request (e.g. a client retrying a
    # revoked token) never blocks the whole mock.
    server = ThreadingHTTPServer(("127.0.0.1", 9999), OAuthMcpHandler)
    print("=" * 60)
    print("  Mock OAuth MCP Server running at http://localhost:9999")
    print()
    print("  Add to mcps.toml:")
    print("    [test_oauth]")
    print('    display_name = "Test OAuth"')
    print('    description = "Local OAuth test server"')
    print('    logo_url = "https://cdn.simpleicons.org/testinglibrary"')
    print('    url = "http://localhost:9999/mcp"')
    print('    transport = "streamable_http"')
    print('    auth_type = "oauth2"')
    print('    client_id = "test-client"')
    print()
    print("  Then click Enable on 'Test OAuth' in Settings.")
    print("=" * 60)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nShutting down...")
        server.shutdown()


if __name__ == "__main__":
    main()
