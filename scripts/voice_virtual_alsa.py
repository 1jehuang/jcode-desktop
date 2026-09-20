"""Public-fixture ALSA source for isolated native acceptance. Never accesses devices.
Use ONLY inside bwrap --dev /dev, with this entire ALSA_CONFIG_PATH (no includes).
Verified on Linux CPAL 0.15.3: default null/file negotiates 44100Hz F32 stereo.
No application mocks: native CPAL/file/null capture and production resampler run.

pump = VirtualAlsa(scratch_dir, public_hello_path, lead_seconds=3, tail_seconds=20)
launch normal app with ALSA_CONFIG_PATH=str(pump.config) in mandatory private /dev
pump.start()  # start after UI enters recording; safe to start immediately too
... await final transcript/capture release, then terminate isolated app ...
pump.close()  # NEVER stop draining while native capture might still be active

The file plugin MUST read a REGULAR file in the exact negotiated native format.
Its OUTPUT FIFO is drained at 352800 bytes/s to pace capture by backpressure.
Do not use infile FIFO or ALSA plug: observed short reads/mmap skipped samples.
"""
import array
import fcntl
import hashlib
import os
from pathlib import Path
import threading
import time

FIXTURE_SHA256 = "6f6c586a333ba03aa961e6c5f999050023eaec54df0a1a900b127e8523edf08b"
NATIVE_RATE = 44100
BYTES_PER_SECOND = 44100 * 2 * 4

class VirtualAlsa:
    def __init__(self, root, fixture, lead_seconds=3.0, tail_seconds=20.0):
        self.root = Path(root).resolve()
        if any(character in str(self.root) for character in ('"', '\\', '\n', '\r')):
            raise ValueError("unsafe ALSA configuration path")
        self.root.mkdir(parents=True, exist_ok=True)
        if not 0 <= lead_seconds <= 3 or not 1 <= tail_seconds <= 20:
            raise ValueError("bounded lead/tail required")
        source = Path(fixture).read_bytes()
        if hashlib.sha256(source).hexdigest() != FIXTURE_SHA256:
            raise ValueError("only verified public hello.pcm fixture accepted")
        if os.sys.byteorder != "little":
            raise RuntimeError("native fixture implementation requires little endian")
        samples = array.array("h", source)
        out = array.array("f", [0.0]) * (round(lead_seconds * NATIVE_RATE) * 2)
        for index in range(round(len(samples) * NATIVE_RATE / 16000)):
            pos = index * 16000 / NATIVE_RATE
            left = int(pos)
            frac = pos - left
            x = samples[left] if left < len(samples) else 0
            y = samples[left + 1] if left + 1 < len(samples) else 0
            value = (x + (y - x) * frac) / 32767
            out.extend((value, value))
        out.extend(array.array("f", [0.0]) * (round(tail_seconds * NATIVE_RATE) * 2))
        self.native = self.root / "public-native.f32le"
        self.native.write_bytes(out.tobytes())
        self.fifo = self.root / "capture-drain.fifo"
        if self.fifo.exists():
            raise FileExistsError("use a fresh private scratch directory")
        os.mkfifo(self.fifo, 0o600)
        self.fd = os.open(self.fifo, os.O_RDWR | os.O_NONBLOCK)
        fcntl.fcntl(self.fd, fcntl.F_SETPIPE_SZ, 4096)
        self.config = self.root / "alsa.conf"
        self.config.write_text(
            'pcm.!default { type file slave.pcm { type null } '
            f'file "{self.fifo}" infile "{self.native}" format "raw" }}\n'
        )
        self.bytes_drained = 0
        self.failure = None
        self._stop = threading.Event()
        self._thread = None

    def start(self):
        if self._thread is not None:
            raise RuntimeError("already started")
        def drain():
            next_at = time.monotonic()
            try:
                while not self._stop.is_set():
                    now = time.monotonic()
                    if now >= next_at:
                        try:
                            data = os.read(self.fd, 3528)  # 10 ms, below PIPE_BUF
                        except BlockingIOError:
                            data = b""
                        if data:
                            self.bytes_drained += len(data)
                            # Absolute schedule compensates short FIFO reads and
                            # scheduler jitter without slowing the audio clock.
                            next_at += len(data) / BYTES_PER_SECOND
                        elif self.bytes_drained == 0:
                            next_at = now
                    self._stop.wait(.001)
            except BaseException as error:
                self.failure = type(error).__name__
        self._thread = threading.Thread(target=drain, name="public-alsa-drain", daemon=True)
        self._thread.start()

    def close(self):
        """Call only AFTER capture/app has ended, else native output may block."""
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=1)
        os.close(self.fd)
