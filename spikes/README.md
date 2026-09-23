# Spikes

Standalone crates that answer one question and are then deleted.

Each is a **separate cargo package, in no workspace**, and that is the point of
the directory rather than an accident of layout:

- **Tauri's bundler installs every bin target its package produces.** A release
  once shipped `gen-contract.exe` into the install directory and pointed a
  Start Menu shortcut at it. A throwaway binary must not be able to reach a
  bundle at all, and the cheapest guarantee is that it belongs to a different
  crate.
- **They type-check from a Linux dev box.** A spike depends on `windows` and
  nothing else, so `cargo check --target x86_64-pc-windows-msvc` runs here. The
  main crate cannot: a C dependency's build script wants MSVC's `lib.exe`.
  That is what makes it reasonable to write Windows-only FFI away from the
  machine that runs it.
- Nothing in CI builds them, and `biome.jsonc` skips `spikes/**/target`.

| Crate | Question | Issue |
|---|---|---|
| [`p0c-audio`](p0c-audio/README.md) | Can WASAPI process loopback isolate one application's audio, and which PID is the root? | #7 |
| `p0c-video` | Does WGC reach a fragmented MP4, and does a killed file play? | #8 |

## What a spike is not

Not a prototype of the thing it is testing. `p0c-audio` writes float PCM into a
RIFF file and stops: no encoder, no container, no device switching, no error
recovery. Everything it leaves out is something `recorder/own/` will have to
do, and doing any of it here would make a failure ambiguous between "the API
cannot do this" and "the spike is wrong", which is the one distinction the
exercise exists to keep clean.

## Running one

From the box. `rust-toolchain.toml` here pins the same compiler as
`src-tauri/`, because rustup only looks in the working directory and its
parents and `src-tauri/` is neither; bump the two together.

```powershell
cd spikes\p0c-audio
cargo run --release -- --help
```

A spike with its own run guide says so in the table above; `p0c-audio`'s is
the procedure for #7.

Checking one from anywhere (no MSVC linker needed, because `check` does not
link):

```bash
rustup target add x86_64-pc-windows-msvc
cd spikes/p0c-audio
cargo check --target x86_64-pc-windows-msvc
cargo clippy --target x86_64-pc-windows-msvc -- -D warnings
```
