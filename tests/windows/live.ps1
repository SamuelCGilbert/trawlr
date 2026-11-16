# Live-update checks for trawlrd on Windows: files and folders created,
# renamed, moved, hard-linked and deleted in a scratch folder must show up in
# (and leave) the index within seconds. Needs a running trawlrd that
# indexes the drive $Dir is on.
#   powershell -File tests\windows\live.ps1 [-Trawlr path\to\trawlr.exe] [-Dir C:\trawlr-live]
param([string]$Trawlr = "target\release\trawlr.exe", [string]$Dir = "C:\trawlr-live")
$ErrorActionPreference = 'Stop'
$T = (Resolve-Path $Trawlr).Path
$root = $Dir
$leaf = Split-Path $Dir -Leaf
if (Test-Path $root) { Remove-Item -Recurse -Force $root }
$fails = 0
function Q([string]$q) { (& $T --count $q) -as [int] }
function Expect([string]$what, [string]$q, [int]$n) {
    $deadline = (Get-Date).AddSeconds(5)
    do { $got = Q $q; if ($got -eq $n) { break }; Start-Sleep -Milliseconds 100 } while ((Get-Date) -lt $deadline)
    if ($got -eq $n) { Write-Output "ok    $what" } else { Write-Output "FAIL  $what : $q gave $got, expected $n"; $script:fails++ }
}
$tag = 'zq' + (Get-Random -Maximum 999999)
New-Item -ItemType Directory "$root\a\b" | Out-Null
Set-Content "$root\a\b\$tag-one.txt" 'hello'
Expect 'new file in new folders' "$tag-one.txt" 1
Expect 'its path' "$leaf\a\b\$tag-one.txt" 1
Rename-Item "$root\a\b\$tag-one.txt" "$tag-two.txt"
Expect 'rename: old name gone' "$tag-one.txt" 0
Expect 'rename: new name there' "$tag-two.txt" 1
1..50 | ForEach-Object { Set-Content "$root\a\b\$tag-many-$_.dat" 'x' }
Expect 'fifty files' "$tag-many-" 50
Rename-Item "$root\a\b" "c"
Expect 'folder rename moves children' "$leaf\a\c\$tag-many-" 50
Expect 'old folder path empty' "$leaf\a\b\$tag-many-" 0
New-Item -ItemType Directory "$root\x\y" | Out-Null
Move-Item "$root\a\c" "$root\x\y\c"
Expect 'folder move moves children' "$leaf\x\y\c\$tag-many-" 50
Expect 'nothing left at old place' "$leaf\a\c\" 0
Add-Content "$root\x\y\c\$tag-two.txt" ('z' * 5000)
Start-Sleep -Milliseconds 600
$row = (& $T --json "$tag-two.txt") | ConvertFrom-Json
if ($row.s -gt 5000) { Write-Output "ok    size follows writes ($($row.s))" } else { Write-Output "FAIL  size is $($row.s)"; $fails++ }
New-Item -ItemType HardLink -Path "$root\x\$tag-link.txt" -Target "$root\x\y\c\$tag-two.txt" | Out-Null
Expect 'hard link appears' "$tag-link.txt" 1
Expect 'original still there' "$tag-two.txt" 1
Remove-Item "$root\x\$tag-link.txt"
Expect 'hard link removed' "$tag-link.txt" 0
Expect 'original kept' "$tag-two.txt" 1
Rename-Item "$root\x\y\c\$tag-two.txt" "$tag-TWO.txt"
Start-Sleep -Milliseconds 600
$names = (& $T "$tag-two.txt")
if ($names -match "$tag-TWO.txt" -and @($names).Count -eq 1) { Write-Output "ok    case-only rename" } else { Write-Output "FAIL  case-only rename: $names"; $fails++ }
# Delete one name, create another under it, in one go.
Remove-Item "$root\x\y\c\$tag-many-1.dat"; Set-Content "$root\x\y\c\$tag-many-1.dat" 'again'
Expect 'delete then recreate same name' "$tag-many-1.dat" 1
# Folder created, filled and moved quickly.
New-Item -ItemType Directory "$root\fast" | Out-Null; 1..10 | ForEach-Object { Set-Content "$root\fast\$tag-fast-$_" 'f' }; New-Item -ItemType Directory "$root\fast2" | Out-Null; Move-Item "$root\fast" "$root\fast2\fast"
Expect 'quick create-fill-move' "$leaf\fast2\fast\$tag-fast-" 10
Remove-Item -Recurse -Force "$root\x"
Expect 'tree delete' "$tag-many-" 0
Expect 'tree delete (case-renamed)' "$tag-two.txt" 0
Remove-Item -Recurse -Force $root
Expect 'scratch folder gone' "$tag" 0
Write-Output "failures: $fails"
exit [int]($fails -gt 0)
