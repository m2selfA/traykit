# TrayKit

A small Windows tray toolbox written in Rust with [WindUI](https://github.com/huanfeng/wind-ui-rust). Version 0.3.3 provides context-aware clipboard actions through one shared registry used by the tray menu, global hotkeys and one-shot CLI commands. It does not install a service, keep clipboard history or monitor clipboard changes.

## Run

Windows 10/11, Rust MSVC toolchain, and Windows SDK/Visual Studio C++ build tools:

```powershell
cargo run --locked
cargo build --release --locked
.\target\release\traykit.exe
```

TrayKit starts in WindUI resident mode with one tray icon and no settings window. Left-click opens the tools menu at the pointer. Right-click offers Settings and Exit. Settings is created on demand; closing it frees that window while the tray keeps running. A second instance in the same Windows session exits quietly.

- **Ctrl+Alt+Q** opens the menu by default.
- Enabled actions that cannot accept the current clipboard remain visible but are disabled. The probe checks format availability only; it does not copy or write clipboard data.
- A direct action hotkey rechecks availability and reports the reason instead of executing an incompatible action.
- Settings uses WindUI tabs to separate general and tool configuration. The tools tab uses a scrollable sidebar generated from the shared registry and shows one tool detail page at a time, so adding tools does not lengthen the settings form.
- Settings records global shortcuts from the physical key chord: focus a hotkey field and press modifiers plus the main key. It does not accept typed shortcut text. Unmodified Backspace/Delete clears an optional tool binding; Escape keeps the current value.
- A setting-disabled action is hidden, releases its OS hotkey and rejects residual requests. Its shortcut text is retained for later re-enabling.
- All hotkey changes are staged together. Conflicts or persistence errors preserve the previous config, registrations, menu and middle-click state. Swapping two existing shortcuts is supported.
- Middle-click is off by default. When enabled, releasing the physical middle button opens the menu and passes the click through. Injected clicks are ignored.
- Actions run on demand in the existing single worker. Repeated requests while busy are rejected. Exit asks the user to retry after the current action finishes.

## CLI

The CLI invokes the same action IDs and execution path as the GUI without starting a resident process:

```powershell
.\target\release\traykit.exe actions list --json
.\target\release\traykit.exe run text-clean
.\target\release\traykit.exe doctor --json
.\target\release\traykit.exe --convert   # compatibility alias for run clipboard-image
```

`actions list --json` reports each descriptor, configured enablement and current availability. `doctor --json` validates config and reads a clipboard format/sequence summary without opening or changing the clipboard. It does not prove that a global hotkey is free or perform an action roundtrip.

Exit codes are stable for automation: 0 success, 1 execution/diagnostic failure, 2 invalid syntax or unknown action ID, and 3 disabled/unavailable/busy. JSON commands emit machine-readable errors on stdout; action execution errors are written to stderr. GUI-subsystem callers should use a waiting `ProcessStartInfo` and inspect the real exit code.

## Actions

| Action ID | Accepted input | Output | Behavior |
| --- | --- | --- | --- |
| `clipboard-image` | image, FileDrop, or OLE virtual file | image | Convert in-memory PNG/DIB/Bitmap, referenced image files, or indexed `FileGroupDescriptorW`/`FileContents` images into PNG + DIBV5 + DIB without creating files |
| `text-clean` | Unicode text | text | Trim spaces/tabs from line ends and normalize line endings to CRLF while preserving indentation and blank lines |
| `path-powershell` | Unicode text | text | Convert one path per line to a single-quoted PowerShell literal, doubling embedded apostrophes; never execute it |
| `path-rdp` | Unicode text | text | Toggle absolute drive paths and `\\tsclient\drive` paths; never connect to or validate a remote machine |

Text actions accept up to 4 MiB of UTF-16, transform in memory and compare the clipboard sequence before writing. An unchanged result leaves the clipboard untouched. Invalid multiline input rejects the complete action before mutation.

Image conversion snapshots registered PNG, DIBV5, DIB and Bitmap representations under the clipboard lock, then releases the lock before decoding. If none decodes, it tries readable image files from Unicode FileDrop in order. It also supports Shell/OLE virtual files: `FileGroupDescriptorW` is parsed in memory and the matching indexed `FileContents` is read through `IDataObject::GetData` as `IStream` or `HGLOBAL`. Virtual contents are never materialized as files. Limits are 256 descriptors, 128 MiB encoded input, 32 megapixels and 16384 pixels per side. All output allocations happen before `EmptyClipboard`; sequence checks around virtual reads and before write-back prevent overwriting content changed by another process.

The implementation never creates image or temporary files. It may read an existing local, network or redirected file named by FileDrop. Windows has no atomic multi-format clipboard transaction: a rare total failure after `EmptyClipboard` can still leave the clipboard empty. RDP clipboard redirection and receiving applications can modify formats asynchronously, so cross-session compatibility requires an end-to-end test in that environment.

## Configuration

Startup does not create a config file. Only a settings Save writes `%APPDATA%\TrayKit\config.toml`. `TRAYKIT_CONFIG` overrides the path for isolated tests.

```toml
menu_hotkey = "Ctrl+Alt+Q"
middle_click = false
disabled_tools = [] # e.g. ["text-clean", "path-rdp"]

[tool_hotkeys]
# clipboard-image = "Ctrl+Alt+F11"
# text-clean = "Ctrl+Alt+F10"
# path-powershell = "Ctrl+Alt+F9"
# path-rdp = "Ctrl+Alt+F8"
```

Use Ctrl/Alt/Shift/Win plus A-Z, 0-9, F1-F24, Space, Home, End, Insert or Delete. Duplicate active shortcuts and reserved/in-use combinations are rejected. Disabled bindings are dormant; enabling one validates conflicts again. Focus an optional tool hotkey and press unmodified Backspace/Delete to remove its binding; the menu hotkey cannot be empty. Unknown action IDs and malformed shortcuts are rejected. Existing 0.1.x/0.2.x configs remain compatible. File edits apply at next startup; settings changes apply immediately. No startup registry entries or services are created.

## Extend

Implement `tools::Tool` with a stable `descriptor()`, optional `probe(&ClipboardSummary)`, and `run() -> anyhow::Result<ActionResult>`, then add it to `tools::registered()`. `ToolDescriptor` declares ID, name, category, accepted inputs, output, side effects, and preview/cancel support. The menu, hotkey router, settings and CLI all consume this registry.

```text
src/
  main.rs                 lifecycle, worker dispatch and unified action execution
  cli.rs                  one-shot parser, JSON contracts and exit-code mapping
  clipboard.rs            format summary plus shared owner/lock/HGLOBAL buffers
  tray.rs                 native contextual menu and optional middle mouse hook
  hotkey.rs               shortcut parser and transactional OS registrations
  settings.rs             tab/sidebar navigation, physical hotkey capture and transactional apply
  config.rs               TOML config, validation and explicit save
  tools/
    mod.rs                descriptors, availability, result model and registry
    clipboard_image.rs    bounded image read/decode/publish
    text.rs               text/path transforms and Unicode clipboard adapter
```

Preview, cancellation, typed pipelines, external scripts and third-party plugins are not implemented in 0.3.3. All current descriptors therefore report `supports_preview = false` and `supports_cancel = false`.

## GitHub release automation

`.github/workflows/release-windows-x64.yml` runs only for pushed tags beginning with `v`. The job rejects a tag unless it exactly matches the Cargo package version (`vX.Y.Z`) and the runner host is `x86_64-pc-windows-msvc`. It runs formatting, strict Clippy, unit tests, release compilation, and PE/resource verification before producing:

- `traykit-X.Y.Z-windows-x64.exe`
- `traykit-X.Y.Z-windows-x64.zip`
- `SHA256SUMS.txt`

The files are uploaded as a 30-day Actions artifact and attached to a GitHub Release. Normal branch pushes and pull requests do not trigger this workflow. After bumping and committing all version sources, create the matching release tag:

```powershell
$Version = "0.3.4"
git tag -a "v$Version" -m "TrayKit $Version"
git push origin "v$Version"
```

## Verification

```powershell
cargo fmt --check
cargo metadata --locked --format-version 1
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
pwsh -NoProfile -File .\scripts\runtime-smoke.ps1 -Binary .\target\release\traykit.exe
python .\scripts\verify-resources.py
```

See `VERIFICATION.md` for canonical hashes, actual runtime evidence and remaining live-blocked checks. Clipboard live probes must preserve provenance and prove baseline restoration; local format checks do not substitute for RDP/WeType end-to-end testing.
