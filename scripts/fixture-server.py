#!/usr/bin/env python3
"""Loopback CoinGecko-compatible fixture server for offline manual runs.

Serves ``/api/v3/coins/markets`` and ``/api/v3/global`` with small sanitized
JSON so the release binary can run fully offline against a delayed or failing
provider. Raw HTTP is permitted only for loopback hosts precisely so a fixture
server like this can back manual measurements without touching the live API.

Usage:
    scripts/fixture-server.py --port 8137 [--delay-ms 250] [--mode server-error]
"""

import argparse
import json
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

UPDATED_AT = "2026-08-17T00:00:00Z"


def build_rows():
    rows = []
    for index in range(1, 101):
        price = 100_000.0 / index
        rows.append(
            {
                "id": f"fixture-coin-{index}",
                "symbol": f"FC{index}",
                "name": f"Fixture Coin {index}",
                "market_cap_rank": index,
                "current_price": price,
                "price_change_percentage_1h_in_currency": 0.1 * index,
                "price_change_percentage_24h": -0.05 * index,
                "price_change_percentage_7d_in_currency": 0.01 * index,
                "market_cap": price * 10_000_000,
                "total_volume": price * 1_000_000,
                "circulating_supply": 21_000_000.0,
                "sparkline_in_7d": {"price": [float(index), 2.0 * index, 3.0 * index, index]},
                "last_updated": UPDATED_AT,
            }
        )
    return rows


ROWS_BODY = json.dumps(build_rows()).encode()
GLOBAL_BODY = json.dumps(
    {
        "data": {
            "total_market_cap": {"usd": 2_000_000_000_000.0},
            "total_volume": {"usd": 80_000_000_000.0},
            "market_cap_percentage": {"btc": 54.0},
            "market_cap_change_percentage_24h_usd": -0.4,
            "updated_at": 1784320000,
        }
    }
).encode()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        mode = getattr(self.server, "mode", "ok")
        delay_ms = getattr(self.server, "delay_ms", 0)
        if path == "/api/v3/coins/markets":
            if mode in ("malformed", "rate-limited", "server-error", "timeout"):
                self._mode(mode)
                return
            if delay_ms:
                time.sleep(delay_ms / 1000.0)
            self._send(ROWS_BODY)
        elif path == "/api/v3/global":
            if mode == "timeout" and delay_ms:
                time.sleep(delay_ms / 1000.0)
            self._send(GLOBAL_BODY)
        else:
            self.send_error(404)

    def _mode(self, mode):
        if mode == "malformed":
            self._send(b"this is not json")
        elif mode == "rate-limited":
            self.send_response(429)
            self.send_header("Retry-After", "5")
            self.send_header("Content-Length", "0")
            self.end_headers()
        elif mode == "server-error":
            self.send_response(500)
            self.send_header("Content-Length", "0")
            self.end_headers()
        elif mode == "timeout":
            time.sleep(50)

    def _send(self, body):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8137)
    parser.add_argument("--delay-ms", type=int, default=0)
    parser.add_argument(
        "--mode",
        default="ok",
        choices=["ok", "malformed", "rate-limited", "server-error", "timeout"],
    )
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    server.mode = args.mode
    server.delay_ms = args.delay_ms
    print(
        f"fixture server listening on http://127.0.0.1:{args.port}/ "
        f"(mode={args.mode} delay={args.delay_ms}ms)",
        flush=True,
    )
    server.serve_forever()


if __name__ == "__main__":
    main()
