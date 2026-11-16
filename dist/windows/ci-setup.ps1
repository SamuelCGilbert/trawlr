# Install what dist\windows\build.ps1 needs on a fresh Windows machine (a
# GitHub runner): Qt 6.8 for MSVC, and with -Inno, Inno Setup 6.
param([switch]$Inno)
$ErrorActionPreference = 'Stop'

python -m pip install --quiet aqtinstall==3.3.0
python -m aqt install-qt windows desktop 6.8.3 win64_msvc2022_64 --archives qtbase qtsvg -O C:\Qt
if ($LASTEXITCODE) { throw "Qt did not install" }
$qt = 'C:\Qt\6.8.3\msvc2022_64\bin'
if ($env:GITHUB_PATH) { $qt | Out-File -Append -Encoding utf8 $env:GITHUB_PATH }

if ($Inno) {
    $f = Join-Path $env:RUNNER_TEMP 'innosetup.exe'
    Invoke-WebRequest -UseBasicParsing -Uri 'https://github.com/jrsoftware/issrc/releases/download/is-6_7_4/innosetup-6.7.4.exe' -OutFile $f
    # Run it only if it is signed by Inno Setup's publisher.
    $sig = Get-AuthenticodeSignature $f
    if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'Pyrsys') { throw "Inno Setup download is not signed as expected: $($sig.Status) $($sig.SignerCertificate.Subject)" }
    $p = Start-Process $f -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/ALLUSERS', '/SP-' -Wait -PassThru
    if ($p.ExitCode) { throw "Inno Setup did not install ($($p.ExitCode))" }
}
