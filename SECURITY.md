# Security policy

## What counts

The client talks to servers it does not control. A security problem is anything that a server, a
demo, a map, a downloaded file or a MOTD page can make the client do beyond what playing needs:
- run code or commands on the player's machine;
- read, write or delete files outside the game directories;
- reach the player's machine or local network;
- crash or hang the client, or exhaust its memory or disk.

Cheats and anti-cheat evasion are not security problems.

## Supported versions

Only the latest commit on `main`.

## Reporting

Report privately on this repository: **Security → Report a vulnerability**. Do not open a public
issue.

Include:
- what the attacker controls (a server, demo, map, download or MOTD);
- the steps or a proof of concept;
- the commit you tested.

The fix goes to `main` first; the advisory is published after it.
