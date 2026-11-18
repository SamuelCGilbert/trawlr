# Start trawlrd on drive C: (as administrator), run the live-update checks
# against it, and stop it again. CI runs this on GitHub's Windows machines.
#   powershell -ExecutionPolicy Bypass -File tests\windows\run.ps1
param([string]$Bin = "target\release")
$ErrorActionPreference = 'Stop'
$Bin = (Resolve-Path $Bin).Path
$work = Join-Path $env:TEMP "trawlr-test-$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$cfg = Join-Path $work 'config.toml'
Set-Content $cfg ''
# This daemon runs in our session, not as the service: name its pipe, which
# tells trawlr to talk to it anyway.
$env:TRAWLRD_SOCKET = '\\.\pipe\trawlrd'
$d = Start-Process "$Bin\trawlrd.exe" -ArgumentList '--root', 'C:', '--db', "$work\index.db", '--config', $cfg -PassThru -NoNewWindow -RedirectStandardError "$work\trawlrd.log"
$code = 1
try {
    $deadline = (Get-Date).AddMinutes(3)
    do {
        Start-Sleep 1
        $s = (& "$Bin\trawlr.exe" --status 2>$null) -join ' '
    } until ($s -match ', ready' -or $d.HasExited -or (Get-Date) -gt $deadline)
    Get-Content "$work\trawlrd.log"
    if ($s -notmatch ', ready') { throw "trawlrd did not get ready: $s" }
    & powershell -NoProfile -ExecutionPolicy Bypass -File "$PSScriptRoot\live.ps1" -Trawlr "$Bin\trawlr.exe"
    $code = $LASTEXITCODE
} finally {
    Stop-Process -Id $d.Id -Force -ErrorAction SilentlyContinue
}
exit $code
