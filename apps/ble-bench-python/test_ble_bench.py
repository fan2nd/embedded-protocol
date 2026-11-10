"""ACK scheduling regressions; no BLE hardware is needed."""
import asyncio
import struct
import unittest

import ble_bench as bench


def ack(next_seq, run=7, turn=1):
    return (bench.ACK,run,turn,struct.pack("<I",next_seq))


class AckSchedulingTests(unittest.IsolatedAsyncioTestCase):
    def window(self):
        window = bench.Window(256,2048,8)
        window.begin(16)
        return window

    async def test_ack_while_write_queue_blocked_cancels_before_first_fragment(self):
        window = self.window()
        writes = []
        async def write(data):
            writes.append(data)
        def ready():
            asyncio.get_running_loop().call_soon(window.accept_ack,ack(8),7,1,8)
            return False
        submitted = await asyncio.wait_for(bench.write_frame_chunks(
            bench.frame(bench.DATA,7,0,b"x"*225),182,ready,write,
            lambda: True,1,bench.HostTiming(),lambda: window.base == 0),.1)
        self.assertFalse(submitted)
        self.assertEqual(writes,[])
        self.assertEqual(window.width,8)

    async def test_ack_during_fragmented_retry_skips_acknowledged_tail(self):
        window = self.window()
        decoder = bench.Decoder()
        packets = []
        fragments = []
        async def write(data):
            if data[:2] == bench.SOF:
                seq = struct.unpack_from("<I",data,19)[0]
                fragments.append(seq)
                next_seq = 3 if seq == 0 else 8
                asyncio.get_running_loop().call_soon(window.accept_ack,ack(next_seq),7,1,8)
            packets.extend(decoder.feed(data))
        cursor = 0
        while (seq := window.next_pending(cursor)) is not None:
            await bench.write_frame_chunks(bench.frame(bench.DATA,7,seq,b"x"*225),182,
                lambda: True,write,lambda: True,1,bench.HostTiming(),lambda: seq >= window.base)
            cursor = seq+1
        # Both retries stop at the first ATT fragment, then a complete packet
        # from the next window recovers through CRC/header resynchronization.
        self.assertEqual(fragments,[0,3])
        packets.extend(decoder.feed(bench.frame(bench.DATA,7,8,b"x"*225)))
        self.assertEqual([packet[2] for packet in packets],[8])
        self.assertEqual(window.base,8)
        self.assertEqual(window.width,8)

    async def test_queued_ack_is_processed_before_unblocked_first_submission(self):
        window = self.window()
        async def write(data):
            self.fail("already acknowledged frame was submitted")
        asyncio.get_running_loop().call_soon(window.accept_ack,ack(8),7,1,8)
        submitted = await bench.write_frame_chunks(b"test",182,lambda: True,write,
            lambda: True,1,bench.HostTiming(),lambda: window.base == 0)
        self.assertFalse(submitted)

    def test_stale_duplicate_and_unsent_acks_do_not_trigger_retry(self):
        window = self.window()
        self.assertTrue(window.accept_ack(ack(3),7,1,4))
        for message in (ack(3),ack(2),ack(8),ack(4,run=6),ack(4,turn=0)):
            self.assertFalse(window.accept_ack(message,7,1,4))
        self.assertEqual(window.base,3)
        self.assertEqual(window.next_pending(1),3)
        self.assertEqual([bench.retry_timeout(n) for n in range(8)],[.1,.2,.4,.4,.4,.4,.4,.4])

    def test_all_coalesced_ack_frames_advance_the_same_window(self):
        window = self.window()
        decoder = bench.Decoder()
        raw = b"".join(bench.frame(*ack(n)) for n in (2,5,8))
        for message in decoder.feed(raw):
            window.accept_ack(message,7,1,8)
        self.assertEqual(window.base,8)
        self.assertEqual(window.width,8)


if __name__ == "__main__":
    unittest.main()
