"""Run a Windows program on an invisible desktop, relaying its stdout/stderr.

Used for test infrastructure (e.g. hlds.exe) so crash/assert message boxes never appear on the
user's screen. The child sees a separate, never-displayed desktop; its output is piped here.

    python tools/hidden_run.py [--cwd DIR] -- program.exe args...
"""
import argparse
import ctypes
import subprocess
import sys
from ctypes import wintypes

DESKTOP_NAME = "cstrike_rust_hidden"
GENERIC_ALL = 0x10000000
STARTF_USESTDHANDLES = 0x00000100
CREATE_NO_WINDOW = 0x08000000
HANDLE_FLAG_INHERIT = 0x00000001
INFINITE = 0xFFFFFFFF

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
user32 = ctypes.WinDLL("user32", use_last_error=True)


class STARTUPINFOW(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD), ("lpReserved", wintypes.LPWSTR), ("lpDesktop", wintypes.LPWSTR),
        ("lpTitle", wintypes.LPWSTR), ("dwX", wintypes.DWORD), ("dwY", wintypes.DWORD),
        ("dwXSize", wintypes.DWORD), ("dwYSize", wintypes.DWORD), ("dwXCountChars", wintypes.DWORD),
        ("dwYCountChars", wintypes.DWORD), ("dwFillAttribute", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
        ("wShowWindow", wintypes.WORD), ("cbReserved2", wintypes.WORD), ("lpReserved2", ctypes.c_void_p),
        ("hStdInput", wintypes.HANDLE), ("hStdOutput", wintypes.HANDLE), ("hStdError", wintypes.HANDLE),
    ]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [("hProcess", wintypes.HANDLE), ("hThread", wintypes.HANDLE),
                ("dwProcessId", wintypes.DWORD), ("dwThreadId", wintypes.DWORD)]


class SECURITY_ATTRIBUTES(ctypes.Structure):
    _fields_ = [("nLength", wintypes.DWORD), ("lpSecurityDescriptor", ctypes.c_void_p), ("bInheritHandle", wintypes.BOOL)]


def check(ok, what):
    if not ok:
        raise OSError(ctypes.get_last_error(), what)
    return ok


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cwd")
    parser.add_argument("cmd", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    cmd = args.cmd[1:] if args.cmd[:1] == ["--"] else args.cmd
    if not cmd:
        parser.error("missing program")

    user32.CreateDesktopW.restype = wintypes.HANDLE
    desktop = check(user32.CreateDesktopW(DESKTOP_NAME, None, None, 0, GENERIC_ALL, None), "CreateDesktopW")

    sa = SECURITY_ATTRIBUTES(ctypes.sizeof(SECURITY_ATTRIBUTES), None, True)
    read_end, write_end = wintypes.HANDLE(), wintypes.HANDLE()
    check(kernel32.CreatePipe(ctypes.byref(read_end), ctypes.byref(write_end), ctypes.byref(sa), 0), "CreatePipe")
    check(kernel32.SetHandleInformation(read_end, HANDLE_FLAG_INHERIT, 0), "SetHandleInformation")

    si = STARTUPINFOW()
    si.cb = ctypes.sizeof(STARTUPINFOW)
    si.lpDesktop = DESKTOP_NAME
    si.dwFlags = STARTF_USESTDHANDLES
    si.hStdOutput = write_end
    si.hStdError = write_end
    pi = PROCESS_INFORMATION()
    cmdline = ctypes.create_unicode_buffer(subprocess.list2cmdline(cmd))
    check(kernel32.CreateProcessW(None, cmdline, None, None, True, 0, None, args.cwd, ctypes.byref(si), ctypes.byref(pi)),
          "CreateProcessW")
    kernel32.CloseHandle(write_end)
    print(f"[hidden_run] pid {pi.dwProcessId} on desktop {DESKTOP_NAME}", flush=True)

    buf = ctypes.create_string_buffer(4096)
    read = wintypes.DWORD()
    while kernel32.ReadFile(read_end, buf, len(buf), ctypes.byref(read), None) and read.value:
        sys.stdout.buffer.write(buf.raw[:read.value])
        sys.stdout.buffer.flush()

    kernel32.WaitForSingleObject(pi.hProcess, INFINITE)
    code = wintypes.DWORD()
    kernel32.GetExitCodeProcess(pi.hProcess, ctypes.byref(code))
    for h in (pi.hProcess, pi.hThread, read_end):
        kernel32.CloseHandle(h)
    user32.CloseDesktop(desktop)
    print(f"[hidden_run] exit code {code.value}", flush=True)
    sys.exit(code.value & 0xFF)


if __name__ == "__main__":
    main()
