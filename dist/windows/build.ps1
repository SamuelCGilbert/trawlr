# Build Trawlr for Windows and package it as an installer, in dist\out.
#   powershell -ExecutionPolicy Bypass -File dist\windows\build.ps1
# Needs, on PATH: cargo (Rust, MSVC toolchain), Qt 6 for MSVC (qmake,
# windeployqt), the Visual Studio build tools environment (cl, nmake; run
# it from a "Developer PowerShell" or after vcvars64.bat), and Inno Setup 6
# (iscc; looked for in its usual folder too). -NoInstaller stops after
# building the programs.
param([switch]$NoInstaller)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
Set-Location $root
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value

cargo build --release --locked
if ($LASTEXITCODE) { throw "cargo build failed" }

$gb = "gui\build-windows"
New-Item -ItemType Directory -Force $gb | Out-Null
Push-Location $gb
qmake ..\trawlr.pro "QMAKE_CXXFLAGS+=/MP"
if ($LASTEXITCODE) { throw "qmake failed" }
nmake /nologo
if ($LASTEXITCODE) { throw "the window did not build" }
Pop-Location
if ($NoInstaller) { exit 0 }

$stage = "dist\out\windows-stage"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\trawlrd.exe, target\release\trawlr.exe, "$gb\release\trawlr-gui.exe" $stage
Copy-Item README.md, LICENSE $stage
# Qt's libraries and the plugins the window loads. Nothing for networking
# beyond the local pipe, no translations, no software OpenGL.
windeployqt --release --no-translations --no-system-d3d-compiler --no-system-dxc-compiler --no-opengl-sw --no-compiler-runtime `
    --skip-plugin-types tls,networkinformation,generic,sqldrivers "$stage\trawlr-gui.exe"
if ($LASTEXITCODE) { throw "windeployqt failed" }
# The C++ runtime the Qt libraries need, beside them (the Rust programs carry
# their own). From the build tools' redistributable folder.
$crt = Get-ChildItem -Path $env:VCToolsRedistDir -Directory -Filter 'x64' | ForEach-Object { Get-ChildItem $_.FullName -Directory -Filter 'Microsoft.VC*.CRT' } | Select-Object -First 1
if (-not $crt) { throw "the Visual C++ runtime was not found under VCToolsRedistDir ($env:VCToolsRedistDir)" }
Copy-Item "$($crt.FullName)\vcruntime140*.dll", "$($crt.FullName)\msvcp140.dll", "$($crt.FullName)\msvcp140_1.dll", "$($crt.FullName)\msvcp140_2.dll" $stage

$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) { $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
& $iscc /Q "/DAppVersion=$version" "/DStage=$root\$stage" dist\windows\trawlr.iss
if ($LASTEXITCODE) { throw "the installer did not build" }
Get-ChildItem dist\out\*.exe
