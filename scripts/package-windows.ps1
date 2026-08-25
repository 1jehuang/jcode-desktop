$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$JcodeRepo = if ($env:JCODE_REPO) { $env:JCODE_REPO } else { (Resolve-Path (Join-Path $Root '../jcode')).Path }
$Version = if ($env:VERSION) { $env:VERSION } else { (Select-String -Path "$Root/Cargo.toml" -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value }
$Version = $Version -replace '^desktop-v','' -replace '^v',''
if ($Version -notmatch '^\d+\.\d+\.\d+([.-][0-9A-Za-z.-]+)?$') { throw "Invalid VERSION: $Version" }
$Out = if ($env:OUT_DIR) { $env:OUT_DIR } else { Join-Path $Root 'dist/windows' }
$Stage = Join-Path $Out "Jcode-$Version-windows-x86_64"
$env:JCODE_DESKTOP_VERSION = $Version
if ($env:SKIP_BUILD -ne '1') {
  cargo build --manifest-path "$Root/Cargo.toml" --release --bin jcode-desktop
  if ($LASTEXITCODE) { throw 'Desktop build failed' }
  cargo build --manifest-path "$JcodeRepo/Cargo.toml" --release --bin jcode
  if ($LASTEXITCODE) { throw 'Jcode build failed' }
  cargo build --manifest-path "$JcodeRepo/Cargo.toml" --release --package jcode-harness-api-server --bin jcode-harness-api-bridge
  if ($LASTEXITCODE) { throw 'Bridge build failed' }
}
Remove-Item $Stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item $Stage -ItemType Directory -Force | Out-Null
Copy-Item "$Root/target/release/jcode-desktop.exe" "$Stage/jcode-desktop.exe"
Copy-Item "$JcodeRepo/target/release/jcode.exe" "$Stage/jcode.exe"
Copy-Item "$JcodeRepo/target/release/jcode-harness-api-bridge.exe" "$Stage/jcode-harness-api-bridge.exe"
Copy-Item "$Root/assets/app-icon/icon-1024.png" "$Stage/Jcode.png"
Copy-Item "$Root/packaging/windows/jcode-desktop.exe.manifest" "$Stage/jcode-desktop.exe.manifest"
$Zip = Join-Path $Out "Jcode-$Version-windows-x86_64.zip"
Remove-Item $Zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path $Stage -DestinationPath $Zip -CompressionLevel Optimal
(Get-FileHash $Zip -Algorithm SHA256).Hash.ToLower() + "  " + (Split-Path $Zip -Leaf) | Set-Content (Join-Path $Out 'SHA256SUMS-windows')
Write-Host "Packaged Windows artifact at $Zip"
