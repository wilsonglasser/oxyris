# Build the Linux x86_64-musl static binary for the Oxyris WSL agent.
#
# Uses Docker Buildx so the output stage can be extracted straight into the
# local filesystem. Run from the repo root:
#
#   pwsh -NoProfile -File scripts/build-agent-linux.ps1
#
# Output: ./dist/agent/oxyris-agent
#
# To wire this into a running Oxyris app, set the env var before launching:
#
#   $env:OXYRIS_AGENT_BIN_PATH = (Resolve-Path .\dist\agent\oxyris-agent)
#   bun tauri dev

$ErrorActionPreference = "Stop"

$dockerfile = "docker/agent.Dockerfile"
$outDir     = "./dist/agent"

if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    Write-Error "Docker not found. Install Docker Desktop and try again."
}

# Ensure buildx is available (required for --output=type=local).
docker buildx inspect default *> $null
if ($LASTEXITCODE -ne 0) {
    docker buildx create --name oxyris-builder --use | Out-Null
}

if (Test-Path $outDir) {
    Remove-Item -Recurse -Force $outDir
}
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

Write-Host "Building oxyris-agent (Linux x86_64-musl) via Docker…"
docker buildx build `
    --file $dockerfile `
    --target export `
    --output "type=local,dest=$outDir" `
    --platform linux/amd64 `
    .

if ($LASTEXITCODE -ne 0) {
    Write-Error "Docker build failed."
}

$bin = Join-Path $outDir "oxyris-agent"
if (-not (Test-Path $bin)) {
    Write-Error "Expected binary at $bin was not produced."
}

$resolved = (Resolve-Path $bin).Path
$size = (Get-Item $bin).Length
Write-Host "✔ Built: $resolved ($([Math]::Round($size / 1MB, 2)) MB)"
Write-Host ""
Write-Host "To use it, set the env var before launching Oxyris:"
Write-Host "  `$env:OXYRIS_AGENT_BIN_PATH = `"$resolved`""
