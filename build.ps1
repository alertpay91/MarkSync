# Builds MarkSync.exe and the installer into .\dist
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
$ver = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
# a running MarkSync locks target\release\marksync.exe: then build into target-dev
$targetDir = "target"
if (Get-Process marksync -ErrorAction SilentlyContinue) { $targetDir = "target-dev" }
$env:CARGO_TARGET_DIR = $targetDir
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$exeDir = Join-Path $PSScriptRoot (Join-Path $targetDir "release")
python tools\make_cyrillic_isl.py
$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found" }
New-Item -ItemType Directory -Force dist | Out-Null
& $iscc "/DAppVer=$ver" "/DExeDir=$exeDir" installer\marksync.iss
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
Copy-Item (Join-Path $exeDir "marksync.exe") "dist\MarkSync-$ver-portable.exe" -Force
Get-ChildItem dist | Select-Object Name, @{n = "MB"; e = { [math]::Round($_.Length / 1MB, 1) } }
