# Build an unsigned MSIX for the Microsoft Store. Partner Center signs it on upload.
# Usage: packaging\msix\build-msix.ps1 -Version 2026.1.0
param(
  [Parameter(Mandatory)] [string] $Version,
  # Values from Partner Center > AIVory Code (9NG19Z8T9XR6) > Product identity.
  [string] $IdentityName = "AIVoryInc.AIVoryCode",
  [string] $Publisher = "CN=1E786547-45C5-4CB8-8218-99D720EF3558",
  [string] $PublisherDisplay = "AIVory, Inc.",
  [string] $BinDir = "target\release",
  [string] $OutDir = "dist"
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot\..\.."
Set-Location $root

# MSIX versions have four parts; the last part must be 0 for the Store.
$msixVersion = "$Version.0"

$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" |
  Sort-Object FullName | Select-Object -Last 1
if (-not $sdk) { throw "makeappx.exe not found. Install the Windows SDK." }

$stage = Join-Path $OutDir "msix-stage"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force "$stage\Assets" | Out-Null

foreach ($exe in "aivory.exe", "aivoryd.exe") {
  $src = Join-Path $BinDir $exe
  if (-not (Test-Path $src)) { throw "$src missing. Run: cargo build --release --workspace" }
  Copy-Item $src $stage
}
if (Test-Path "apps\desktop\assets") { Copy-Item -Recurse "apps\desktop\assets" "$stage\assets" }

# Logos from the 512 px app icon.
python "$PSScriptRoot\make-logos.py" "apps\desktop\packaging\icons\512x512.png" "$stage\Assets"
if ($LASTEXITCODE) { throw "logo step failed" }

(Get-Content "$PSScriptRoot\AppxManifest.xml" -Raw).
  Replace("{{IDENTITY_NAME}}", $IdentityName).
  Replace("{{PUBLISHER}}", [Security.SecurityElement]::Escape($Publisher)).
  Replace("{{PUBLISHER_DISPLAY}}", [Security.SecurityElement]::Escape($PublisherDisplay)).
  Replace("{{VERSION}}", $msixVersion) |
  Set-Content -Encoding utf8 "$stage\AppxManifest.xml"

$out = Join-Path $OutDir "aivory-code-$Version-x64.msix"
& $sdk.FullName pack /o /d $stage /p $out
if ($LASTEXITCODE) { throw "makeappx failed" }
Write-Host "MSIX: $out"
