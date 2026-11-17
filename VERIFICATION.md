# TrayKit 0.3.3 verification

## Canonical artifact (0.3.3)

- Executable: `target/release/traykit.exe`.
- Version: 0.3.3; Windows x64 GUI subsystem; 2,803,712 bytes.
- SHA-256: `5d1c41040fb09eb5db4f16fdd97cc8c45a7bd5d020777c481052ff10bf77dcf7`.
- Final evidence: `target/verification/0.3.3/` and `target/verification/runtime-20261117-231319/`.
- Source provenance includes Rust/Cargo/build/resource inputs and the runtime harness (`runtime-smoke.ps1`, `NativeProbe.cs`). The repository still has no tracked baseline or commit, so an empty `git diff` is not provenance evidence.
- 0.3.2 and earlier artifacts remain historical and are not relabeled as 0.3.3 evidence.

## Settings tabs and tool sidebar

0.3.3 replaces the single scrolling settings form with WindUI-native navigation patterns:

- `Element::tabs_pill` separates `常规` from `工具`, following the WindUI 0.33.0 two-to-four-option tabs example.
- The tools tab uses `Element::list_pill` as a scrollable registry-backed sidebar. `Signal<usize>` and pure `visible_when` conditions show exactly one matching detail page.
- The left list and right pages are generated from the same `tools::registered()` snapshot and index order, preventing label/form drift.
- Each detail page retains its existing enable signal and `HotkeyCapture`; switching pages does not discard unsaved values.
- Status, `保存并应用全部设置`, and `隐藏到托盘` stay outside the tabs. Save still applies every signal through the existing staged registration/persistence transaction, regardless of the selected tab or tool.
- At 620 x 600 logical pixels and 150% DPI, the general page and the RDP detail page render without overlap; title, description, switch, hotkey, help text, status, and footer are all visible. The sidebar independently scrolls when the registry grows.

## Final gates (0.3.3)

- `cargo fmt --all -- --check`, locked metadata, strict Clippy, and fat-LTO release build passed.
- 26/26 unit tests passed, 0 failed.
- PE FileVersion/ProductVersion/fixed version, Cargo package, and CLI version agree on 0.3.3.
- Runtime acceptance on the final SHA-256: 15 passed, 0 failed, 1 explicitly skipped. The skipped clipboard-unavailable assertion did not meet its precondition because `clipboard-image` was available; it did not execute or mutate the clipboard and was not counted as passed.
- Physical GUI coverage selected the tools tab and RDP sidebar item, captured Ctrl+Alt+F10 and Ctrl+Alt+F9, disabled/re-enabled the tool, verified tray menu filtering, and proved old/new OS hotkey release and acquisition.
- Common settings still captured Ctrl+Alt+W, rejected a reserved conflict without persistence, then applied the same chord plus middle-click atomically.
- Visual receipts: `settings.png`, `tools-tab.png`, and `tools.png` in `target/verification/runtime-20261117-231319/`.
- The native menu probe now waits 100 ms before reading an item rectangle and clicking. This corrects a harness race where an enumerable but not yet stable popup returned command 0; no product failure was hidden.
- No TrayKit process remained after the final run.

## imini deployment (0.3.3)

The canonical 0.3.3 executable was deployed to `C:/Users/inter/Desktop/CD/traykit.exe` after a hash-gated replacement:

- Pre-deployment canonical was the known 0.3.1 hash `0721003716cf49994bf538c3e94fc632bfa71fe1ef61b45a2f318b673a3b1a80`; no TrayKit process was running.
- The old executable was preserved at `C:/Users/inter/Desktop/CD/versions/0.3.1/traykit.exe` and its hash was independently re-read.
- The uploaded candidate was checked for size, ProductVersion, and SHA-256 before same-directory replacement.
- Post-deployment canonical SHA-256 is `5d1c41040fb09eb5db4f16fdd97cc8c45a7bd5d020777c481052ff10bf77dcf7`; FileVersion/ProductVersion are 0.3.3 and CLI readback is `TrayKit 0.3.3`.
- The upload temporary file was removed, no TrayKit process remained, and the application was not auto-started.
- Local deployment receipt: `target/verification/0.3.3/deployment-imini.json` (release evidence, ignored with `target/`).

## Historical canonical artifact (0.3.2 hotkey capture)

- Executable SHA-256: `a02a1404273fe40fbf7398d03a9e1cf313d64c9bc3f1dbd3bdc9373fcdbc468b`; 2,759,680 bytes.
- Evidence: `target/verification/0.3.2/` and `target/verification/runtime-20261117-224715/`.
- The following physical-hotkey section preserves the 0.3.2 implementation provenance.

## Physical hotkey capture

0.3.2 replaces editable hotkey text boxes with a focusable WindUI widget that consumes physical key events. A modifier-only event waits for the main key; the subsequent `WM_KEYDOWN` is normalized through the same `Shortcut` representation used by config validation and OS registration. The translated `WM_CHAR` is ignored, so IME/text injection cannot overwrite the recorded chord.

- Menu and per-tool fields accept Ctrl/Alt/Shift/Win plus the existing supported main keys.
- Escape cancels without changing the value.
- Unmodified Backspace/Delete clears optional tool bindings; the menu binding stays non-empty.
- Tab remains focus navigation.
- Saving still uses the existing staged registration, persistence, menu update, and rollback transaction.

## Final gates (0.3.2)

- `cargo fmt --all -- --check`, locked metadata, and strict Clippy passed.
- 26/26 unit tests passed, including physical-key normalization, modifier-only handling, F24, clear/reject behavior, duplicate `WM_CHAR` suppression, and parser/display roundtrip.
- PE FileVersion/ProductVersion/fixed version, Cargo package, and CLI version agree on 0.3.2.
- Runtime acceptance: 15 passed, 0 failed, 1 explicitly skipped. The skipped clipboard-unavailable assertion did not meet its precondition because `clipboard-image` was available; the corrected harness did not execute or mutate the clipboard and did not count the item as passed.
- Real `SendInput` chords, not Unicode shortcut strings, recorded Ctrl+Alt+W, Ctrl+Alt+F10, and Ctrl+Alt+F9. Conflict rollback, successful menu-hotkey rebind, tool disable/reenable, old registration release, and new registration acquisition all passed.
- Two harness defects were corrected before final acceptance: an available image action was previously misclassified as an unavailable no-write probe, and immediate `Get-Content` polling could hold a non-delete-sharing handle across the app's atomic config replace.
- No TrayKit process remained after the final run.

## Historical canonical artifact (0.3.1 OLE fix)

- Executable SHA-256: `0721003716cf49994bf538c3e94fc632bfa71fe1ef61b45a2f318b673a3b1a80`; 2,835,456 bytes.
- Evidence: `target/verification/0.3.1/` and `target/verification/runtime-20261117-222113/`.
- The following sections preserve the 0.3.1 OLE diagnosis and deployment provenance.

## Diagnosed fault

The imini interactive-session probe captured the failing clipboard at sequence 15921. It enumerated `FileGroupDescriptorW` (49389) and `FileContents` (49388), but no `CF_BITMAP`, `CF_DIB`, `CF_DIBV5`, `CF_HDROP`, or registered `PNG`. Evidence: `target/verification/0.3.1/imini-clipboard-formats.json`.

This is an OLE virtual file, not an in-memory bitmap or a filesystem path. File Explorer can consume the descriptor plus indexed contents and create a file; TrayKit 0.3.0 only probed bitmap formats and `CF_HDROP`, so its unavailable result was consistent with its incomplete input model.

The implementation follows Microsoft's Shell clipboard contract:

- <https://learn.microsoft.com/en-us/windows/win32/shell/clipboard>
- <https://learn.microsoft.com/en-us/windows/win32/shell/datascenarios>
- <https://learn.microsoft.com/en-us/windows/win32/api/objidl/nf-objidl-idataobject-getdata>
- <https://learn.microsoft.com/en-us/windows/win32/api/ole2/nf-ole2-olegetclipboard>

## Implemented fix

0.3.1 adds a distinct `virtual-file` input kind rather than mislabeling OLE data as `FileDrop`:

- Read-only availability requires both `FileGroupDescriptorW` and `FileContents`.
- Action execution releases the ordinary Win32 clipboard lock before entering OLE.
- `OleInitialize`/`OleUninitialize` are paired on the actual action worker or CLI thread.
- `FILEGROUPDESCRIPTORW` is parsed with checked arithmetic, at most 256 entries, unaligned-safe reads, UTF-16 filenames, and optional `FD_FILESIZE` handling.
- The first supported image descriptor supplies the exact `FORMATETC.lindex` for `FileContents`.
- `TYMED_ISTREAM` and `TYMED_HGLOBAL` are accepted. Every successful `GetData` is immediately owned by an RAII guard that calls `ReleaseStgMedium` on all returns.
- Virtual contents stay in memory. No temporary or destination file is created.
- Encoded input remains capped at 128 MiB. Existing 32-megapixel and 16,384-pixel dimension limits remain.
- Clipboard sequence is compared around the delayed OLE read and again before the existing single write-back, so a provider/user change cancels conversion rather than overwriting newer content.

Tray menu, direct hotkey, settings, and CLI still resolve the same action ID and `Tool::run`. The tray route runs it on the named `traykit-tool` worker, which is also where OLE is initialized.

## Final gates

The following passed on the final source and final SHA-256:

```powershell
cargo fmt --all -- --check
cargo metadata --locked --format-version 1
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
pwsh -NoProfile -File scripts/runtime-smoke.ps1 -Binary target/release/traykit.exe
python scripts/verify-resources.py
```

Results:

- 23 tests passed, 0 failed. New coverage includes packed descriptor parsing/selection/oversize/truncation and a real Windows `IStream` created by `SHCreateMemStream` with exact-length, unknown-length, and premature-EOF cases.
- Strict Clippy passed with `-D warnings`.
- Runtime GUI/OS acceptance passed 16/16. Receipt: `target/verification/0.3.1/runtime-smoke.json`, bound to final hash `072100...a80`.
- PE/resource gate passed: Cargo, CLI, FileVersion, and ProductVersion are 0.3.1; fixed version is 0.3.1.0; x64 GUI subsystem, manifest, and nine icon payloads match. Receipt: `target/verification/0.3.1/resources.json`.
- `actions list --json` exposes `clipboard-image.accepted_inputs = ["image", "file-drop", "virtual-file"]`.

## OLE roundtrip evidence

A controlled interactive-session provider on imini published a real COM `IDataObject` containing one indexed `FileGroupDescriptorW`/`FileContents` PNG. The final binary hash was verified remotely before execution.

Final-hash result:

- Candidate SHA-256: `0721003716cf49994bf538c3e94fc632bfa71fe1ef61b45a2f318b673a3b1a80`.
- Input: 123-byte 3 x 2 PNG exposed only as OLE virtual file data.
- CLI exit: 0; source reported as `VirtualFile（controlled.png）`.
- Sequence: 16062 before provider, 16067 after provider publication, 16072 after conversion.
- Readback: PNG plus DIB/DIBV5-compatible formats; `Clipboard.GetImage()` returned 3 x 2.
- Receipt: `target/verification/0.3.1/final-0721-controlled-result.json`.

This is a native OLE protocol roundtrip through the real final EXE, but the provider is a controlled fixture. It validates the protocol implementation; it is not claimed as native equivalence to the user's original screenshot application.

## Evidence boundaries

Verified:

- The failing clipboard's format topology was captured on imini without reading or logging clipboard body data.
- The observed topology is now accepted by the action model.
- Descriptor parsing, `lindex`, HGLOBAL and real COM IStream reads, bounded decoding, output publication, and image readback are verified.
- Existing GUI/hotkey/config transaction behavior remains 16/16 passing.

Live-blocked:

- A fresh screenshot from the original application through the tray action. Before the final candidate reached imini, the clipboard had changed to a single `CF_HDROP`; the guarded source-app probe exited 40 without running or changing the clipboard. Receipt: `target/verification/0.3.1/candidate-675e-source-precondition.json`.
- Cross-RDP/WeType paste and physical middle-button behavior remain separate field checks; the controlled OLE fixture does not substitute for them.

When the original application produces another screenshot, the remaining acceptance probe is: confirm the tray item is enabled, invoke it through tray/hotkey, then paste into the target application and verify the resulting image. Until that is observed, the defensible claim is “the observed virtual-file protocol is supported and a controlled native OLE roundtrip passed,” not “all screenshot applications are confirmed.”

## Harness corrections and audit notes

- A first CLI contract assertion treated the JSON root object as the action array. Raw output proved `virtual-file` was present; the assertion was corrected to inspect `.actions`.
- A first GUI rerun used Windows PowerShell 5.1 on a UTF-8 no-BOM script and failed during parsing. No app started; the final run used `pwsh` and passed 16/16.
- A first receipt summary treated the runtime root object as a case and falsely reported one failure. The corrected audit reads `.results`: 16 cases, 0 failed. No warning was waived.
- Adding a test-only real-IStream case changed the release hash despite unchanged size. All final-hash gates and the OLE roundtrip were rerun; receipts from the earlier `675e...d6c6` candidate are retained with explicit candidate filenames and are not final evidence.
- `git status` shows the whole repository as untracked; therefore `git diff` is empty and is not used to assert no changes.

## imini deployment

The canonical remote file was hash-gated against the known 0.3.0 baseline before replacement. Deployment receipt: `target/verification/0.3.1/deployment-imini.json`.

- Canonical path: `C:/Users/inter/Desktop/CD/traykit.exe`.
- Deployed SHA-256/version/size: `072100...a80`, 0.3.1, 2,835,456 bytes.
- Canonical CLI readback: `TrayKit 0.3.1`.
- Previous 0.3.0 was preserved at `C:/Users/inter/Desktop/CD/versions/0.3.0/traykit.exe` with original hash `032b54...372d`.
- No TrayKit process was running before replacement, so none was auto-started afterward.
- Remote temporary probe files and `TrayKit031*` scheduled tasks were removed; counts are zero.

## Reproduce

```powershell
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 | Out-Null
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/runtime-smoke.ps1 -Binary target/release/traykit.exe
python scripts/verify-resources.py
```
