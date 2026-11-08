param([string]$Target = "")
$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent
Push-Location $Root
try {
    if (-not $Target) {
        $Target = ((& rustc -vV | Select-String '^host: ').Line -replace '^host: ', '').Trim()
    }
    & cargo build --release --locked -p pc-monitor-host --target $Target
    if ($LASTEXITCODE -ne 0) { throw "Cargo build failed" }
    $Name = if ($Target -match 'windows') { 'pc-monitor-host.exe' } else { 'pc-monitor-host' }
    $Out = Join-Path $Root "output/host/$Target"
    New-Item -ItemType Directory -Force $Out | Out-Null
    Copy-Item "target/$Target/release/$Name" $Out
    Copy-Item README.md $Out
    Copy-Item docs/protocol.md "$Out/PROTOCOL.md"
    if ($Target -match 'linux') { Copy-Item packaging/linux/70-pc-monitor.rules $Out }
    Compress-Archive -Path "$Out/*" -DestinationPath "output/pc-monitor-host-$Target.zip" -Force
    Write-Host "Built $Out/$Name"
} finally { Pop-Location }
