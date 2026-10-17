#!/usr/bin/env python3
"""Run native UI checks on a private input desktop on disposable hosted CI only.

This does not dismiss OOBE, change privacy settings, or terminate system apps.
The child runs the unchanged native checks on a real visible Windows desktop.
The original input and thread desktops are restored in finally, even on failure.
"""
import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def run_child(kernel, command, desktop, timeout):
    # Python's subprocess.STARTUPINFO does not expose lpDesktop. Use the real
    # CreateProcessW structure so the test process and its children inherit it.
    class Startup(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("lpReserved", wintypes.LPWSTR),
                    ("lpDesktop", wintypes.LPWSTR), ("lpTitle", wintypes.LPWSTR),
                    ("dwX", wintypes.DWORD), ("dwY", wintypes.DWORD),
                    ("dwXSize", wintypes.DWORD), ("dwYSize", wintypes.DWORD),
                    ("dwXCountChars", wintypes.DWORD), ("dwYCountChars", wintypes.DWORD),
                    ("dwFillAttribute", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
                    ("wShowWindow", wintypes.WORD), ("cbReserved2", wintypes.WORD),
                    ("lpReserved2", ctypes.POINTER(wintypes.BYTE)),
                    ("hStdInput", wintypes.HANDLE), ("hStdOutput", wintypes.HANDLE),
                    ("hStdError", wintypes.HANDLE)]
    class Process(ctypes.Structure):
        _fields_ = [("hProcess", wintypes.HANDLE), ("hThread", wintypes.HANDLE),
                    ("dwProcessId", wintypes.DWORD), ("dwThreadId", wintypes.DWORD)]
    kernel.CreateProcessW.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.c_void_p,
                                     ctypes.c_void_p, wintypes.BOOL, wintypes.DWORD,
                                     ctypes.c_void_p, wintypes.LPCWSTR,
                                     ctypes.POINTER(Startup), ctypes.POINTER(Process)]
    kernel.CreateProcessW.restype = wintypes.BOOL
    kernel.GetStdHandle.argtypes = [wintypes.DWORD]
    kernel.GetStdHandle.restype = wintypes.HANDLE
    kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel.WaitForSingleObject.restype = wintypes.DWORD
    kernel.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    kernel.GetExitCodeProcess.restype = wintypes.BOOL
    kernel.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    info = Startup()
    info.cb, info.lpDesktop, info.dwFlags = ctypes.sizeof(info), desktop, 0x100
    info.hStdInput, info.hStdOutput, info.hStdError = [kernel.GetStdHandle(n) for n in (-10, -11, -12)]
    process = Process()
    args = ctypes.create_unicode_buffer(subprocess.list2cmdline(command))
    if not kernel.CreateProcessW(None, args, None, None, True, 0x08000000, None, None,
                                 ctypes.byref(info), ctypes.byref(process)):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        status = kernel.WaitForSingleObject(process.hProcess, timeout * 1000)
        if status == 258:  # WAIT_TIMEOUT. Only our own test child is terminated.
            kernel.TerminateProcess(process.hProcess, 1)
            kernel.WaitForSingleObject(process.hProcess, 10000)
            raise TimeoutError(f"Native UI test exceeded {timeout}s")
        if status != 0:
            raise ctypes.WinError(ctypes.get_last_error())
        code = wintypes.DWORD()
        if not kernel.GetExitCodeProcess(process.hProcess, ctypes.byref(code)):
            raise ctypes.WinError(ctypes.get_last_error())
        return code.value
    finally:
        kernel.CloseHandle(process.hThread)
        kernel.CloseHandle(process.hProcess)


def run(command, evidence, timeout=900):
    if (os.name != "nt" or os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"
            or os.environ.get("RUNNER_OS") != "Windows"):
        raise RuntimeError("Private input desktops are allowed only on disposable GitHub-hosted Windows CI")
    user = ctypes.WinDLL("user32", use_last_error=True)
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.GetCurrentThreadId.restype = wintypes.DWORD
    user.GetThreadDesktop.argtypes = [wintypes.DWORD]
    user.GetThreadDesktop.restype = wintypes.HANDLE
    user.OpenInputDesktop.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    user.OpenInputDesktop.restype = wintypes.HANDLE
    user.CreateDesktopW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, ctypes.c_void_p,
                                   wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p]
    user.CreateDesktopW.restype = wintypes.HANDLE
    for name in ("SwitchDesktop", "SetThreadDesktop", "CloseDesktop"):
        getattr(user, name).argtypes = [wintypes.HANDLE]
        getattr(user, name).restype = wintypes.BOOL
    original_thread = user.GetThreadDesktop(kernel.GetCurrentThreadId())
    if not original_thread:
        raise ctypes.WinError(ctypes.get_last_error())
    original_input = user.OpenInputDesktop(0, False, 0x100)  # DESKTOP_SWITCHDESKTOP
    if not original_input:
        raise ctypes.WinError(ctypes.get_last_error())
    private = None
    report = {"environment": "github-hosted", "command": command, "events": []}
    primary = None
    result = None
    try:
        name = f"JcodeAcceptance_{os.getpid()}_{time.monotonic_ns()}"
        private = user.CreateDesktopW(name, None, None, 0, 0x10000000, None)  # GENERIC_ALL
        if not private:
            raise ctypes.WinError(ctypes.get_last_error())
        report["desktop"] = name
        report["events"].append("created_private_desktop")
        if not user.SwitchDesktop(private):
            raise ctypes.WinError(ctypes.get_last_error())
        report["events"].append("switched_to_private_desktop")
        result = run_child(kernel, command, name, timeout)
        report["child_returncode"] = result
    except BaseException as error:
        primary = error
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        cleanup_errors = []
        for label, function, handle in (
                ("restored_input_desktop", user.SwitchDesktop, original_input),
                ("restored_thread_desktop", user.SetThreadDesktop, original_thread),
                ("closed_private_desktop", user.CloseDesktop, private),
                ("closed_original_input_handle", user.CloseDesktop, original_input)):
            if handle:
                if function(handle):
                    report["events"].append(label)
                else:
                    cleanup_errors.append(f"{label}: WinError {ctypes.get_last_error()}")
        report["cleanup_errors"] = cleanup_errors
        report["success"] = primary is None and not cleanup_errors and result == 0
        evidence.parent.mkdir(parents=True, exist_ok=True)
        evidence.write_text(json.dumps(report, indent=2), encoding="utf-8")
        if cleanup_errors and primary is None:
            primary = RuntimeError("Private desktop cleanup failed: " + "; ".join(cleanup_errors))
    if primary:
        raise primary
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or args.timeout <= 0:
        parser.error("A command and positive timeout are required")
    return run(command, args.evidence, args.timeout)


if __name__ == "__main__":
    sys.exit(main())
