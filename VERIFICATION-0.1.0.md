# TrayKit verification

## Canonical artifact

- Executable: `target/release/traykit.exe`
- Version: 0.1.0; Windows x64 GUI subsystem; 2,798,592 bytes.
- SHA-256: `43e094ef8cd891597034e2d2df9eed6a267587557a60edff1babb2fa1f485285`
- Source/lockfile/build-script hashes: `target/verification/resources.json`. No commit was made; the original scaffold had no git remote or tracked files.
- WindUI: published `windui = 0.33.0`, default features disabled, unmodified. Upstream `examples/tray.rs`, `hotkey.rs`, `resident.rs`, `background_task.rs`, and `settings.rs` were inspected.

## Static and build verification — passed on canonical source

`cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, and `cargo build --release --locked` all passed. Seven unit tests cover shortcut parsing/rejection, config defaults/roundtrip/duplicate/unknown keys, memory representation fallback, bounds and malformed images, bottom-up DIB row order, RGB padding, alpha and white-background composition. Logs are under `target/verification/final/`.

The resource verifier parses the PE with pefile and compares SHA-256 of every embedded RT_ICON payload against the corresponding payload in `assets/traykit.ico`. All nine sizes match: 16, 20, 24, 32, 40, 48, 64, 128, 256. The GUI subsystem, x64 architecture, version resource, asInvoker manifest, and PerMonitorV2 DPI declaration were verified. The window's large icon was also read back from a live HWND.

## Runtime verification — passed on canonical binary

Evidence: `target/verification/runtime-20261117-191233/result.json`, `stderr.log`, and `settings.png`.

Twelve checks passed, including:

- Resident startup creates no settings window and exposes no visible application window.
- A second instance exits cleanly.
- Menu and individual-tool shortcuts are held by the OS.
- Synthesized Ctrl+Alt+Q input opens the expected tool registry menu.
- Shell_NotifyIconGetRect confirms the tray icon is registered in Explorer.
- Injected Shell left/right callback messages open the distinct tools/basic menus. This is callback verification, not a physical tray mouse test.
- A real native menu click opens the WindUI settings window; screenshot reviewed.
- Reserving the proposed new shortcut causes Save to reject it while preserving the previous OS binding and config file.
- Editing the WindUI input, enabling the middle switch, and clicking Save atomically persists the config, registers Ctrl+Alt+W, releases Ctrl+Alt+Q, and successfully installs the middle hook.
- Closing settings destroys that window while the resident process continues.
- Exit from the menu ends the process and releases both configured shortcuts. No TrayKit test process remains running.

One two-second idle sample after closing settings recorded 0 ms additional process CPU, 15,720,448 private bytes and 37,699,584 working-set bytes. These are local samples, not performance guarantees.

## Clipboard verification — evidence tiers

The canonical binary passed six live image cases in `target/verification/clipboard-20261117-192159/result.json`:

1. PNG memory takes precedence over inaccessible FileDrop.
2. DIB memory input (white-background alpha composition expected).
3. DIBV5 memory input with alpha preserved.
4. A valid PNG is not blocked by an oversized FileDrop list.
5. A malformed PNG falls back to readable FileDrop.
6. Unicode FileDrop paths and fallback to the next readable image.

Every successful case verifies PNG/DIB/DIBV5 publication, removal of CF_HDROP, and every PNG output pixel. Windows may synthesize additional bitmap formats. The 128 MiB input and 32-megapixel limits apply before commit. Production conversion code has no image/file write path; the test fixtures themselves are created by the test script.

An earlier binary (`133dd0849d9691f70b0f29e1d9b1f3a2ac5562ff3081e2b4b6dd295966f280e3`) passed all ten image/negative/no-file-mutation cases with full original-format hash restoration in `target/verification/clipboard-20261117-185009/result.json`. That is historical evidence and is not relabeled as the canonical binary's full suite result.

### Current live blocker

On the canonical-binary negative-case rerun, the test owned CF_HDROP before invoking TrayKit, but afterward the clipboard owner was PID 229184, verified as the current session's `C:\Windows\System32\rdpclip.exe`, and the formats were replaced with RDP registered formats. The trace includes owner PID, sequence, input payload hash, output formats, stdout, and stderr. Another round saw an empty clipboard. These runs do not establish repeatable final negative-case behavior in the active redirected clipboard environment. They remain **live-blocked**, not passed or silently retried to a green result.

An attempted separate window station was rejected by Windows with Access Denied before clipboard mutation. A temporary pause of the current session's rdpclip prevented backup of its delayed-rendered formats; no conversion fixtures were written by that attempt. rdpclip was resumed successfully, and its threads were checked for residual Suspended state (none). Evidence: `target/verification/controlled-clipboard.json` and `rdpclip-restored-threads.json`. The unavailable isolation/pause path was removed from maintained scripts and archived under target/verification.

A controlled local session without an actively rewriting clipboard is required to finish canonical negative-case/no-file-change regression. An actual Win+Shift+S → WeType → conversion → remote Ctrl+V test is still required for RDP compatibility. No claim of cross-session success is made.

## Test-harness incident and correction

During instrumentation, a temporary version of the smoke script initialized an empty baseline before Capture finished. When Capture failed on registered format 49341, its finally block restored that empty list, clearing the interactive clipboard and incorrectly marking restoration true. The unreadable original payload could not be recovered. This is a test-harness defect, not an application conversion result, and must not be hidden behind earlier successful restoration records.

The maintained script now sets `snapshotReady` only after Capture returns completely. A backup failure cannot call Put/EmptyClipboard. `-SimulateBackupFailure` exercises the pre-commit abort path. `target/verification/backup-abort.json` records unchanged clipboard sequence 144808 before/after injection. Later backup failures correctly abort without restoration or clipboard writes. The most recent completed image test restored its own captured baseline; this does not retroactively restore the lost incident payload.

## Remaining manual/live checks

- Physical middle-button release opens the menu at the pointer. Hook installation is verified; hardware input and per-application pass-through remain untested.
- Final negative-case regression in a controlled clipboard session, including no image, inaccessible paths, and clipboard contention.
- Cross-session WeType/RDP paste behavior and receiving applications (e.g. chat editors, Paint, Office).
- Concurrent clipboard replacement and rare total/partial Windows SetClipboardData failure injection are not live-tested. The sequence comparison and buffer allocation boundaries are statically inspected; Windows offers no atomic multi-format clipboard transaction.

## Reproduce

```powershell
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
.\scripts\runtime-smoke.ps1
.\scripts\clipboard-smoke.ps1
uv run --with pefile python scripts/verify-resources.py
```

Runtime tests use TRAYKIT_CONFIG under target/verification, so the user's application config is not changed. Clipboard tests require a fully readable baseline and arm rollback only after its capture completes. They deliberately fail if the environment prevents reliable validation. Test output and fixture directories are ignored by git.

## Primary references

- https://github.com/huanfeng/wind-ui-rust (official source and examples; published source also inspected locally)
- clipboard-win 5.4.3 source in the local Cargo registry (FileDrop/std feature, locks and Bitmap reading)
- arboard 3.6.1 Windows source (reviewed for PNG/DIBV5 and Office-compatible positive-height orientation; not included as a dependency)
- https://learn.microsoft.com/en-us/windows/win32/winstation/window-stations (each window station owns its clipboard)
- https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/ns-processthreadsapi-startupinfow (window station/desktop startup semantics)
