"""GoldSrc rcon client: `python tools/rcon.py [--addr 127.0.0.1:27015] [--password test] cmd...`.

Each argument is sent as one rcon command; replies are printed.
"""

import argparse
import socket

OOB = b"\xff\xff\xff\xff"


def request(sock, addr, payload):
    sock.sendto(OOB + payload, addr)
    data, _ = sock.recvfrom(65536)
    return data[4:]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--addr", default="127.0.0.1:27015")
    ap.add_argument("--password", default="test")
    ap.add_argument("commands", nargs="+")
    args = ap.parse_args()
    host, port = args.addr.rsplit(":", 1)
    addr = (host, int(port))
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(3.0)
    reply = request(sock, addr, b"challenge rcon\n").decode("latin-1").split()
    challenge = reply[2]
    for cmd in args.commands:
        payload = f'rcon {challenge} "{args.password}" {cmd}\n'.encode("latin-1")
        try:
            text = request(sock, addr, payload)
            print(text[1:].decode("latin-1").rstrip())
        except socket.timeout:
            print(f"({cmd}: no reply)")


if __name__ == "__main__":
    main()
