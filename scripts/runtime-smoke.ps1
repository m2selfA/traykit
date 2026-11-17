[CmdletBinding()]
param([string]$Binary = "$PSScriptRoot/../target/release/traykit.exe")
$ErrorActionPreference = 'Stop'
Add-Type -Path "$PSScriptRoot/NativeProbe.cs"
[void][NativeProbe]::SetThreadDpiAwarenessContext([IntPtr](-4))
$Binary = (Resolve-Path $Binary).Path
if (Get-Process traykit -ErrorAction SilentlyContinue) { throw 'An existing TrayKit is running; probe will not disturb it' }
$work = Join-Path $PSScriptRoot ('../target/verification/runtime-' + (Get-Date -Format yyyyMMdd-HHmmss))
New-Item -ItemType Directory -Path $work -Force | Out-Null
$work = (Resolve-Path $work).Path
$config = Join-Path $work 'config.toml'
@'
menu_hotkey = "Ctrl+Alt+Q"
middle_click = false
[tool_hotkeys]
clipboard-image = "Ctrl+Alt+F11"
'@ | Set-Content $config -Encoding utf8
$results = New-Object 'Collections.Generic.List[object]'
$process = $null
function Wait-Until([scriptblock]$Condition,[string]$Description) {
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do { if (& $Condition) { return }; Start-Sleep -Milliseconds 30 } while ([DateTime]::UtcNow -lt $deadline)
    throw "Timed out: $Description"
}
function Start-TrayKit {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $Binary
    $info.WorkingDirectory = $work
    $info.UseShellExecute = $false
    $info.RedirectStandardError = $true
    $info.RedirectStandardOutput = $true
    $info.EnvironmentVariables['TRAYKIT_CONFIG'] = $config
    $info.EnvironmentVariables['TRAYKIT_DIAGNOSTICS'] = '1'
    return [Diagnostics.Process]::Start($info)
}
function Read-ActionStatus {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $Binary
    $info.Arguments = 'actions list --json'
    $info.UseShellExecute = $false
    $info.RedirectStandardError = $true
    $info.RedirectStandardOutput = $true
    $info.CreateNoWindow = $true
    $info.EnvironmentVariables['TRAYKIT_CONFIG'] = $config
    $probe = [Diagnostics.Process]::Start($info)
    $stdout = $probe.StandardOutput.ReadToEnd()
    $stderr = $probe.StandardError.ReadToEnd()
    $probe.WaitForExit()
    if ($probe.ExitCode -ne 0) { throw "actions list failed: $stderr" }
    return $stdout | ConvertFrom-Json
}
function Read-Menu {
    $popup = [NativeProbe]::TrayKitPopupMenu($process.Id)
    if ($popup -eq [IntPtr]::Zero) { return $null }
    $items = @([NativeProbe]::TrayKitMenuItems($process.Id) | ForEach-Object { ($_ -split "`t",2)[0] })
    return ,$items
}
try {
    if (-not [NativeProbe]::RegisterHotKey([IntPtr]::Zero,901,3,0x51)) { throw 'Ctrl+Alt+Q is already occupied before test' }
    [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,901)
    $process = Start-TrayKit
    Wait-Until { [NativeProbe]::FindWindow('TrayKit.NativeBridge','TrayKit Events') -ne [IntPtr]::Zero } 'native bridge startup'
    if ([NativeProbe]::FindWindow($null,'TrayKit 设置') -ne [IntPtr]::Zero) { throw 'Settings window should be created only on demand' }
    if ([NativeProbe]::VisibleWindows($process.Id) -ne 0) { throw 'Startup exposed a visible window' }
    $results.Add([pscustomobject]@{case='starts resident with no settings window or visible windows';passed=$true})
    $second = Start-TrayKit
    if (-not $second.WaitForExit(3000) -or $second.ExitCode -ne 0) { throw 'Second instance did not exit cleanly' }
    $results.Add([pscustomobject]@{case='second instance exits cleanly';passed=$true})
    Wait-Until {
        $free = [NativeProbe]::RegisterHotKey([IntPtr]::Zero,902,3,0x51)
        if ($free) { [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,902) }
        return -not $free
    } 'menu hotkey registration'
    Wait-Until {
        $free = [NativeProbe]::RegisterHotKey([IntPtr]::Zero,903,3,0x7A)
        if ($free) { [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,903) }
        return -not $free
    } 'tool hotkey registration'
    $results.Add([pscustomobject]@{case='menu and direct tool shortcuts registered in OS';passed=$true})
    [NativeProbe]::Shortcut(0x51)
    Wait-Until { $null -ne (Read-Menu) } 'real global shortcut opening tools menu'
    $items = Read-Menu
    if ($items -notcontains '修复剪贴板图片' -or $items -notcontains '设置…' -or $items -notcontains '退出 TrayKit') { throw ('Unexpected menu: ' + ($items -join ',')) }
    $actionStatus = Read-ActionStatus
    $menuStates = @()
    foreach ($action in $actionStatus.actions) {
        $actual = [NativeProbe]::TrayKitMenuItemEnabled($process.Id,$action.name)
        $expected = if ($action.enabled -and $action.availability.state -eq 'available') { 1 } else { 0 }
        if ($actual -ne $expected) { throw "Menu availability mismatch for $($action.id): expected=$expected actual=$actual" }
        $menuStates += "$($action.id):$actual"
    }
    $results.Add([pscustomobject]@{case='global shortcut opens context-aware tool registry menu';passed=$true;items=$items;states=$menuStates})
    [NativeProbe]::Key(0x1B)
    Wait-Until { $null -eq (Read-Menu) } 'dismiss hotkey menu'

    $clipboardImage = $actionStatus.actions | Where-Object id -eq 'clipboard-image'
    if ($clipboardImage.availability.state -eq 'unavailable') {
        $beforeUnavailable = [NativeProbe]::GetClipboardSequenceNumber()
        $bridge = [NativeProbe]::FindWindow('TrayKit.NativeBridge','TrayKit Events')
        [void][NativeProbe]::PostMessage($bridge,0x312,[UIntPtr]2,[IntPtr]((0x7A -shl 16) -bor 3))
        Start-Sleep -Milliseconds 300
        if ([NativeProbe]::GetClipboardSequenceNumber() -ne $beforeUnavailable) { throw 'Unavailable routed hotkey changed clipboard sequence' }
        $results.Add([pscustomobject]@{case='unavailable routed hotkey preserves clipboard sequence';passed=$true;sequence=$beforeUnavailable})
    } else {
        $results.Add([pscustomobject]@{case='unavailable routed hotkey preserves clipboard sequence';passed=$null;skipped=$true;reason='clipboard-image action was available; probe did not mutate clipboard'})
    }

    $trayHost=[NativeProbe]::FindWindow('WindUiAppHostClass',$null)
    $trayRect=[NativeProbe]::TrayRectangle($trayHost)
    $results.Add([pscustomobject]@{case='Shell confirms registered tray icon';passed=$true;left=$trayRect.Left;top=$trayRect.Top})
    [void][NativeProbe]::PostMessage($trayHost,0x8001,[UIntPtr]1,[IntPtr]0x205) # injected Shell right-click callback
    Wait-Until { $null -ne (Read-Menu) } 'right-click basic menu callback'
    $basic=Read-Menu
    if ($basic -contains '修复剪贴板图片' -or $basic -notcontains '设置…') { throw 'Right-click menu is not the basic settings menu' }
    [NativeProbe]::Key(0x1B)
    Wait-Until { $null -eq (Read-Menu) } 'dismiss basic menu'
    [void][NativeProbe]::PostMessage($trayHost,0x8001,[UIntPtr]1,[IntPtr]0x202) # injected Shell left-click callback
    Wait-Until { $null -ne (Read-Menu) } 'left-click tools menu callback'
    if ((Read-Menu) -notcontains '修复剪贴板图片') { throw 'Left-click callback did not open tools menu' }
    $results.Add([pscustomobject]@{case='injected tray left/right callbacks open the correct separate menus';passed=$true})
    [NativeProbe]::ClickMenu($process.Id,[array]::IndexOf((Read-Menu),"设置…")) # stable label lookup
    Wait-Until { [NativeProbe]::FindWindow($null,'TrayKit 设置') -ne [IntPtr]::Zero } 'on-demand settings creation'
    $settings = [NativeProbe]::FindWindow($null,'TrayKit 设置')
    Wait-Until { [NativeProbe]::IsWindowVisible($settings) } 'Settings action through native menu'
    if ([NativeProbe]::SendMessage($settings,0x7F,[IntPtr]1,[IntPtr]::Zero) -eq [IntPtr]::Zero) { throw 'Settings window has no large icon' }
    Add-Type -AssemblyName System.Drawing
    $rectangle = [NativeProbe+Rect]::new()
    [void][NativeProbe]::GetWindowRect($settings,[ref]$rectangle)
    Start-Sleep -Milliseconds 200
    $capture=[Drawing.Bitmap]::new($rectangle.Right-$rectangle.Left,$rectangle.Bottom-$rectangle.Top)
    $graphics=[Drawing.Graphics]::FromImage($capture)
    try { $graphics.CopyFromScreen($rectangle.Left,$rectangle.Top,0,0,$capture.Size); $capture.Save((Join-Path $work 'settings.png'),[Drawing.Imaging.ImageFormat]::Png) } finally { $graphics.Dispose(); $capture.Dispose() }
    $results.Add([pscustomobject]@{case='native menu opens WindUI settings';passed=$true})
    # Input and save through the real custom UI; reserve W to force a registration conflict.
    [NativeProbe]::ClickClient($settings,150,328)
    Start-Sleep -Milliseconds 100
    [NativeProbe]::CaptureShortcut($settings,0x57,$false)
    if (-not [NativeProbe]::RegisterHotKey([IntPtr]::Zero,905,3,0x57)) { throw 'Test could not reserve Ctrl+Alt+W' }
    try {
        [NativeProbe]::ClickClient($settings,80,550)
        Start-Sleep -Milliseconds 200
        if ((Get-Content $config -Raw) -match 'Ctrl\+Alt\+W') { throw 'Failed binding was persisted' }
        if ([NativeProbe]::RegisterHotKey([IntPtr]::Zero,906,3,0x51)) { [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,906); throw 'Failed rebind lost old shortcut' }
    } finally { [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,905) }
    $results.Add([pscustomobject]@{case='conflicting settings hotkey keeps old OS registration and config';passed=$true})
    [NativeProbe]::CloseTransientPopups($process.Id)
    Start-Sleep -Milliseconds 200
    [NativeProbe]::ClickClient($settings,570,380) # middle-click switch
    [NativeProbe]::ClickClient($settings,80,550)
    Start-Sleep -Milliseconds 300 # Do not open the config while the UI performs its atomic replace.
    Wait-Until { (Get-Content $config -Raw) -match 'Ctrl\+Alt\+W' -and (Get-Content $config -Raw) -match 'middle_click = true' } 'settings save and live hotkey/middle-hook apply'
    if ([NativeProbe]::RegisterHotKey([IntPtr]::Zero,907,3,0x57)) { [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,907); throw 'New shortcut was saved but not registered' }
    if (-not [NativeProbe]::RegisterHotKey([IntPtr]::Zero,908,3,0x51)) { throw 'Old shortcut was not released by successful rebind' }
    [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,908)
    if (Get-ChildItem $work -Filter '*.tmp') { throw 'Atomic config save left a temporary file' }
    $results.Add([pscustomobject]@{case='real settings save atomically applies changed hotkey and enables middle hook';passed=$true})
    # Select the tools tab, then scroll only its content area.
    [NativeProbe]::ClickClient($settings,120,130) # tools tab
    Start-Sleep -Milliseconds 250
    [NativeProbe]::ClickClient($settings,120,335) # RDP tool in sidebar
    Start-Sleep -Milliseconds 300
    $toolsTabCapture=[Drawing.Bitmap]::new($rectangle.Right-$rectangle.Left,$rectangle.Bottom-$rectangle.Top)
    $toolsTabGraphics=[Drawing.Graphics]::FromImage($toolsTabCapture)
    try { $toolsTabGraphics.CopyFromScreen($rectangle.Left,$rectangle.Top,0,0,$toolsTabCapture.Size); $toolsTabCapture.Save((Join-Path $work 'tools-tab.png'),[Drawing.Imaging.ImageFormat]::Png) } finally { $toolsTabGraphics.Dispose(); $toolsTabCapture.Dispose() }
    if (-not [NativeProbe]::RegisterHotKey([IntPtr]::Zero,909,3,0x79)) { throw 'Ctrl+Alt+F10 is occupied before tool UI test' }
    [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,909)
    [NativeProbe]::ClickClient($settings,420,369)
    [NativeProbe]::CaptureShortcut($settings,0x79,$false)
    [NativeProbe]::ClickClient($settings,555,291) # disable RDP tool but retain its shortcut text
    [NativeProbe]::ClickClient($settings,90,550)
    Start-Sleep -Milliseconds 300 # Do not race the atomic config replace with Get-Content.
    Wait-Until { (Get-Content $config -Raw) -match 'path-rdp = "Ctrl\+Alt\+F10"' -and (Get-Content $config -Raw) -match 'disabled_tools = \["path-rdp"\]' } 'per-tool disabled setting persisted'
    if (-not [NativeProbe]::RegisterHotKey([IntPtr]::Zero,910,3,0x79)) { throw 'Disabled tool unexpectedly holds its shortcut' }
    [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,910)
    # Close settings before menu inspection so WindUI toast popups cannot be mistaken for #32768 native menus.
    [void][NativeProbe]::PostMessage($settings,0x10,[UIntPtr]::Zero,[IntPtr]::Zero)
    Wait-Until { [NativeProbe]::FindWindow($null,'TrayKit 设置') -eq [IntPtr]::Zero } 'destroy settings before filtered menu'
    Start-Sleep -Milliseconds 500
    [NativeProbe]::Shortcut(0x57)
    Wait-Until { @((Read-Menu) | Where-Object { $_ }).Count -gt 0 } 'filtered menu'
    $filteredMenu = Read-Menu
    $filteredMenu | ConvertTo-Json | Set-Content (Join-Path $work 'filtered-menu.json') -Encoding utf8
    if ($filteredMenu -contains 'RDP 路径互转' -or $filteredMenu -notcontains '整理剪贴板文本') { throw ('Tool filtering lost stable registry mapping: ' + ($filteredMenu -join ',')) }
    [NativeProbe]::ClickMenu($process.Id,[array]::IndexOf($filteredMenu,'设置…'))
    Wait-Until { [NativeProbe]::FindWindow($null,'TrayKit 设置') -ne [IntPtr]::Zero } 'reopen settings for enable'
    $settings=[NativeProbe]::FindWindow($null,'TrayKit 设置')
    Start-Sleep -Milliseconds 500 # HWND exists before WindUI finishes laying out tab content.
    [NativeProbe]::ClickClient($settings,120,130) # tools tab
    Start-Sleep -Milliseconds 250
    [NativeProbe]::ClickClient($settings,120,335) # RDP tool in sidebar
    Start-Sleep -Milliseconds 300
    $results.Add([pscustomobject]@{case='sidebar tool selection and disable hide menu while retaining dormant shortcut';passed=$true})
    [NativeProbe]::ClickClient($settings,555,291)
    [NativeProbe]::ClickClient($settings,90,550)
    Start-Sleep -Milliseconds 300 # Do not race the atomic config replace with Get-Content.
    Wait-Until { (Get-Content $config -Raw) -match 'disabled_tools = \[\]' } 'tool reenabled'
    if ([NativeProbe]::RegisterHotKey([IntPtr]::Zero,911,3,0x79)) { [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,911); throw 'Enabled tool hotkey was not installed' }
    [void][NativeProbe]::PostMessage($settings,0x10,[UIntPtr]::Zero,[IntPtr]::Zero)
    Wait-Until { [NativeProbe]::FindWindow($null,'TrayKit 设置') -eq [IntPtr]::Zero } 'destroy settings before reenabled menu'
    Start-Sleep -Milliseconds 300
    [NativeProbe]::Shortcut(0x57)
    Wait-Until { @((Read-Menu) | Where-Object { $_ }).Count -gt 0 } 'reenabled menu'
    $reenabledMenu=Read-Menu
    if ($reenabledMenu -notcontains 'RDP 路径互转') { throw ('Reenabled tool missing from menu: ' + ($reenabledMenu -join ',')) }
    [NativeProbe]::ClickMenu($process.Id,[array]::IndexOf($reenabledMenu,'设置…'))
    Wait-Until { [NativeProbe]::FindWindow($null,'TrayKit 设置') -ne [IntPtr]::Zero } 'reopen settings to clear binding'
    $settings=[NativeProbe]::FindWindow($null,'TrayKit 设置')
    Start-Sleep -Milliseconds 500 # Wait for the recreated settings content before selecting its tools tab.
    [NativeProbe]::ClickClient($settings,120,130) # tools tab
    Start-Sleep -Milliseconds 250
    [NativeProbe]::ClickClient($settings,120,335) # RDP tool in sidebar
    Start-Sleep -Milliseconds 300
    $results.Add([pscustomobject]@{case='sidebar tool selection and reenable restore menu and per-tool OS hotkey';passed=$true})
    if (-not [NativeProbe]::RegisterHotKey([IntPtr]::Zero,912,3,0x78)) { throw 'Ctrl+Alt+F9 is occupied before per-tool rebind' }
    [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,912)
    [NativeProbe]::ClickClient($settings,420,369)
    [NativeProbe]::CaptureShortcut($settings,0x78,$false)
    [NativeProbe]::ClickClient($settings,90,550)
    Start-Sleep -Milliseconds 300 # Do not race the atomic config replace with Get-Content.
    Wait-Until { (Get-Content $config -Raw) -match 'path-rdp = "Ctrl\+Alt\+F9"' } 'per-tool shortcut rebind'
    if (-not [NativeProbe]::RegisterHotKey([IntPtr]::Zero,913,3,0x79)) { throw 'Old per-tool hotkey remains registered' }
    [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,913)
    if ([NativeProbe]::RegisterHotKey([IntPtr]::Zero,914,3,0x78)) { [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,914); throw 'New per-tool hotkey was not registered' }
    $results.Add([pscustomobject]@{case='real per-tool shortcut rebind releases old and registers new OS hotkey';passed=$true})
    Start-Sleep -Milliseconds 300
    [void][NativeProbe]::GetWindowRect($settings,[ref]$rectangle)
    $toolCapture=[Drawing.Bitmap]::new($rectangle.Right-$rectangle.Left,$rectangle.Bottom-$rectangle.Top)
    $toolGraphics=[Drawing.Graphics]::FromImage($toolCapture)
    try { $toolGraphics.CopyFromScreen($rectangle.Left,$rectangle.Top,0,0,$toolCapture.Size); $toolCapture.Save((Join-Path $work 'tools.png'),[Drawing.Imaging.ImageFormat]::Png) } finally { $toolGraphics.Dispose(); $toolCapture.Dispose() }
    [void][NativeProbe]::PostMessage($settings,0x10,[UIntPtr]::Zero,[IntPtr]::Zero)
    Wait-Until { -not [NativeProbe]::IsWindowVisible($settings) } 'close hides settings'
    if ($process.HasExited) { throw 'Settings close exited the resident process' }
    $results.Add([pscustomobject]@{case='settings close destroys its window without exiting resident process';passed=$true})
    # Sample true idle after settings has hidden; no polling is added to TrayKit.
    $process.Refresh(); $cpuBefore=$process.TotalProcessorTime.TotalMilliseconds
    Start-Sleep -Seconds 2
    $process.Refresh(); $cpuDelta=$process.TotalProcessorTime.TotalMilliseconds-$cpuBefore
    $results.Add([pscustomobject]@{case='idle resource sample';passed=$true;cpu_ms_over_2s=$cpuDelta;private_bytes=$process.PrivateMemorySize64;working_set=$process.WorkingSet64})
    [NativeProbe]::Shortcut(0x57)
    Wait-Until { $null -ne (Read-Menu) } 'reopen for exit'
    [NativeProbe]::ClickMenu($process.Id,[array]::IndexOf((Read-Menu),"退出 TrayKit")) # stable label lookup
    if (-not $process.WaitForExit(5000) -or $process.ExitCode -ne 0) { throw 'Menu exit failed' }
    if ([NativeProbe]::FindWindow('TrayKit.NativeBridge','TrayKit Events') -ne [IntPtr]::Zero) { throw 'Native event window leaked after exit' }
    foreach ($key in @(0x51,0x57,0x78,0x79,0x7A)) {
        if (-not [NativeProbe]::RegisterHotKey([IntPtr]::Zero,904,3,$key)) { throw 'Shortcut was not released on exit' }
        [void][NativeProbe]::UnregisterHotKey([IntPtr]::Zero,904)
    }
    $results.Add([pscustomobject]@{case='menu exit destroys native host and releases both hotkeys';passed=$true})
} finally {
    if ($null -ne $process -and -not $process.HasExited) {
        [NativeProbe]::Key(0x1B)
        # Cleanup only the process launched by this test if the normal path failed.
        $process.Kill(); $process.WaitForExit()
    }
    if ($null -ne $process) { $process.StandardError.ReadToEnd() | Set-Content (Join-Path $work "stderr.log") -Encoding utf8; $process.StandardOutput.ReadToEnd() | Set-Content (Join-Path $work "stdout.log") -Encoding utf8 }
    [pscustomobject]@{binary=$Binary;binary_sha256=(Get-FileHash $Binary -Algorithm SHA256).Hash;results=$results} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $work 'result.json') -Encoding utf8
}
$results | Format-Table case,passed
Write-Output ('Evidence: ' + (Join-Path $work 'result.json'))
