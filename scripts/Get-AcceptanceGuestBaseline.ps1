# Read-only checks. Run only inside the disposable Hyper-V guest.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$WebViewProbe,
    [string]$Output = 'C:\RevenantAcceptance\baseline.json'
)
$ErrorActionPreference = 'Stop'
$system = Get-CimInstance Win32_ComputerSystem
if ($system.Manufacturer -ne 'Microsoft Corporation' -or $system.Model -ne 'Virtual Machine') {
    throw 'This baseline script is restricted to the disposable Hyper-V guest.'
}
$probe = (Resolve-Path -LiteralPath $WebViewProbe).Path
$runtime = (& $probe | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0 -or $runtime.unsupported -or $runtime.available -isnot [bool]) {
    throw 'WebView runtime discovery probe failed or returned an unsupported result.'
}
if (-not $runtime.available -and [string]::IsNullOrWhiteSpace($runtime.error)) {
    throw 'The runtime absence result must include its discovery error.'
}
$tools = @('cargo', 'rustc', 'node', 'npm', 'vite', 'revenant') | ForEach-Object {
    $command = Get-Command $_ -ErrorAction SilentlyContinue
    [ordered]@{ name = $_; present = [bool]$command }
}
$adapters = @(Get-NetAdapter | Select-Object Name,Status,LinkSpeed)
$baseline = [ordered]@{
    recordedAt = [DateTime]::UtcNow.ToString('o')
    os = Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version,BuildNumber
    hyperVGuest = $true
    webView = $runtime
    probeSha256 = (Get-FileHash -LiteralPath $probe -Algorithm SHA256).Hash
    developmentTools = @($tools)
    adapters = $adapters
    baselinePass = (-not $runtime.available -and -not ($tools | Where-Object present) -and -not ($adapters | Where-Object Status -eq 'Up'))
}
$parent = Split-Path -Parent $Output
if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
$baseline | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $Output -Encoding utf8
if (-not $baseline.baselinePass) { throw "Baseline saved at $Output but its absence/offline requirements did not pass." }
Write-Output "Baseline passed and recorded at $Output"
