#!/usr/bin/env python3
"""Small dependency-free HTTP/1.1 load probe for the Super hello API."""

from __future__ import annotations

import argparse
import socket
import statistics
import threading
import time
from pathlib import Path


def rss_kib(pid: int) -> int:
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    except OSError:
        pass
    return 0


def percentile(values: list[float], fraction: float) -> float:
    return values[min(len(values) - 1, int(len(values) * fraction))]


def run(host: str, port: int, path: str, concurrency: int, duration: float, pid: int) -> dict:
    deadline = time.monotonic() + duration
    latencies: list[float] = []
    errors = 0
    peak_rss = 0
    lock = threading.Lock()
    request = (
        f"GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    ).encode()

    def worker() -> None:
        nonlocal errors
        local: list[float] = []
        failed = 0
        while time.monotonic() < deadline:
            started = time.perf_counter_ns()
            try:
                with socket.create_connection((host, port), timeout=3) as sock:
                    sock.sendall(request)
                    response = bytearray()
                    while chunk := sock.recv(4096):
                        response.extend(chunk)
                if b"200 OK" in response:
                    local.append((time.perf_counter_ns() - started) / 1_000_000)
                else:
                    failed += 1
            except OSError:
                failed += 1
        with lock:
            latencies.extend(local)
            errors += failed

    def monitor() -> None:
        nonlocal peak_rss
        while time.monotonic() < deadline:
            peak_rss = max(peak_rss, rss_kib(pid))
            time.sleep(0.02)

    threads = [threading.Thread(target=worker) for _ in range(concurrency)]
    monitor_thread = threading.Thread(target=monitor)
    started = time.monotonic()
    monitor_thread.start()
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    monitor_thread.join()
    elapsed = time.monotonic() - started
    latencies.sort()
    return {
        "requests": len(latencies),
        "errors": errors,
        "rps": len(latencies) / elapsed,
        "p50_ms": percentile(latencies, 0.50),
        "p99_ms": percentile(latencies, 0.99),
        "peak_rss_kib": peak_rss,
        "concurrency": concurrency,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=3021)
    parser.add_argument("--path", default="/ping")
    parser.add_argument("--concurrency", type=int, default=32)
    parser.add_argument("--duration", type=float, default=5)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--pid", type=int, required=True)
    args = parser.parse_args()
    results = []
    for index in range(args.runs):
        result = run(
            args.host,
            args.port,
            args.path,
            args.concurrency,
            args.duration,
            args.pid,
        )
        results.append(result)
        fields = " ".join(
            f"{key}={value:.3f}" if isinstance(value, float) else f"{key}={value}"
            for key, value in result.items()
        )
        print(f"run={index + 1} {fields}")
    print(
        "median "
        f"rps={statistics.median(r['rps'] for r in results):.2f} "
        f"p50_ms={statistics.median(r['p50_ms'] for r in results):.3f} "
        f"p99_ms={statistics.median(r['p99_ms'] for r in results):.3f} "
        f"peak_rss_kib={max(r['peak_rss_kib'] for r in results)}"
    )


if __name__ == "__main__":
    main()
