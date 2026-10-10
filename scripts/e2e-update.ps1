# End-to-end: an older Windows Jcode Desktop updates itself from the live
# public release channel, the way a user who extracted the zip has it.
#
# Usage: e2e-update.ps1 -Binary <jcode-desktop.exe built as an older version>
# Only a temporary directory is modified.
param(
  [Parameter(Mandatory = $true)][string]$Binary,
  [string]$Expect
)
$ErrorActionPreference = 'Stop'

if (-not $Expect) {
  $latest = Invoke-RestMethod 'https://github.com/1jehuang/jcode-desktop-releases/releases/download/desktop-latest/latest.json'
  $Expect = $latest.tag_name -replace '^desktop-v', ''
}

function Get-DesktopVersion([string]$exe) {
  $out = & $exe --version
  if ($LASTEXITCODE) { throw "$exe --version failed" }
  return (($out -split '\s+')[2]) -replace '^v', ''
}

$old = Get-DesktopVersion $Binary
if ($old -eq $Expect) { throw "$Binary already reports $Expect. Build it with JCODE_DESKTOP_VERSION=<older>" }
Write-Host "Updating Jcode Desktop $old -> live $Expect"

$root = Join-Path ([System.IO.Path]::GetTempPath()) ("jcode-e2e-" + [guid]::NewGuid())
$folder = Join-Path $root "Jcode-$old-windows"
New-Item $folder -ItemType Directory -Force | Out-Null
Copy-Item $Binary (Join-Path $folder 'jcode-desktop.exe')
foreach ($name in 'jcode.exe', 'jcode-harness-api-bridge.exe', 'jcode-desktop.exe.manifest', 'Jcode.png') {
  Set-Content -Path (Join-Path $folder $name) -Value 'placeholder companion, replaced by the update'
}
$env:USERPROFILE = Join-Path $root 'profile'
$env:APPDATA = Join-Path $root 'profile\AppData\Roaming'
$env:LOCALAPPDATA = Join-Path $root 'profile\AppData\Local'
$env:JCODE_HOME = Join-Path $root 'profile\.jcode'
$env:CI = '1'
New-Item $env:APPDATA, $env:LOCALAPPDATA -ItemType Directory -Force | Out-Null

$entry = Join-Path $folder 'jcode-desktop.exe'
# The updater runs inside jcode-desktop.exe and swaps that very file while it
# runs, exactly as the in-app background updater does. Windows must allow it.
$output = & $entry --update 2>&1 | Out-String
Write-Host $output
if ($LASTEXITCODE) { throw "--update exited $LASTEXITCODE" }
if ($output -notmatch [regex]::Escape("Installed Jcode Desktop $Expect (windows_portable)")) {
  throw "update did not report installing $Expect"
}
$after = Get-DesktopVersion $entry
if ($after -ne $Expect) { throw "after updating, jcode-desktop.exe reports $after, expected $Expect" }
foreach ($name in 'jcode.exe', 'jcode-harness-api-bridge.exe') {
  $bytes = [System.IO.File]::ReadAllBytes((Join-Path $folder $name))
  if ($bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) { throw "$name was not replaced by a real executable" }
}
$cli = & (Join-Path $folder 'jcode.exe') --version
if ($LASTEXITCODE) { throw 'updated jcode.exe does not start' }
Write-Host "Bundled CLI after update: $cli"
$aside = @(Get-ChildItem $folder -Filter '*.jcode-old-*').Count
Write-Host "Previous files set aside for cleanup at next launch: $aside"
Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
Write-Host "PASS: Windows zip install updated $old -> $Expect in place"
