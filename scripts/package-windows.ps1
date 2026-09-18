$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$JcodeRepo = if ($env:JCODE_REPO) { $env:JCODE_REPO } else { (Resolve-Path (Join-Path $Root '../jcode')).Path }
$Version = if ($env:VERSION) { $env:VERSION } else { (Select-String -Path "$Root/Cargo.toml" -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value }
$Version = $Version -replace '^desktop-v','' -replace '^v',''
if ($Version -notmatch '^\d+\.\d+\.\d+([.-][0-9A-Za-z.-]+)?$') { throw "Invalid VERSION: $Version" }
$Target = $env:TARGET
if (-not $Target) {
  # OSArchitecture identifies the native OS even under an emulated x64 shell.
  switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
    'X64' { $Target = 'x86_64-pc-windows-msvc' }
    'Arm64' { $Target = 'aarch64-pc-windows-msvc' }
    default { throw 'Unsupported native Windows architecture' }
  }
}
switch ($Target) {
  'x86_64-pc-windows-msvc' { $Arch = 'x86_64'; $Checksums = 'SHA256SUMS-windows'; $CliFeatures = @() }
  'aarch64-pc-windows-msvc' { $Arch = 'aarch64'; $Checksums = 'SHA256SUMS-windows-aarch64'; $CliFeatures = @('--no-default-features', '--features', 'pdf') }
  default { throw "Unsupported Windows TARGET: $Target" }
}
$Out = if ($env:OUT_DIR) { $env:OUT_DIR } else { Join-Path $Root 'dist/windows' }
$Stage = Join-Path $Out "Jcode-$Version-windows-$Arch"
$env:JCODE_DESKTOP_VERSION = $Version
if ($env:SKIP_BUILD -ne '1') {
  cargo build --manifest-path "$Root/Cargo.toml" --release --target $Target --bin jcode-desktop
  if ($LASTEXITCODE) { throw 'Desktop build failed' }
  cargo build --manifest-path "$JcodeRepo/Cargo.toml" --release --target $Target --bin jcode @CliFeatures
  if ($LASTEXITCODE) { throw 'Jcode build failed' }
  cargo build --manifest-path "$JcodeRepo/Cargo.toml" --release --target $Target --package jcode-harness-api-server --bin jcode-harness-api-bridge
  if ($LASTEXITCODE) { throw 'Bridge build failed' }
}
Remove-Item $Stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item $Stage -ItemType Directory -Force | Out-Null
Copy-Item "$Root/target/$Target/release/jcode-desktop.exe" "$Stage/jcode-desktop.exe"
Copy-Item "$JcodeRepo/target/$Target/release/jcode.exe" "$Stage/jcode.exe"
Copy-Item "$JcodeRepo/target/$Target/release/jcode-harness-api-bridge.exe" "$Stage/jcode-harness-api-bridge.exe"
Copy-Item "$Root/assets/app-icon/icon-1024.png" "$Stage/Jcode.png"
Copy-Item "$Root/packaging/windows/jcode-desktop.exe.manifest" "$Stage/jcode-desktop.exe.manifest"
$Zip = Join-Path $Out "Jcode-$Version-windows-$Arch.zip"
Remove-Item $Zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path $Stage -DestinationPath $Zip -CompressionLevel Optimal
python "$Root/scripts/verify-release-package.py" --target $Target $Zip
if ($LASTEXITCODE) { throw 'Package verification failed' }
(Get-FileHash $Zip -Algorithm SHA256).Hash.ToLower() + "  " + (Split-Path $Zip -Leaf) | Set-Content (Join-Path $Out $Checksums)
Write-Host "Packaged Windows artifact at $Zip"
