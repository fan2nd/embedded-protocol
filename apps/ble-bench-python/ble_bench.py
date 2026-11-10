#!/usr/bin/env python3
"""Mac BLE central for the separate ble-bench-protocol diagnostic firmware.

Install apps/ble-bench-python/requirements.txt into a venv. Does not send AT commands.
Reports CRC- and pattern-validated end-to-end payload goodput, never write speed.
"""
import argparse
import asyncio
import copy
from contextlib import asynccontextmanager
import datetime
import json
import secrets
import struct
import time
from pathlib import Path

SOF, EOF = b"\xa5\x5a", b"\x5a\xa5"
HELLO, REPORT, TX_START, POLL, CONFIRM = 0x11, 0x12, 0x13, 0x14, 0x15
DATA, TX_DATA = 0x21, 0x22
READY, STATS, TX_DONE, ACK, CHALLENGE = 0x31, 0x32, 0x33, 0x34, 0x35
KINDS = {HELLO, REPORT, TX_START, POLL, CONFIRM, DATA, TX_DATA, READY, STATS, TX_DONE, ACK, CHALLENGE}
OVERHEAD = 31
MAX_ATTEMPTS = 8


def retry_timeout(attempt):
    return (0.1,0.2,0.4)[min(attempt,2)]
MAX_PAYLOAD = 1024


def _crc_table():
    table = []
    for byte in range(256):
        value = byte
        for _ in range(8):
            value = (value >> 1) ^ (0x82F63B78 if value & 1 else 0)
        table.append(value)
    return table


CRC_TABLE = _crc_table()


def crc32c(data):
    value = 0xFFFFFFFF
    for byte in data:
        value = (value >> 8) ^ CRC_TABLE[(value ^ byte) & 255]
    return value ^ 0xFFFFFFFF


def frame(kind, run, seq=0, payload=b""):
    if len(payload) > MAX_PAYLOAD or kind not in KINDS:
        raise ValueError("invalid diagnostic frame")
    body = SOF + struct.pack("<B16sIH", kind, run.to_bytes(16,"little"), seq, len(payload)) + payload
    return body + struct.pack("<I", crc32c(body)) + EOF


def pattern(seq, size):
    return bytes((seq * 31 + i * 17) & 255 for i in range(size))


class Decoder:
    def __init__(self):
        self.buffer = bytearray()
        self.errors = 0

    def feed(self, data):
        self.buffer.extend(data)
        result = []
        while len(self.buffer) >= 2:
            if self.buffer[:2] != SOF:
                del self.buffer[0]
                continue
            if len(self.buffer) < 25:
                break
            kind, session_bytes, seq, length = struct.unpack_from("<B16sIH", self.buffer, 2)
            run = int.from_bytes(session_bytes,"little")
            if length > MAX_PAYLOAD or kind not in KINDS:
                self.errors += 1
                del self.buffer[0]
                continue
            total = OVERHEAD + length
            if len(self.buffer) < total:
                break
            part = bytes(self.buffer[:total])
            if part[-2:] != EOF or crc32c(part[:-6]) != struct.unpack("<I", part[-6:-2])[0]:
                self.errors += 1
                del self.buffer[0]
                continue
            result.append((kind, run, seq, part[25:-6]))
            del self.buffer[:total]
        return result


class Window:
    def __init__(self, frame_bytes, byte_limit, frame_limit):
        self.width = min(byte_limit // frame_bytes,frame_limit,8)
        if self.width < 1: raise ValueError("Window cannot hold one packet")
        self.base = self.end = 0

    def begin(self,total):
        if self.base == self.end: self.end = min(self.base+self.width,total)
        return range(self.base,self.end)

    def acknowledge(self,next_seq):
        if not self.base <= next_seq <= self.end: raise ValueError("ACK outside sent range")
        self.base = next_seq

    def next_pending(self, cursor):
        next_seq = max(cursor,self.base)
        return next_seq if next_seq < self.end else None

    def accept_ack(self, message, run, turn, sent_end):
        kind, rx_run, rx_turn, payload = message[:4]
        if kind != ACK or rx_run != run or rx_turn != turn or len(payload) != 4:
            return False
        next_seq, = struct.unpack("<I",payload)
        if not self.base < next_seq <= min(self.end,sent_end):
            return False
        self.acknowledge(next_seq)
        return True


def self_test():
    assert crc32c(b"123456789") == 0xE3069283
    raw = frame(DATA, 7, 3, b"AT+++\r\n\xa5\x5a\x5a\xa5")
    decoder = Decoder()
    assert decoder.feed(raw[:5]) == []
    assert decoder.feed(raw[5:]) == [(DATA, 7, 3, b"AT+++\r\n\xa5\x5a\x5a\xa5")]
    damaged = bytearray(raw)
    damaged[25] ^= 1
    assert decoder.feed(damaged + raw) == [(DATA, 7, 3, b"AT+++\r\n\xa5\x5a\x5a\xa5")]
    tx = Window(128,512,4)
    assert list(tx.begin(10)) == [0,1,2,3]
    tx.acknowledge(1)
    assert list(tx.begin(10)) == [1,2,3]
    assert tx.width == 4
    tx.acknowledge(4)
    assert list(tx.begin(10)) == [4,5,6,7]
    assert [retry_timeout(i) for i in (0,1,2,7)] == [.1,.2,.4,.4]
    print("Python CRC/framing/fixed-window/retry checks passed")


async def discover(args):
    from bleak import BleakScanner
    # A just-disconnected peripheral can take time to advertise again. Retry
    # discovery only; this must never become a delay between DATA packets.
    for attempt in range(3):
        found = await BleakScanner.discover(timeout=args.scan_seconds, return_adv=True)
        matches = []
        for device, adv in found.values():
            name = adv.local_name or device.name or ""
            if args.scan or (args.address and device.address == args.address) or (not args.address and args.name.lower() in name.lower()):
                print(json.dumps({"name": name, "address": device.address, "rssi": adv.rssi, "services": adv.service_uuids}), flush=True)
            if (args.address and device.address == args.address) or (not args.address and args.name.lower() in name.lower()):
                matches.append(device)
        if matches or args.scan:
            break
        print(f"Discovery retry {attempt+1}/3", flush=True)
    if args.scan:
        return None
    if len(matches) != 1:
        raise RuntimeError(f"Expected one matching BLE peripheral, found {len(matches)}; use --address from --scan")
    return matches[0]


class HostTiming:
    def __init__(self):
        self.reset()
        self.active = False

    def reset(self):
        self.active = True
        self.samples = {}
        self.queue_peak = 0
        self.previous_native = None

    def observe(self, name, seconds):
        if not self.active:
            return
        value = max(0.0, seconds * 1000)
        row = self.samples.setdefault(name, {"count": 0, "total_ms": 0.0, "max_ms": 0.0, "buckets": {}})
        row["count"] += 1
        row["total_ms"] += value
        row["max_ms"] = max(row["max_ms"], value)
        bound = next((b for b in (.01,.05,.1,.5,1,5,10,50,100,500,1000,5000) if value <= b), float("inf"))
        row["buckets"][bound] = row["buckets"].get(bound,0)+1

    def snapshot(self):
        result = {"decoded_frame_queue_peak": self.queue_peak}
        for name,row in list(self.samples.items()):
            cumulative = 0
            percentile = None
            for bound,n in sorted(row["buckets"].items()):
                cumulative += n
                if cumulative >= row["count"]*.99:
                    percentile = bound if bound != float("inf") else None
                    break
            result[name] = {"count":row["count"], "mean_ms":row["total_ms"]/row["count"],
                "max_ms":row["max_ms"], "p99_upper_bound_ms":percentile}
        return result


class DelegateLoopTiming:
    """Measure the native CoreBluetooth callback -> asyncio handoff, without SDK edits."""
    def __init__(self, loop, timing):
        self.loop = loop
        self.timing = timing

    def __getattr__(self, name):
        return getattr(self.loop,name)

    def call_soon_threadsafe(self, callback, *args, **kwargs):
        if getattr(callback,"__name__","") != "did_update_value_for_characteristic":
            return self.loop.call_soon_threadsafe(callback,*args,**kwargs)
        queued = time.monotonic()
        timing = self.timing
        if timing.active:
            if timing.previous_native is not None:
                timing.observe("native_notification_interval",queued-timing.previous_native)
            timing.previous_native = queued
        def dispatch():
            timing.observe("native_to_asyncio",time.monotonic()-queued)
            callback(*args)
        return self.loop.call_soon_threadsafe(dispatch,**kwargs)


class NativeWriteBlocked(RuntimeError):
    def __init__(self, timing):
        super().__init__("Native BLE write queue did not become ready")
        self.host_timing = timing.snapshot()


async def write_frame_chunks(raw, chunk, ready, write, connected, timeout, timing, should_send=None):
    """Return submitted bytes; a late ACK can cancel the remaining fragments."""
    for offset in range(0,len(raw),chunk):
        queue_wait_start = time.monotonic()
        deadline = queue_wait_start + timeout
        # Run already queued notification callbacks before deciding to submit.
        await asyncio.sleep(0)
        while True:
            if should_send is not None and not should_send():
                return offset
            if ready():
                break
            if not connected() or time.monotonic() >= deadline:
                timing.observe("native_write_queue_wait",time.monotonic()-queue_wait_start)
                raise NativeWriteBlocked(timing)
            await asyncio.sleep(0)
        timing.observe("native_write_queue_wait",time.monotonic()-queue_wait_start)
        await write(raw[offset:offset+chunk])
    return len(raw)


async def benchmark(args, device=None, existing_client=None):
    from bleak import BleakClient
    device = device or await discover(args)
    if args.scan:
        return
    result = {"time": datetime.datetime.now(datetime.timezone.utc).isoformat(), "device": device.address,
              "name": device.name, "parameters": vars(args).copy(), "tests": [], "method": "v3 challenge sessions, fixed window, ACK priority at each submission, data retransmit after 100/200/400ms, whole UART packet", "wire_frame_bytes": args.payload+OVERHEAD}
    @asynccontextmanager
    async def connection():
        if existing_client is not None:
            yield existing_client
        else:
            async with BleakClient(device, timeout=20) as connected:
                yield connected

    async with connection() as client:
        services = []
        for svc in client.services:
            services.append({"uuid": svc.uuid, "characteristics": [{"uuid": c.uuid, "properties": c.properties} for c in svc.characteristics]})
        print(json.dumps({"services": services}, indent=2), flush=True)
        result["services"] = services
        base = "0000{}-0000-1000-8000-00805f9b34fb"
        duplex = [c for svc in client.services for c in svc.characteristics
                  if "write-without-response" in c.properties and "notify" in c.properties]
        def characteristic(uuid):
            if uuid:
                return client.services.get_characteristic(base.format(uuid) if len(uuid) == 4 else uuid)
            if len(duplex) != 1:
                raise RuntimeError("Ambiguous GATT service; set --write-uuid and --notify-uuid explicitly")
            return duplex[0]
        write = characteristic(args.write_uuid)
        notify = characteristic(args.notify_uuid)
        if write is None or notify is None or "write-without-response" not in write.properties or "notify" not in notify.properties:
            raise RuntimeError("Configured write/notify characteristics missing required properties")
        chunk = min(args.chunk, write.max_write_without_response_size)
        result["gatt_write_limit"] = write.max_write_without_response_size
        result["effective_write_chunk"] = chunk
        result["write_uuid"] = write.uuid
        result["notify_uuid"] = notify.uuid
        result["notify_properties"] = list(notify.properties)
        result["negotiated_mtu"] = client.mtu_size
        notification_sizes = {}
        raw_capture = {"active": False, "bytes": 0, "first": None, "last": None, "bins": {}}
        decoder = Decoder()
        queue = asyncio.Queue(maxsize=16384)
        last_notification = None
        ack_handler = None
        timing = HostTiming()
        delegate = getattr(client._backend,"_delegate",None)
        original_loop = delegate._event_loop if delegate is not None else None
        if delegate is not None:
            while isinstance(original_loop,DelegateLoopTiming):
                original_loop = original_loop.loop
            delegate._event_loop = DelegateLoopTiming(original_loop,timing)

        def notification(_, data):
            nonlocal last_notification
            now = time.monotonic()
            # An idle timeout abandons only a partial frame, never delivered data.
            if last_notification is not None and now-last_notification >= 1 and decoder.buffer:
                decoder.buffer.clear()
                decoder.errors += 1
            last_notification = now
            if raw_capture["active"]:
                raw_capture["first"] = raw_capture["first"] or now
                raw_capture["last"] = now
                raw_capture["bytes"] += len(data)
                second = int(now - raw_capture["first"])
                raw_capture["bins"][second] = raw_capture["bins"].get(second, 0) + len(data)
            notification_sizes[len(data)] = notification_sizes.get(len(data), 0) + 1
            for value in decoder.feed(data):
                if ack_handler is not None and value[0] == ACK:
                    ack_handler(value)
                    continue
                queue.put_nowait((*value, now))
                if timing.active:
                    timing.queue_peak = max(timing.queue_peak,queue.qsize())
            timing.observe("notification_callback_work",time.monotonic()-now)

        await client.start_notify(notify, notification)
        result["notification_subscription_started"] = True

        # Bleak 1.1.1's CoreBluetooth backend does not wait for this queue.
        # Respect native backpressure without imposing a frame/fragment period.
        peripheral = getattr(client._backend, "_peripheral", None)
        result["native_write_backpressure"] = peripheral is not None

        async def send(kind, run, seq=0, payload=b"", should_send=None):
            return await send_raw(frame(kind, run, seq, payload),should_send)

        async def send_raw(raw, should_send=None):
            return await write_frame_chunks(raw,chunk,
                lambda: peripheral is None or peripheral.canSendWriteWithoutResponse(),
                lambda data: client.write_gatt_char(write,data,response=False),
                lambda: client.is_connected,args.timeout,timing,should_send)

        async def wait(kind, run, timeout=5, seq=None):
            deadline = time.monotonic() + timeout
            while True:
                message = await asyncio.wait_for(queue.get(), max(0.001, deadline - time.monotonic()))
                timing.observe("decoded_frame_queue_wait",time.monotonic()-message[4])
                if message[0] == kind and message[1] == run and (seq is None or message[2] == seq):
                    return message

        handshakes = {}

        async def handshake():
            client_nonce = secrets.randbits(128) or 1
            hello = b"RLB3" + client_nonce.to_bytes(16, "little") + struct.pack("<I", args.window_bytes)
            for attempt in range(3):
                await send(HELLO, 0, payload=hello)
                try:
                    end = time.monotonic()+5
                    while True:
                        challenge = await asyncio.wait_for(queue.get(), max(.001,end-time.monotonic()))
                        if challenge[0] == CHALLENGE and challenge[1] != 0 and challenge[2] == 0 and challenge[3] == client_nonce.to_bytes(16,"little"):
                            break
                    run = challenge[1]
                    await send(CONFIRM, run, payload=client_nonce.to_bytes(16,"little"))
                    ready = await wait(READY, run)
                    baud, max_payload, max_count, version, max_window, budget = struct.unpack("<IHHHHI", ready[3])
                    if version != 3 or args.payload > max_payload or args.count > max_count or args.window > max_window or budget < args.payload+OVERHEAD:
                        raise ValueError("Test exceeds negotiated firmware limits")
                    print(f"FIXED_READY session={run:032x} UART={baud} window_bytes={budget}", flush=True)
                    result["uart_baud"] = baud
                    result["protocol_version"] = version
                    handshakes[run] = (hello,client_nonce)
                    return run
                except asyncio.TimeoutError:
                    print(f"Handshake attempt {attempt+1} timed out", flush=True)
            raise RuntimeError("No verified challenge/confirmation response")

        if args.replay_test:
            async def acknowledged(run, turn, end):
                await send(POLL,run,turn,struct.pack("<I",end))
                reply=await wait(ACK,run,seq=turn)
                return struct.unpack("<I",reply[3])[0]
            async def statistics(run, turn):
                await send(REPORT,run,turn)
                reply=await wait(STATS,run,seq=turn)
                return struct.unpack("<8I",reply[3])
            old = await handshake()
            await send(DATA,old,0,pattern(0,33))
            assert await acknowledged(old,1,1) == 1
            await send(DATA,old,0,pattern(0,33))
            before=await statistics(old,2)
            assert before[0] == 1 and before[2] == 1
            current = await handshake()
            old_hello, old_client = handshakes[old]
            await send(HELLO,0,payload=old_hello)
            await send(CONFIRM,old,payload=old_client.to_bytes(16,"little"))
            await send(DATA,old,0,pattern(0,33))
            await send(DATA,old,1,pattern(1,33))
            assert await acknowledged(current,1,0) == 0
            await send(DATA,current,0,pattern(0,33))
            assert await acknowledged(current,2,1) == 1
            await send(CONFIRM,current,payload=handshakes[current][1].to_bytes(16,"little"))
            await wait(READY,current)
            await send(DATA,current,0,pattern(0,33))
            await send(ACK,current,999,struct.pack("<I",999))
            await send(DATA,current,1,pattern(1,33))
            assert await acknowledged(current,3,2) == 2
            after=await statistics(current,4)
            assert after[0] == 2 and after[2] == 1
            result["replay_validation"]={"passed":True,"old_session_rejected":True,
                "old_handshake_cannot_restore_session":True,"duplicate_confirm_preserves_data_id":True,
                "duplicate_data_delivered_once":True,"ack_does_not_consume_data_id":True}
            print(json.dumps(result["replay_validation"]),flush=True)

        rto = .4
        if args.direction in ("down", "both") and not args.replay_test:
            run = await handshake()
            decoder.errors = 0
            window = Window(args.payload+OVERHEAD, args.window_bytes, args.window)
            timing.reset()
            start = time.monotonic()
            deadline = start + args.timeout
            attempts = retransmits = high = turn = polls = ack_timeouts = 0
            ack_progress_events = cancelled_before_submit = cancelled_partial_frames = cancelled_wire_bytes = 0
            ack_changed = asyncio.Event()
            def receive_ack(message):
                nonlocal ack_progress_events
                if window.accept_ack(message,run,turn,high):
                    ack_progress_events += 1
                    ack_changed.set()
            ack_handler = receive_ack
            injected = False
            failure = None
            while window.base < args.count:
                if time.monotonic() >= deadline:
                    failure = "transfer deadline"
                    break
                window.begin(args.count)
                turn += 1
                for attempt in range(MAX_ATTEMPTS):
                    cursor = window.base
                    while True:
                        await asyncio.sleep(0)
                        seq = window.next_pending(cursor)
                        if seq is None:
                            break
                        raw = frame(DATA, run, seq, pattern(seq, args.payload))
                        if args.inject_loss and not injected and seq == 1:
                            raw = bytearray(raw)
                            raw[25] ^= 1
                            injected = True
                        submitted = await send_raw(raw,lambda: seq >= window.base)
                        if submitted:
                            retransmits += seq < high
                            high = max(high,seq+1)
                            attempts += 1
                            if submitted < len(raw):
                                cancelled_partial_frames += 1
                                cancelled_wire_bytes += submitted
                        else:
                            cancelled_before_submit += 1
                        cursor = seq+1
                    if window.base == window.end:
                        break
                    ack_changed.clear()
                    submitted = await send(POLL,run,turn,struct.pack("<I",window.end),
                        should_send=lambda: window.base < window.end)
                    if not submitted:
                        break
                    polls += 1
                    if window.base == window.end:
                        break
                    try:
                        await asyncio.wait_for(ack_changed.wait(),retry_timeout(attempt))
                    except asyncio.TimeoutError:
                        ack_timeouts += 1
                    await asyncio.sleep(0)
                    if window.base == window.end:
                        break
                if window.base < window.end:
                    failure = "retry limit"
                    break
            ack_handler = None
            stats = None
            for attempt in range(3):
                await send(REPORT, run, attempt)
                try:
                    stats = await wait(STATS, run, seq=attempt)
                    break
                except asyncio.TimeoutError:
                    pass
            if stats is None:
                raise RuntimeError("No receiver stats; no goodput can be claimed")
            elapsed = stats[4] - start
            count, received, duplicates, invalid, parse_errors, uart_errors, rx_us, out_of_order = struct.unpack("<8I", stats[3])
            row = {"direction": "mac_to_stm32", "requested_frames": args.count, "valid_unique_frames": count,
                   "missing_frames": args.count-count, "payload_bytes_received": received, "duplicates": duplicates,
                   "invalid_payload": invalid, "device_crc_parse_errors": parse_errors, "device_uart_errors": uart_errors,
                   "elapsed_seconds": elapsed, "payload_bytes_per_second": received/elapsed,
                   "steady_payload_bytes_per_second": (received-args.payload)*1e6/rx_us if count>1 and rx_us else None,
                   "data_attempts": attempts, "retransmitted_frames": retransmits, "poll_attempts": polls,
                   "final_window_frames": window.width, "injected_corruption": injected,
                   "discarded_out_of_order": out_of_order,
                   "unreceived_or_corrupt_data_attempts": attempts-count-duplicates-out_of_order,
                   "ack_timeouts_or_invalid": ack_timeouts,
                   "ack_progress_events": ack_progress_events,
                   "cancelled_before_submit": cancelled_before_submit,
                   "cancelled_partial_frames": cancelled_partial_frames,
                   "cancelled_wire_bytes": cancelled_wire_bytes,
                   "failure": failure,
                   "complete": failure is None and count == args.count}
            timing.active = False
            row["host_timing"] = timing.snapshot()
            result["tests"].append(row)
            print(json.dumps(row, indent=2), flush=True)

        if args.direction in ("up", "both") and not args.replay_test:
            run = await handshake()
            decoder.errors = 0
            next_seq = duplicates = invalid = gaps = received = 0
            dropped_data = dropped_ack = False
            command = struct.pack("<IHH", args.count, args.payload, args.window)
            raw_capture.update(active=True, bytes=0, first=None, last=None, bins={})
            timing.reset()
            start = time.monotonic()
            await send(TX_START, run, payload=command)
            deadline = start + args.timeout
            first_data = last_data = None
            done = None
            started = False
            while time.monotonic() < deadline:
                try:
                    kind, rx_run, seq, payload, timestamp = await asyncio.wait_for(queue.get(), min(rto+1, deadline-time.monotonic()))
                except asyncio.TimeoutError:
                    # Lost START is idempotent; lost DONE is read back from a cached result.
                    await send(REPORT if started else TX_START, run, payload=b"TX" if started else command)
                    continue
                timing.observe("decoded_frame_queue_wait",time.monotonic()-timestamp)
                if rx_run != run:
                    continue
                started = True
                if kind == TX_DONE and len(payload) == 32:
                    done = struct.unpack("<8I", payload)
                    break
                if kind == POLL and len(payload) == 4:
                    end, = struct.unpack("<I", payload)
                    if not next_seq <= end <= args.count:
                        continue
                    if args.inject_loss and not dropped_ack:
                        dropped_ack = True
                        continue
                    await send(ACK, run, seq, struct.pack("<I", next_seq))
                    timing.observe("poll_callback_to_ack_submitted",time.monotonic()-timestamp)
                    continue
                if kind != TX_DATA:
                    continue
                if seq >= args.count or payload != pattern(seq, args.payload):
                    invalid += 1
                elif args.inject_loss and not dropped_data and seq == 1:
                    dropped_data = True
                elif seq < next_seq:
                    duplicates += 1
                elif seq > next_seq:
                    gaps += 1
                else:
                    next_seq += 1
                    received += len(payload)
                    first_data = first_data or timestamp
                    last_data = timestamp
            elapsed = time.monotonic()-start
            raw_capture["active"] = False
            row = {"direction": "stm32_to_mac", "requested_frames": args.count, "valid_unique_frames": next_seq,
                   "missing_frames": args.count-next_seq, "payload_bytes_received": received, "duplicates": duplicates,
                   "discarded_out_of_order": gaps, "invalid_payload": invalid, "host_crc_parse_errors": decoder.errors,
                   "done_received": done is not None, "acknowledged_frames": done[0] if done else None,
                   "data_attempts": done[1] if done else None, "retransmitted_frames": done[2] if done else None,
                   "poll_attempts": done[3] if done else None, "device_transfer_elapsed_us": done[4] if done else None,
                   "device_status": done[5] if done else None,
                   "unreceived_or_corrupt_data_attempts": done[1]-next_seq-duplicates-gaps if done else None,
                   "ack_timeouts_or_invalid": done[7] if done else None,
                   "elapsed_seconds": elapsed, "payload_bytes_per_second": received/elapsed,
                   "steady_payload_bytes_per_second": (received-args.payload)/(last_data-first_data) if next_seq>1 and last_data>first_data else None,
                   "injected_data_loss": dropped_data, "injected_ack_loss": dropped_ack,
                   "complete": done is not None and done[5] == 0 and done[0] == args.count and next_seq == args.count,
                   "raw_notification_bytes_including_control": raw_capture["bytes"]}
            timing.active = False
            row["host_timing"] = timing.snapshot()
            result["tests"].append(row)
            print(json.dumps(row, indent=2), flush=True)
        await client.stop_notify(notify)
        if delegate is not None:
            delegate._event_loop = original_loop
        result["notification_size_histogram"] = notification_sizes
    if args.output:
        path = Path(args.output)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(result, indent=2, ensure_ascii=False)+"\n")
        print(f"Saved {path}", flush=True)
    return result


def matrix_markdown(rows):
    sizes = sorted({r["wire_frame_bytes"] for r in rows})
    windows = sorted({r["window"] for r in rows})
    lines = ["每格：下行 kB/s / 上行 kB/s / 下行丢包率 / 上行丢包率。† 表示至少一个方向未完成。", "",
             "| 窗口＼整包字节 | " + " | ".join(map(str, sizes)) + " |",
             "|---:|" + "---|" * len(sizes)]
    for window in windows:
        cells = []
        for size in sizes:
            row = next((r for r in rows if r["wire_frame_bytes"] == size and r["window"] == window), None)
            if row is None:
                cells.append("—")
                continue
            tests = {t["direction"]: t for t in row.get("tests", [])}
            rates, losses = [], []
            for direction in ("mac_to_stm32", "stm32_to_mac"):
                test = tests.get(direction, {})
                rate = test.get("payload_bytes_per_second")
                attempts = test.get("data_attempts")
                lost = test.get("unreceived_or_corrupt_data_attempts")
                rates.append(f"{rate/1000:.2f}" if rate is not None else "—")
                losses.append(f"{100*lost/attempts:.2f}%" if lost is not None and attempts else "—")
            cells.append("/".join(rates+losses) + ("†" if not row.get("complete") else ""))
        lines.append("| " + str(window) + " | " + " | ".join(cells) + " |")
    return "\n".join(lines)+"\n"


async def matrix(args):
    """Persist every completed case; failures never count as successful throughput."""
    destination = Path(args.output or "output/stm32g431/matrix")
    destination.mkdir(parents=True, exist_ok=True)
    index = destination / "summary.json"
    summary = json.loads(index.read_text()) if args.matrix_resume and index.exists() else []
    sizes = [int(x) for x in args.matrix_frames.split(",")]
    windows = [int(x) for x in args.matrix_windows.split(",")]
    from bleak import BleakClient
    shared = None
    device = None
    try:
        for size in sizes:
            for window in windows:
                case = copy.copy(args)
                case.matrix = False
                case.payload = size - OVERHEAD
                case.window = window
                case.window_bytes = size * window
                case.count = (args.matrix_bytes + case.payload - 1) // case.payload
                case.output = str(destination / f"frame{size}-window{window}.json")
                previous = next((r for r in summary if r["wire_frame_bytes"] == size and r["window"] == window), None)
                if previous and previous.get("complete") and previous.get("count") == case.count:
                    print(f"MATRIX_SKIP frame={size} window={window} already complete", flush=True)
                    continue
                print(f"MATRIX_BEGIN frame={size} window={window} count={case.count}", flush=True)
                row = {"wire_frame_bytes": size, "window": window, "count": case.count}
                if previous:
                    row["previous_attempts"] = previous.get("previous_attempts", []) + [{k: v for k, v in previous.items() if k != "previous_attempts"}]
                try:
                    if shared is None:
                        device = await discover(case)
                        shared = BleakClient(device, timeout=20)
                        await shared.connect()
                    result = await benchmark(case, device, shared)
                    row["tests"] = result["tests"]
                    row["complete"] = all(t["complete"] for t in result["tests"])
                except Exception as error:
                    row.update(complete=False, error=f"{type(error).__name__}: {error}")
                    if isinstance(error,NativeWriteBlocked):
                        row["host_timing"] = error.host_timing
                    print(f"MATRIX_FAILED {row}", flush=True)
                    if shared is not None:
                        await shared.disconnect()
                        shared = None
                if previous:
                    summary[summary.index(previous)] = row
                else:
                    summary.append(row)
                index.write_text(json.dumps(summary, indent=2)+"\n")
                (destination / "matrix.md").write_text(matrix_markdown(summary))
                print(f"MATRIX_END frame={size} window={window} complete={row['complete']}", flush=True)
    finally:
        if shared is not None:
            await shared.disconnect()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scan", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--name", default="G431-BLE")
    parser.add_argument("--address")
    parser.add_argument("--scan-seconds", type=float, default=8)
    parser.add_argument("--write-uuid", help="16-bit or full UUID; default: unique write+notify characteristic")
    parser.add_argument("--notify-uuid", help="16-bit or full UUID; default: unique write+notify characteristic")
    parser.add_argument("--direction", choices=["down", "up", "both"], default="both")
    parser.add_argument("--count", type=int, default=128)
    parser.add_argument("--payload", type=int, default=993, help="diagnostic payload; 993 produces a 1024-byte wire frame")
    parser.add_argument("--chunk", type=int, default=182)
    parser.add_argument("--window", type=int, default=2, help="maximum unacknowledged packets, 1..8")
    parser.add_argument("--window-bytes", type=int, default=2048, help="maximum unacknowledged wire bytes, 64..8192")
    parser.add_argument("--replay-test", action="store_true", help="validate duplicate and retired-session replay rejection on hardware")
    parser.add_argument("--inject-loss", action="store_true", help="once corrupt downlink data, drop uplink data and ACK")
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--output")
    parser.add_argument("--matrix", action="store_true", help="sweep equal-byte, symmetric frame/window combinations")
    parser.add_argument("--matrix-resume", action="store_true", help="skip complete cases; retry failed cases and preserve attempt history")
    parser.add_argument("--matrix-frames", default="256,512,1024,182,128,64", help="comma-separated full wire lengths")
    parser.add_argument("--matrix-windows", default="1,2,4,8")
    parser.add_argument("--matrix-bytes", type=int, default=16384, help="minimum unique payload bytes per direction per case")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not 1 <= args.count <= 8192 or not 1 <= args.payload <= MAX_PAYLOAD or args.chunk < 1 or not 1 <= args.window <= 8 or not 64 <= args.window_bytes <= 8192 or args.payload+OVERHEAD > args.window_bytes or args.timeout <= 0:
        parser.error("test parameters outside supported range")
    if args.matrix:
        try:
            sizes = [int(x) for x in args.matrix_frames.split(",")]
            windows = [int(x) for x in args.matrix_windows.split(",")]
            if not sizes or not windows or not 1 <= args.matrix_bytes <= 200000:
                raise ValueError()
            for size in sizes:
                if not 64 <= size <= MAX_PAYLOAD+OVERHEAD or (args.matrix_bytes+size-OVERHEAD-1)//(size-OVERHEAD) > 8192:
                    raise ValueError()
                for window in windows:
                    if not 1 <= window <= 8 or size*window > 8192:
                        raise ValueError()
        except ValueError:
            parser.error("invalid matrix sizes/windows/byte budget")
    asyncio.run(matrix(args) if args.matrix else benchmark(args))


if __name__ == "__main__":
    main()
