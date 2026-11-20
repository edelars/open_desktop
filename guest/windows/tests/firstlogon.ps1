<#
.SYNOPSIS
    OD8 phase script run inside Windows: firstlogon / boot / shutdown markers (S8-22).

.DESCRIPTION
    Mode FirstLogon (default): started once by FirstLogonCommands of autounattend.xml
    (tools/build/make_windows_unattend.py) from the payload disk (volume label OD8PAYLOAD).
      1. reads OD8\nonce.txt from the payload disk;
      2. registers the scheduled task OD8Boot (at system startup, as SYSTEM);
      3. appends the marker "firstlogon" to OD8\markers.txt;
      4. appends the marker "shutdown", flushes the file to disk, runs shutdown /s /t 0.
    Mode Boot: started by the task OD8Boot on every following boot. It finds the payload
    disk by label again, reads OD8\nonce.txt, appends the marker "boot", runs
    OD8\boot.ps1 when it exists (the scenario script of S8-30...S8-63; it receives the
    payload root and the nonce in the environment variables OD8_ROOT and OD8_NONCE and
    may append its own markers), then appends "shutdown", flushes and powers off.
    A scenario that must reboot or keep running must not return from boot.ps1.

    Marker format (one line, LF):  OD8 <nonce> <phase> <status> <sha256>
      phase  : firstlogon | boot | shutdown
      status : ok | fail
      sha256 : lowercase hex SHA-256 that the host can recompute independently:
        firstlogon, boot : of the UTF-8 string "<CurrentBuild>.<UBR>" from
                           HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion
                           (on fail: of the error message);
        shutdown         : of the bytes of markers.txt as they were before this line
                           (on fail the status is fail, the hash rule is the same).
    The script only reports phases it really passed; it never writes a marker without
    the nonce. SHA-256 is computed with System.Security.Cryptography, no external tools.
    The script holds no secrets (keys or credentials).

    Documentation used (read 2026-11-20):
    - Task registration: Register-ScheduledTask, New-ScheduledTaskTrigger -AtStartup,
      New-ScheduledTaskPrincipal -UserId ... -LogonType ServiceAccount (example with
      LOCALSERVICE), New-ScheduledTaskSettingsSet, all under
      https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/ ;
      schtasks /create /sc ONSTART "runs every time the system starts" /ru system:
      https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/schtasks-create
      The cmdlets are used; schtasks is the documented alternative.
      Source for ExecutionTimeLimit 0 = unlimited was not found, so an explicit 7 days is set.
    - Get-Volume -FileSystemLabel:
      https://learn.microsoft.com/en-us/powershell/module/storage/get-volume
    - -ExecutionPolicy Bypass: about_Execution_Policies says "The execution policy isn't
      a security boundary, it's defense in depth", so this is not a bypass in the sense of
      the stage 8 plan dictionary:
      https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_execution_policies
#>
[CmdletBinding()]
param(
    [ValidateSet('FirstLogon', 'Boot')]
    [string]$Mode = 'FirstLogon'
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$PayloadLabel = 'OD8PAYLOAD'
$PayloadDir = 'OD8'
$ScriptName = 'firstlogon.ps1'
$TaskName = 'OD8Boot'
$PayloadWaitSeconds = 120
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$DiagDir = Join-Path $env:ProgramData 'OD8'

function Write-Diag {
    # Guest-local diagnostic line (read later by S8-12); never fatal.
    param([string]$Text)
    try {
        if (-not (Test-Path -LiteralPath $DiagDir)) { New-Item -ItemType Directory -Path $DiagDir -Force | Out-Null }
        $line = '{0} {1} {2}' -f ([DateTime]::UtcNow.ToString('o')), $Mode, $Text
        [System.IO.File]::AppendAllText((Join-Path $DiagDir 'diag.txt'), $line + "`n", $Utf8NoBom)
    } catch { }
}

function Get-Sha256Hex {
    param([byte[]]$Bytes)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha.ComputeHash($Bytes)
    } finally {
        $sha.Dispose()
    }
    return (($hash | ForEach-Object { $_.ToString('x2') }) -join '')
}

function Get-StringSha256Hex {
    param([string]$Text)
    return (Get-Sha256Hex -Bytes $Utf8NoBom.GetBytes($Text))
}

function Get-BuildString {
    $key = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
    return ('{0}.{1}' -f $key.CurrentBuild, $key.UBR)
}

function Find-PayloadRoot {
    # Volume with the payload label; the payload disk may appear after the task starts.
    $deadline = [DateTime]::UtcNow.AddSeconds($PayloadWaitSeconds)
    do {
        $volume = Get-Volume -FileSystemLabel $PayloadLabel -ErrorAction SilentlyContinue |
            Where-Object { $_.DriveLetter } | Select-Object -First 1
        if ($volume) { return ('{0}:\' -f $volume.DriveLetter) }
        Start-Sleep -Seconds 2
    } while ([DateTime]::UtcNow -lt $deadline)
    throw ('volume with label {0} not found' -f $PayloadLabel)
}

function Read-Nonce {
    param([string]$Root)
    $path = Join-Path $Root (Join-Path $PayloadDir 'nonce.txt')
    $nonce = [System.IO.File]::ReadAllText($path).Trim()
    # 128 bits as 32 lowercase hex digits (docs/testing/WINDOWS.md, S8-02). -cnotmatch: -match is case-insensitive; \z: $ also matches before a final LF.
    if ($nonce -cnotmatch '^[0-9a-f]{32}\z') { throw 'nonce.txt must hold exactly 32 lowercase hex digits' }
    return $nonce
}

function Add-MarkerLine {
    # Append one line write-through and flush it to the device.
    param([string]$MarkersPath, [string]$Line)
    $bytes = $Utf8NoBom.GetBytes($Line + "`n")
    $stream = New-Object System.IO.FileStream($MarkersPath,
        [System.IO.FileMode]::Append, [System.IO.FileAccess]::Write, [System.IO.FileShare]::Read,
        4096, [System.IO.FileOptions]::WriteThrough)
    try {
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush($true)
    } finally {
        $stream.Dispose()
    }
}

function Write-Marker {
    param([string]$MarkersPath, [string]$Nonce, [string]$Phase, [string]$Status, [string]$Hash)
    Add-MarkerLine -MarkersPath $MarkersPath -Line ('OD8 {0} {1} {2} {3}' -f $Nonce, $Phase, $Status, $Hash)
}

function Write-ShutdownMarker {
    # Hash = SHA-256 of markers.txt as it is before this line.
    param([string]$MarkersPath, [string]$Nonce, [string]$Status)
    $before = [System.IO.File]::ReadAllBytes($MarkersPath)
    Write-Marker -MarkersPath $MarkersPath -Nonce $Nonce -Phase 'shutdown' -Status $Status -Hash (Get-Sha256Hex -Bytes $before)
}

function Get-FinderCommand {
    # Boot-time finder: the drive letter of the payload disk can differ between boots.
    # Single-quoted here-string: the inner variables are for the child PowerShell;
    # only the three __NAME__ placeholders are filled in here.
    $finder = @'
$v = $null
for ($i = 0; $i -lt __TRIES__ -and -not $v; $i++) {
    $v = Get-Volume -FileSystemLabel '__LABEL__' -ErrorAction SilentlyContinue | Where-Object { $_.DriveLetter } | Select-Object -First 1
    if (-not $v) { Start-Sleep -Seconds 2 }
}
if (-not $v) { exit 2 }
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File ('{0}:\__DIR__\__SCRIPT__' -f $v.DriveLetter) -Mode Boot
exit $LASTEXITCODE
'@
    return $finder.Replace('__TRIES__', [string][int]($PayloadWaitSeconds / 2)).Replace('__LABEL__', $PayloadLabel).Replace('__DIR__', $PayloadDir).Replace('__SCRIPT__', $ScriptName)
}

function Register-BootTask {
    $encoded = [Convert]::ToBase64String([System.Text.Encoding]::Unicode.GetBytes((Get-FinderCommand)))
    $arguments = '-NoProfile -ExecutionPolicy Bypass -EncodedCommand ' + $encoded
    $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $arguments
    $trigger = New-ScheduledTaskTrigger -AtStartup
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Days 7)
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
    $task = Get-ScheduledTask -TaskName $TaskName
    if ($task.Principal.UserId -notmatch 'SYSTEM') { throw ('task {0} is not registered for SYSTEM' -f $TaskName) }
}

function Invoke-PowerOff {
    Write-Diag 'shutdown /s /t 0'
    & shutdown.exe /s /t 0
    exit 0
}

function Invoke-FirstLogon {
    param([string]$Root, [string]$Nonce, [string]$MarkersPath)
    $status = 'ok'
    try {
        Register-BootTask
        $hash = Get-StringSha256Hex -Text (Get-BuildString)
    } catch {
        $status = 'fail'
        $hash = Get-StringSha256Hex -Text ([string]$_.Exception.Message)
        Write-Diag ('firstlogon failed: ' + $_.Exception.Message)
    }
    Write-Marker -MarkersPath $MarkersPath -Nonce $Nonce -Phase 'firstlogon' -Status $status -Hash $hash
    Write-ShutdownMarker -MarkersPath $MarkersPath -Nonce $Nonce -Status $status
    Invoke-PowerOff
}

function Invoke-Boot {
    param([string]$Root, [string]$Nonce, [string]$MarkersPath)
    $status = 'ok'
    try {
        $hash = Get-StringSha256Hex -Text (Get-BuildString)
    } catch {
        $status = 'fail'
        $hash = Get-StringSha256Hex -Text ([string]$_.Exception.Message)
        Write-Diag ('boot failed: ' + $_.Exception.Message)
    }
    Write-Marker -MarkersPath $MarkersPath -Nonce $Nonce -Phase 'boot' -Status $status -Hash $hash

    $scenario = Join-Path $Root (Join-Path $PayloadDir 'boot.ps1')
    if (Test-Path -LiteralPath $scenario -PathType Leaf) {
        $env:OD8_ROOT = $Root
        $env:OD8_NONCE = $Nonce
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $scenario
        if ($LASTEXITCODE -ne 0) {
            $status = 'fail'
            Write-Diag ('boot.ps1 exit code ' + $LASTEXITCODE)
        }
    }
    Write-ShutdownMarker -MarkersPath $MarkersPath -Nonce $Nonce -Status $status
    Invoke-PowerOff
}

try {
    $root = Find-PayloadRoot
    $nonce = Read-Nonce -Root $root
    $markersPath = Join-Path $root (Join-Path $PayloadDir 'markers.txt')
} catch {
    # Without the payload disk and the nonce no marker can be written: the run stays
    # without a marker and is judged failed by the host.
    Write-Diag ('cannot start: ' + $_.Exception.Message)
    exit 1
}

if ($Mode -eq 'Boot') {
    Invoke-Boot -Root $root -Nonce $nonce -MarkersPath $markersPath
} else {
    Invoke-FirstLogon -Root $root -Nonce $nonce -MarkersPath $markersPath
}
