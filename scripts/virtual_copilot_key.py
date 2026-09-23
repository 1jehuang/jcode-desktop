#!/usr/bin/env python3
"""Virtual Copilot key for isolated voice tests, via /dev/uinput.

The device also advertises gamepad axes/buttons, so udev tags it as a joystick
and libinput (and therefore the user's compositor) ignores it. Callers must
verify the live compositor never opened the node (scripts check /proc/<pid>/fd)
before pressing anything.

Protocol on stdin, one command per line: press | release | quit.
Prints the created /dev/input/eventN path on the first stdout line.
"""
import ctypes
import fcntl
import os
import struct
import sys
import time
from pathlib import Path

EV_SYN, EV_KEY, EV_ABS = 0, 1, 3
KEY_LEFTSHIFT, KEY_LEFTMETA, KEY_F23 = 42, 125, 193
GAMEPAD_BUTTONS = range(0x130, 0x13f)  # BTN_SOUTH..BTN_THUMBR
AXES = range(0, 6)  # ABS_X..ABS_RZ
NAME = b"jcode-voice-test-copilot"


def ioc(direction, nr, size):
    return (direction << 30) | (size << 16) | (ord('U') << 8) | nr


UI_SET_EVBIT = ioc(1, 100, 4)
UI_SET_KEYBIT = ioc(1, 101, 4)
UI_SET_ABSBIT = ioc(1, 103, 4)
UI_DEV_SETUP = ioc(1, 3, 92)
UI_ABS_SETUP = ioc(1, 4, 28)
UI_DEV_CREATE = ioc(0, 1, 0)
UI_DEV_DESTROY = ioc(0, 2, 0)
UI_GET_SYSNAME = ioc(2, 44, 64)


def emit(fd, kind, code, value):
    os.write(fd, struct.pack('llHHi', 0, 0, kind, code, value))


def main():
    fd = os.open('/dev/uinput', os.O_WRONLY | os.O_NONBLOCK)
    for ev in (EV_KEY, EV_ABS):
        fcntl.ioctl(fd, UI_SET_EVBIT, ev)
    for key in (KEY_LEFTSHIFT, KEY_LEFTMETA, KEY_F23, *GAMEPAD_BUTTONS):
        fcntl.ioctl(fd, UI_SET_KEYBIT, key)
    for axis in AXES:
        fcntl.ioctl(fd, UI_SET_ABSBIT, axis)
        # struct uinput_abs_setup { u16 code; struct input_absinfo {s32 x6} }
        fcntl.ioctl(fd, UI_ABS_SETUP, struct.pack('Hxx6i', axis, 0, -32768, 32767, 0, 0, 0))
    # struct uinput_setup { input_id {u16 bustype,vendor,product,version}; char name[80]; u32 ff }
    fcntl.ioctl(fd, UI_DEV_SETUP, struct.pack('4H80sI', 0x06, 0x1d6b, 0x7a17, 1, NAME, 0))
    fcntl.ioctl(fd, UI_DEV_CREATE)
    buf = ctypes.create_string_buffer(64)
    fcntl.ioctl(fd, UI_GET_SYSNAME, buf)
    sysname = buf.value.decode()
    deadline = time.monotonic() + 5
    node = None
    while node is None and time.monotonic() < deadline:
        for child in Path('/sys/devices/virtual/input', sysname).glob('event*'):
            node = '/dev/input/' + child.name
        time.sleep(.05)
    if node is None:
        raise SystemExit('uinput event node did not appear')
    print(node, flush=True)
    try:
        for line in sys.stdin:
            cmd = line.strip()
            if cmd == 'press':
                for key in (KEY_LEFTSHIFT, KEY_LEFTMETA, KEY_F23):
                    emit(fd, EV_KEY, key, 1)
                    emit(fd, EV_SYN, 0, 0)
            elif cmd == 'release':
                for key in (KEY_F23, KEY_LEFTMETA, KEY_LEFTSHIFT):
                    emit(fd, EV_KEY, key, 0)
                    emit(fd, EV_SYN, 0, 0)
            elif cmd == 'quit':
                break
            print('ok ' + cmd, flush=True)
    finally:
        fcntl.ioctl(fd, UI_DEV_DESTROY)
        os.close(fd)


if __name__ == '__main__':
    main()
