# SingboxGUI-Rust

A lightweight graphical user interface for [sing-box](https://sing-box.sagernet.org/), written in Rust using [eframe/egui](https://github.com/emilk/egui).

## Features

- **Start / Stop** sing-box process directly from the UI
- **Real‑time logging** with timestamps
- **Live log filter** (simple substring search)
- **Configurable sing‑box path** with on‑demand verification
- **Persistent log buffer** (ring buffer, last 1000 lines)
- **Background I/O worker** — UI stays responsive while reading process output
- **Event‑driven repaint** — low CPU idle usage
- **Unix process group kill** — clean termination of sing‑box and its children

## Prerequisites

- Rust ≥ 1.70 (stable)
- [sing‑box](https://sing-box.sagernet.org/guide/getting-started/installation/) installed and in your `PATH`, or specify the absolute path in Settings.

## Build

```bash
cargo build --release
```

The binary will be at `target/release/singboxgui-rust`.

## Run

```bash
cargo run --release
```

or run the built binary directly.

## Usage

1. **Start** — launches `sing-box run` and begins streaming stdout/stderr into the log window.
2. **Stop** — sends a termination request (graceful SIGTERM on Unix, `kill` on Windows).
3. **Settings** — change the sing-box executable path; use **Check** to verify; **Apply** to save.
4. **Auto‑scroll** — when enabled, scrolls the log pane to the bottom automatically on new lines.
5. **Filter** — type a substring to restrict displayed log lines.
6. **Clear Logs** — removes all lines from the current buffer.

## Architecture

### Background worker

All interactions with the child process happen on a dedicated background thread. The UI thread communicates via two `std::sync::mpsc` channels:

```
UI  →  Command channel (main thread → worker)   // Start / Stop
Worker  →  Event channel (worker → main thread) // Log, ProcessStarted, ProcessExited
```

This design prevents any blocking I/O from freezing the UI.

### Event‑driven repaint

The app requests a repaint only when it receives an event from the worker (e.g., a new log line or a process state change). In between events the UI does not spin, resulting in near‑zero CPU usage when idle.

### Ring buffer

Log lines are stored in a `VecDeque<String>` with a maximum capacity of 1000 entries. New lines are `push_back`‑ed; when the capacity is exceeded the oldest entry is `pop_front`‑ed. This avoids the reallocation cost of `Vec::remove(0)`.

### Graceful shutdown

When the main window is closed, the `Drop` implementation closes the command channel and joins the worker thread, guaranteeing that all child processes are reaped before exit.

## Project Structure

```
.
├── Cargo.toml          # Project manifest
├── src
│   └── main.rs         # Application entry point, eframe App impl, background worker
└── .cargo              # Optional: cargo config (mirrors, git-fetch-with-cli)
```

## Troubleshooting

**“Failed to verify sing‑box …”**  
Make sure `sing-box` is in your `PATH`, or use **Settings** to point to the full path (e.g. `/usr/local/bin/sing-box`).

**Build fails with “could not connect to server”**  
If you are behind a proxy, set `http_proxy` / `https_proxy` in the environment, or configure `[net] git-fetch-with-cli = true` in `$CARGO_HOME/config.toml`. For a faster mirror, edit the `[source.crates-io]` entry.

**Process does not terminate**  
On Unix the GUI sends `SIGTERM` to the whole process group, which usually terminates `sing-box` and any helpers. If needed, check your system’s process tree.

## License

MIT OR Apache-2.0
