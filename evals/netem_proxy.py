"""A local HTTP CONNECT proxy that emulates a network profile for the
model API traffic of a `hame eval` run: a round-trip time and an upload
(and optionally download) bandwidth limit.

    python3 evals/netem_proxy.py --port 8899 --rtt-ms 80 --up-mbit 20 \
        --stats /tmp/netem-80-20.json
    HTTPS_PROXY=http://127.0.0.1:8899 NO_PROXY=127.0.0.1,localhost \
        hame eval evals/pilot ...

Each direction of every tunnelled connection is delayed by half the RTT
(so a request/response exchange, including TLS handshakes, costs one RTT)
and the client-to-server direction is paced to the upload rate with a
token bucket. Bytes are counted per direction and written to --stats on
exit and every 10 s. It is an approximation: no packet loss, no TCP
congestion control, and the real path's own latency and bandwidth add to
the emulated ones (measure the unemulated profile alongside).

Only CONNECT is supported (HTTPS); plain HTTP requests get 405.
"""

import argparse
import asyncio
import json
import signal
import time

CHUNK = 16 * 1024


class Stats:
    def __init__(self, path, profile):
        self.path = path
        self.profile = profile
        self.up = 0
        self.down = 0
        self.connections = 0
        self.started = time.time()

    def write(self):
        if not self.path:
            return
        data = dict(self.profile, connections=self.connections, bytes_up=self.up,
                    bytes_down=self.down, seconds=round(time.time() - self.started, 1))
        with open(self.path, "w") as f:
            json.dump(data, f, indent=1)


class Pacer:
    """Delays chunks by a fixed one-way latency and paces them to a rate
    (bytes per second; None for unlimited), keeping their order."""

    def __init__(self, one_way_s, rate):
        self.one_way_s = one_way_s
        self.rate = rate
        self.free_at = 0.0

    def release_time(self, arrived, size):
        start = max(arrived, self.free_at)
        if self.rate:
            self.free_at = start + size / self.rate
            start = self.free_at
        return start + self.one_way_s


async def pipe(reader, writer, pacer, count):
    queue = asyncio.Queue()

    async def receive():
        try:
            while True:
                data = await reader.read(CHUNK)
                if not data:
                    break
                await queue.put((pacer.release_time(time.monotonic(), len(data)), data))
        finally:
            await queue.put((None, None))

    async def send():
        while True:
            when, data = await queue.get()
            if data is None:
                break
            delay = when - time.monotonic()
            if delay > 0:
                await asyncio.sleep(delay)
            writer.write(data)
            count(len(data))
            await writer.drain()
        try:
            writer.write_eof()
        except (OSError, RuntimeError):
            pass

    await asyncio.gather(receive(), send(), return_exceptions=True)


async def handle(client_r, client_w, args, stats):
    try:
        head = await client_r.readuntil(b"\r\n\r\n")
    except (asyncio.IncompleteReadError, asyncio.LimitOverrunError):
        client_w.close()
        return
    line = head.split(b"\r\n", 1)[0].decode("latin-1")
    parts = line.split()
    if len(parts) < 2 or parts[0].upper() != "CONNECT":
        client_w.write(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n")
        await client_w.drain()
        client_w.close()
        return
    host, _, port = parts[1].rpartition(":")
    try:
        # Opening the connection costs one emulated round trip.
        await asyncio.sleep(args.rtt_ms / 1000)
        server_r, server_w = await asyncio.open_connection(host.strip("[]"), int(port))
    except (OSError, ValueError) as e:
        client_w.write(f"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nX-Error: {e}\r\n\r\n".encode())
        await client_w.drain()
        client_w.close()
        return
    stats.connections += 1
    client_w.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
    await client_w.drain()
    one_way = args.rtt_ms / 2000
    up = Pacer(one_way, args.up_mbit * 125_000 if args.up_mbit else None)
    down = Pacer(one_way, args.down_mbit * 125_000 if args.down_mbit else None)

    def add_up(n):
        stats.up += n

    def add_down(n):
        stats.down += n

    await asyncio.gather(
        pipe(client_r, server_w, up, add_up),
        pipe(server_r, client_w, down, add_down),
        return_exceptions=True,
    )
    for w in (server_w, client_w):
        try:
            w.close()
        except OSError:
            pass


async def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--port", type=int, default=8899)
    parser.add_argument("--rtt-ms", type=float, default=0)
    parser.add_argument("--up-mbit", type=float, default=0, help="0 for unlimited")
    parser.add_argument("--down-mbit", type=float, default=0, help="0 for unlimited")
    parser.add_argument("--stats", default=None)
    args = parser.parse_args()
    stats = Stats(args.stats, {"rtt_ms": args.rtt_ms, "up_mbit": args.up_mbit,
                               "down_mbit": args.down_mbit})
    server = await asyncio.start_server(lambda r, w: handle(r, w, args, stats),
                                        "127.0.0.1", args.port)
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, stop.set)
    print(f"netem proxy on 127.0.0.1:{args.port} rtt={args.rtt_ms}ms "
          f"up={args.up_mbit or 'unlimited'}Mbit down={args.down_mbit or 'unlimited'}Mbit",
          flush=True)

    async def flush():
        while not stop.is_set():
            stats.write()
            await asyncio.sleep(10)

    flusher = asyncio.create_task(flush())
    async with server:
        await stop.wait()
    flusher.cancel()
    stats.write()


if __name__ == "__main__":
    asyncio.run(main())
