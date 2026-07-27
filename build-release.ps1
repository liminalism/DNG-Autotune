param(
    [string]$Configuration = "release"
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ProjectRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "Cargo was not found. Install Rust with rustup, reopen the terminal, and run this script again."
}

Write-Host "Running tests..."
cargo test

Write-Host "Building raw-autotune..."
cargo build --release

$DistRoot = Join-Path $ProjectRoot "dist"
$PackageRoot = Join-Path $DistRoot "raw-autotune-windows-x64"
$ZipPath = Join-Path $DistRoot "raw-autotune-windows-x64.zip"

Remove-Item $PackageRoot -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item $ZipPath -Force -ErrorAction SilentlyContinue
New-Item $PackageRoot -ItemType Directory -Force | Out-Null

Copy-Item (Join-Path $ProjectRoot "target\release\raw-autotune.exe") $PackageRoot
Copy-Item (Join-Path $ProjectRoot "README.md") $PackageRoot
Copy-Item (Join-Path $ProjectRoot "RUN_ME_FIRST.txt") $PackageRoot
Copy-Item (Join-Path $ProjectRoot "CHANGELOG.md") $PackageRoot
Copy-Item (Join-Path $ProjectRoot "LICENSE-MIT") $PackageRoot
Copy-Item (Join-Path $ProjectRoot "THIRD_PARTY.md") $PackageRoot
Copy-Item (Join-Path $ProjectRoot "docs\KNOWN_LIMITATIONS.md") $PackageRoot
Copy-Item (Join-Path $ProjectRoot "docs\TESTING.md") $PackageRoot
Copy-Item (Join-Path $ProjectRoot "docs\BUILD_STATUS.md") $PackageRoot
Copy-Item (Join-Path $ProjectRoot "docs\DESIGN.md") $PackageRoot
Copy-Item (Join-Path $ProjectRoot "docs\ROADMAP.md") $PackageRoot

Compress-Archive -Path (Join-Path $PackageRoot "*") -DestinationPath $ZipPath -CompressionLevel Optimal

Write-Host ""
Write-Host "Executable: target\release\raw-autotune.exe"
Write-Host "Package:    $ZipPath"
