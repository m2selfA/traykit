[CmdletBinding()]
param([string]$Binary = "$PSScriptRoot/../target/release/traykit.exe", [switch]$SimulateBackupFailure)
$ErrorActionPreference = 'Stop'
Add-Type -Path "$PSScriptRoot/NativeProbe.cs"
Add-Type -AssemblyName System.Drawing
$Binary = (Resolve-Path $Binary).Path
$work = Join-Path $PSScriptRoot ('../target/verification/clipboard-' + (Get-Date -Format yyyyMMdd-HHmmss))
New-Item -ItemType Directory -Path $work -Force | Out-Null
$work = (Resolve-Path $work).Path
$fixture = Join-Path $work '图片-fixture.png'
Copy-Item "$PSScriptRoot/../assets/traykit.png" $fixture
$pngBytes = [IO.File]::ReadAllBytes($fixture)
$pngFormat = [NativeProbe]::RegisterClipboardFormat('PNG')
$owner = [NativeProbe]::Owner()
if ($owner -eq [IntPtr]::Zero) { throw 'Cannot create verification owner' }
$baseline = $null
$snapshotReady = $false
$restored = $false
$results = New-Object 'Collections.Generic.List[object]'
$calls = New-Object 'Collections.Generic.List[object]'
function Convert-Once([int]$ExpectedExit = 0) {
    $inputFormats = [NativeProbe]::Formats($owner)
    $inputSequence = [NativeProbe]::GetClipboardSequenceNumber()
    $inputOwner = [NativeProbe]::ClipboardOwnerProcess()
    $inputDrop = if ([uint32]15 -in $inputFormats) { [NativeProbe]::Hash([NativeProbe]::Read(15,$owner)) } else { $null }
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $Binary
    $start.Arguments = '--convert'
    $start.WorkingDirectory = $work
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($start)
    if (-not $process.WaitForExit(15000)) { $process.Kill(); $process.WaitForExit(); throw 'Conversion timed out' }
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $calls.Add([pscustomobject]@{index=$calls.Count+1; input_formats=$inputFormats; input_sequence=$inputSequence; input_owner=$inputOwner; input_filedrop_hash=$inputDrop; output_owner=[NativeProbe]::ClipboardOwnerProcess(); exit=$process.ExitCode; expected_exit=$ExpectedExit; stdout=$stdout.Trim(); stderr=$stderr.Trim(); output_formats=[NativeProbe]::Formats($owner)})
    if ($process.ExitCode -ne $ExpectedExit) { throw "Unexpected exit $($process.ExitCode); expected $ExpectedExit; $stdout $stderr" }
    return [pscustomobject]@{exit=$process.ExitCode; stdout=$stdout.Trim(); stderr=$stderr.Trim()}
}
function Assert-Image([string]$Name, [bool]$FlattenWhite = $false) {
    $formats = [NativeProbe]::Formats($owner)
    foreach ($format in @([uint32]8,[uint32]17,$pngFormat)) { if ($format -notin $formats) { throw "Missing output format $format" } }
    if ([uint32]15 -in $formats) { throw 'FileDrop still present after conversion' }
    $output = [NativeProbe]::Read($pngFormat,$owner)
    $sourceStream = [IO.MemoryStream]::new($pngBytes)
    $outputStream = [IO.MemoryStream]::new($output)
    $source = [Drawing.Bitmap]::FromStream($sourceStream)
    $converted = [Drawing.Bitmap]::FromStream($outputStream)
    try {
        if ($source.Size -ne $converted.Size) { throw 'Dimension mismatch' }
        for ($y=0; $y -lt $source.Height; $y++) { for($x=0; $x -lt $source.Width; $x++) {
            $expected = $source.GetPixel($x,$y)
            if ($FlattenWhite) {
                $a = [int]$expected.A
                $rgb = @($expected.R,$expected.G,$expected.B) | ForEach-Object { [int][Math]::Floor(($_ * $a + 255 * (255-$a) + 127) / 255) }
                $expected = [Drawing.Color]::FromArgb(255,$rgb[0],$rgb[1],$rgb[2])
            }
            if($expected.ToArgb() -ne $converted.GetPixel($x,$y).ToArgb()) { throw "Pixel mismatch $x,$y in $Name; expected $expected; actual $($converted.GetPixel($x,$y))" }
        } }
    } finally { $source.Dispose(); $converted.Dispose(); $sourceStream.Dispose(); $outputStream.Dispose() }
    $results.Add([pscustomobject]@{case=$Name; passed=$true; formats=$formats; pixel_comparison='all pixels identical'})
}
try {
    # Refuse unsupported baseline formats before making ANY clipboard changes.
    # Capture must finish before arming rollback. A partial/failed capture may NEVER clear the clipboard.
    $captured = [NativeProbe]::Capture($owner)
    if ($SimulateBackupFailure) { throw "Simulated backup failure before snapshotReady" }
    $baseline = [Collections.Generic.List[ClipBlob]]::new()
    foreach ($blob in $captured) { $baseline.Add($blob) }
    $snapshotReady = $true
    $beforeFiles = @(Get-ChildItem $work -File | ForEach-Object { $_.Name + ':' + (Get-FileHash $_.FullName -Algorithm SHA256).Hash })
    [NativeProbe]::Put($owner,@([ClipBlob]::new($pngFormat,$pngBytes),[ClipBlob]::new(15,[NativeProbe]::FileDrop(@('Z:\inaccessible-session\missing.png')))))
    $result = Convert-Once
    Assert-Image 'memory PNG preferred over inaccessible FileDrop'
    $dib = [NativeProbe]::Read(8,$owner)
    $dibv5 = [NativeProbe]::Read(17,$owner)
    foreach ($memoryCase in @(@(8,$dib,'DIB memory input'),@(17,$dibv5,'DIBV5 memory input'))) {
        [NativeProbe]::Put($owner,@([ClipBlob]::new($memoryCase[0],$memoryCase[1])))
        $result = Convert-Once
        Assert-Image $memoryCase[2] ($memoryCase[0] -eq 8)
    }
    $oversized = [NativeProbe]::FileDrop([string[]](1..257 | ForEach-Object { 'Z:\missing\' + $_ + '.png' }))
    [NativeProbe]::Put($owner,@([ClipBlob]::new($pngFormat,$pngBytes),[ClipBlob]::new(15,$oversized)))
    $result = Convert-Once
    Assert-Image 'valid PNG is not blocked by oversized FileDrop list'
    [NativeProbe]::Put($owner,@([ClipBlob]::new($pngFormat,[byte[]](1,2,3)),[ClipBlob]::new(15,[NativeProbe]::FileDrop(@($fixture)))))
    $result = Convert-Once
    Assert-Image 'invalid memory PNG falls back to readable FileDrop'
    [NativeProbe]::Put($owner,@([ClipBlob]::new(15,[NativeProbe]::FileDrop(@('Z:\inaccessible-session\missing.png',$fixture)))))
    $result = Convert-Once
    Assert-Image 'FileDrop Unicode path and fallback to next readable file'
    $invalid = [NativeProbe]::FileDrop(@('Z:\inaccessible-session\missing.png'))
    [NativeProbe]::Put($owner,@([ClipBlob]::new(15,$invalid)))
    $result = Convert-Once 1
    if ([NativeProbe]::Hash([NativeProbe]::Read(15,$owner)) -ne [NativeProbe]::Hash($invalid)) { throw 'Failed read modified original FileDrop' }
    $results.Add([pscustomobject]@{case='inaccessible FileDrop fails without mutation'; passed=$true})
    $text = [Text.Encoding]::Unicode.GetBytes("TrayKit no-image fixture`0")
    [NativeProbe]::Put($owner,@([ClipBlob]::new(13,$text)))
    $result = Convert-Once 1
    if ([NativeProbe]::Hash([NativeProbe]::Read(13,$owner)) -ne [NativeProbe]::Hash($text)) { throw 'No-image failure modified text' }
    $results.Add([pscustomobject]@{case='no-image preserves text'; passed=$true})
    if (-not [NativeProbe]::OpenClipboard($owner)) { throw 'Cannot hold clipboard for contention test' }
    try { $result = Convert-Once 1 } finally { [void][NativeProbe]::CloseClipboard() }
    if ([NativeProbe]::Hash([NativeProbe]::Read(13,$owner)) -ne [NativeProbe]::Hash($text)) { throw 'Busy clipboard test modified text' }
    $results.Add([pscustomobject]@{case='busy clipboard returns bounded failure'; passed=$true})
    $afterFiles = @(Get-ChildItem $work -File | ForEach-Object { $_.Name + ':' + (Get-FileHash $_.FullName -Algorithm SHA256).Hash })
    if (Compare-Object $beforeFiles $afterFiles) { throw 'Conversion created or modified a fixture file' }
    $results.Add([pscustomobject]@{case='fixture directory file list and SHA256 unchanged'; passed=$true})
} finally {
    try {
        if ($snapshotReady) { [NativeProbe]::VerifyRestore($owner,$baseline); $restored=$true; Write-Output 'Original clipboard restored: all original format payload SHA256 hashes and format count match' }
    } finally {
        [void][NativeProbe]::DestroyWindow($owner)
        [pscustomobject]@{binary=$Binary; binary_sha256=(Get-FileHash $Binary -Algorithm SHA256).Hash; baseline_capture_complete=$snapshotReady; clipboard_restored=$restored; results=$results; calls=$calls} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $work 'result.json') -Encoding utf8
    }
}
$results | Format-Table case,passed
if (-not $restored) { throw 'Clipboard rollback was not verified' }
Write-Output ('Evidence: ' + (Join-Path $work 'result.json'))
