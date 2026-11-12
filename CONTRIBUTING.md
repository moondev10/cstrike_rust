# Contributing

## The rule

cstrike-game reproduces the original client: HL25, build 10210, protocol 48. Every change must do
one of two things:
- make the client behave more like the original;
- add one of the deliberate differences listed in the README.

Any other behaviour change is a bug, including improvements the original does not have.

A pull request that changes behaviour needs evidence of what the original does. Any one of these
will do:
- a capture or screenshot from the original client;
- a demo;
- a server log;
- the function name and address in `hw.dll` or `client.dll` it comes from.

A deliberate difference (a security fix, platform support) must also add its entry to the
README's "Differences from the original" in the same pull request.

## Before you open a pull request

```sh
cargo test --workspace
cargo clippy --workspace --all-targets   # must print no warnings
```

Run the client through the path you changed. `cstrike-game play --help` lists the options for
scripted, windowless runs: `--hidden`, `--cmd`, `--shot`, `--quit-after`.

## Code

- **`unsafe`:** only in the three FFI modules (`cstrike-game/src/ffi.rs`, `gs-steam/src/ffi.rs`,
  `gs-voicecodec/src/ffi.rs`), and every block needs a `// SAFETY:` comment.
- **No `unwrap`, `expect` or `panic!` outside tests.** Servers, demos and maps are untrusted
  input: return an error, or fall back the way the original does.
- **Statics:** no `Box::leak`; prefer `std::sync::LazyLock`.
- **Formatting:** do not run `cargo fmt` on the tree. The code is not rustfmt-formatted; match the
  surrounding style.
- **Doc comments:** name the original function a piece of code reproduces (`CL_ParseTEnt`,
  `PM_FlyMove`), so it can be checked against the binary. Comments that cite `docs/re/*.md` refer
  to unpublished notes; the function names and addresses are enough.
- **Tests:** check what a player or a server would notice (protocol bytes, movement results, edge
  cases, errors). Do not test wiring or incidental defaults.
- **Never commit** game data or Valve binaries: maps, models, sounds, `*.dll`, decompiler output.

## Commits

- One logical change per commit.
- The first line says what changes, e.g. `gs-pmove: clamp the step height to sv_stepsize`.
- The body says why, and what was checked.

## Reporting bugs

Use the issue form. The most useful report includes:
- the same scene in the original client and in this one;
- the server address and the steps;
- the log: `RUST_LOG=debug cstrike-game play ...`.

Security problems go through [SECURITY.md](SECURITY.md), not public issues.
