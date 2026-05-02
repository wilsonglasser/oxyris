# Build + bundle Oxyris into an MSI (and NSIS) installer under
# ./release/*. No code signing — if you want a signed artifact, set the
# TAURI_SIGNING_* env vars or integrate Azure Trusted Signing.
#
# Usage:
#   pwsh -NoProfile -File scripts/package.ps1
#
# The output bundles land under:
#   apps/desktop/target/release/bundle/msi/*.msi
#   apps/desktop/target/release/bundle/nsis/*.exe
# and are copied to ./release/ for easy sharing.

$ErrorActionPreference = "Stop"

Write-Host "Building web frontend"
bun install --frozen-lockfile
bun run --cwd apps/web build

Write-Host "Building desktop (release)"
# Use @tauri-apps/cli for a single-command build + bundle.
bun x tauri build

$bundleDir = Join-Path (Get-Location) "apps/desktop/target/release/bundle"
$releaseDir = Join-Path (Get-Location) "release"
New-Item -ItemType Directory -Force -Path $releaseDir | Out-Null

$found = @()
foreach ($pattern in @("msi/*.msi", "nsis/*.exe")) {
    $matches = Get-ChildItem -Path (Join-Path $bundleDir $pattern) -ErrorAction SilentlyContinue
    foreach ($f in $matches) {
        Copy-Item -Force $f.FullName -Destination $releaseDir
        $found += $f.Name
    }
}

if ($found.Count -eq 0) {
    Write-Error "No installer artifacts were produced; check the build output above."
}

Write-Host "Artifacts copied to $releaseDir :"
$found | ForEach-Object { Write-Host "  - $_" }
