#!/usr/bin/env python3
"""Loopback CoinGecko-compatible fixture server for offline manual runs.

Serves ``/api/v3/coins/markets``, ``/api/v3/global``, a rich
``/api/v3/coins/{id}`` detail body, and an RSS headline feed at ``/rss`` with
small sanitized payloads so the release binary can run fully offline against a
delayed or failing provider. Raw HTTP is permitted only for loopback hosts
precisely so a fixture server like this can back manual measurements without
touching the live API.

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


def build_coin_detail(coin_id, index):
    """A rich ``/coins/{id}`` body mirroring the markets row's coin."""
    price = 100_000.0 / index
    return {
        "id": coin_id,
        "symbol": f"FC{index}",
        "name": f"Fixture Coin {index}",
        "market_cap_rank": index,
        "categories": ["fixtures", "layer-1"],
        "description": {
            "en": f"A sanitized fixture description for coin {index} used by the detail sidebar."
        },
        "market_data": {
            "current_price": {"usd": price},
            "market_cap": {"usd": price * 10_000_000},
            "fully_diluted_valuation": {"usd": price * 11_000_000},
            "total_volume": {"usd": price * 1_000_000},
            "high_24h": {"usd": price * 1.04},
            "low_24h": {"usd": price * 0.98},
            "ath": {"usd": price * 2.0},
            "atl": {"usd": price * 0.1},
            "ath_change_percentage": {"usd": -50.0},
            "atl_change_percentage": {"usd": 900.0},
            "price_change_percentage_1h_in_currency": {"usd": 0.1 * index},
            "price_change_percentage_24h": -0.05 * index,
            "price_change_percentage_7d_in_currency": {"usd": 0.01 * index},
            "price_change_percentage_14d_in_currency": {"usd": 0.02 * index},
            "price_change_percentage_30d_in_currency": {"usd": -0.03 * index},
            "price_change_percentage_60d_in_currency": {"usd": 0.04 * index},
            "price_change_percentage_1y_in_currency": {"usd": 0.05 * index},
            "circulating_supply": 21_000_000.0,
            "total_supply": 21_000_000.0,
            "max_supply": 21_000_000.0,
            "sentiment_votes_up_percentage": 55.0 + index,
            "sentiment_votes_down_percentage": 45.0 - index,
            "sparkline_7d": {"price": [float(index), 2.0 * index, 3.0 * index, index]},
        },
    }


def build_market_chart(index):
    """30 days of hourly prices: a bounded rise-then-fall around `index`."""
    prices = []
    for hour in range(30 * 24):
        price = (index * 1000.0) * (1.0 + 0.1 * (hour % 30) / 30.0) + (hour % 7)
        prices.append([hour * 3600 * 1000, price])
    return {"prices": prices, "market_caps": [], "total_volumes": []}


MARKET_CHART_BODY = json.dumps(build_market_chart(1)).encode()

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
RSS_BODY = (
    """<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Fixture Wire</title>
    <item>
      <title>Bitcoin rises above $100K</title>
      <link>https://example.com/stories/bitcoin-rises</link>
      <pubDate>Tue, 18 Aug 2026 14:41:31 +0000</pubDate>
    </item>
    <item>
      <title>Ethereum settles after a quiet session</title>
      <link>https://example.com/stories/ethereum-settles</link>
      <pubDate>Mon, 17 Aug 2026 09:00:00 +0000</pubDate>
    </item>
    <item>
      <title>Solana leads the 24h gainers</title>
      <link>https://example.com/stories/solana-gainers</link>
      <pubDate>Sun, 16 Aug 2026 21:15:00 +0000</pubDate>
    </item>
  </channel>
</rss>
"""
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
        elif path == "/rss":
            if mode == "timeout" and delay_ms:
                time.sleep(delay_ms / 1000.0)
            self._send(RSS_BODY)
        elif path.startswith("/api/v3/coins/"):
            rest = path[len("/api/v3/coins/") :]
            coin_id, sep, suffix = rest.partition("/")
            if coin_id.startswith("fixture-coin-") and coin_id[13:].isdigit():
                index = int(coin_id[13:])
                if 1 <= index <= 100:
                    if suffix == "market_chart":
                        self._send(json.dumps(build_market_chart(index)).encode())
                    else:
                        self._send(json.dumps(build_coin_detail(coin_id, index)).encode())
                    return
            self.send_error(404)
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
