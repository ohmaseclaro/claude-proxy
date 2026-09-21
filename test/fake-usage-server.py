#!/usr/bin/env python3
"""Stand-in for api.anthropic.com/api/oauth/usage.

The scenario is chosen by the bearer token, so the tests also prove that each
account is probed with its own credential. Prints the bound port on stdout.
"""
import json, sys, time, datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

def at(**kw):
    d = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(**kw)
    return d.strftime("%Y-%m-%dT%H:%M:%SZ")


FULL = {
    "five_hour": {"utilization": 4, "resets_at": at(hours=4, minutes=58)},
    "seven_day": {"utilization": 10, "resets_at": at(days=6, hours=23)},
    "seven_day_sonnet": {"utilization": 12, "resets_at": at(days=5, hours=1)},
    "extra_usage": {"is_enabled": True, "monthly_limit": 2000, "used_credits": 142,
                    "currency": "USD", "decimal_places": 2},
    "limits": [
        {"kind": "five_hour", "percent": 4, "resets_at": at(hours=4, minutes=58)},
        {"kind": "weekly_scoped", "percent": 31, "resets_at": at(days=6, hours=23),
         "scope": {"model": {"display_name": "Fable 5"}}},
    ],
}
MINIMAL = {"five_hour": {"utilization": 77, "resets_at": at(minutes=35)}}
FAR = {"five_hour": {"utilization": 1, "resets_at": "2099-01-01T10:00:00Z"}}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def reply(self, code, body, ctype="application/json"):
        raw = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        auth = self.headers.get("Authorization", "")
        token = auth[7:] if auth.startswith("Bearer ") else ""
        # The real endpoint rate-limits without a claude-code User-Agent.
        if not (self.headers.get("User-Agent") or "").startswith("claude-code/"):
            return self.reply(429, json.dumps({"error": "missing claude-code user agent"}))
        if self.headers.get("anthropic-beta") != "oauth-2025-04-20":
            return self.reply(400, json.dumps({"error": "missing beta header"}))
        if token == "tok-full":
            return self.reply(200, json.dumps(FULL))
        if token == "tok-minimal":
            return self.reply(200, json.dumps(MINIMAL))
        if token == "tok-far":
            return self.reply(200, json.dumps(FAR))
        if token == "tok-garbage":
            return self.reply(200, "{not json,,,")
        if token == "tok-empty":
            return self.reply(200, json.dumps({"account": {"email": "x@y.z"}}))
        if token == "tok-limited":
            return self.reply(429, json.dumps({"error": "rate limited"}))
        if token == "tok-slow":
            time.sleep(30)
            return self.reply(200, json.dumps(MINIMAL))
        return self.reply(401, json.dumps({"error": {"message": "invalid bearer token"}}))


srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(srv.server_address[1], flush=True)
srv.serve_forever()
