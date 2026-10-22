# Creates only a new disposable VM. Run in an elevated host PowerShell.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$IsoPath,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedIsoSha256,
    [string]$Name = 'Revenant-Acceptance-20261007-F64',
    [string]$Directory = 'F:\Revenant-Acceptance-20261007',
    [ValidateRange(16, 2048)][int]$DiskSizeGiB = 64,
    [Nullable[guid]]$ResumeVmId,
    [switch]$LeaveOff
)
$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this reviewed script in elevated PowerShell; Hyper-V setup needs elevation.'
}
$iso = (Resolve-Path -LiteralPath $IsoPath).Path
if ([IO.Path]::GetExtension($iso) -ne '.iso') { throw 'Expected official evaluation ISO media.' }
$hash = (Get-FileHash -LiteralPath $iso -Algorithm SHA256).Hash
if ($hash -ne $ExpectedIsoSha256) { throw 'Evaluation media SHA256 does not match the recorded hash.' }
$resolved = [IO.Path]::GetFullPath($Directory)
$disk = Join-Path $resolved 'acceptance.vhdx'
$diskCapacity = [uint64]$DiskSizeGiB * 1GB
if ($ResumeVmId) {
    if (Test-Path -LiteralPath (Join-Path $resolved 'vm-creation.json')) {
        throw 'This guest setup is already recorded as complete; preserving it.'
    }
    $vm = Get-VM -Id $ResumeVmId -ErrorAction Stop
    if ($vm.Name -ne $Name -or $vm.Generation -ne 2 -or $vm.State -ne 'Off') {
        throw 'Resume requires the named, powered-off generation2 guest.'
    }
    $disks = @(Get-VMHardDiskDrive -VM $vm)
    if ($disks.Count -ne 1 -or [IO.Path]::GetFullPath($disks[0].Path) -ne $disk) {
        throw 'Resume VM disk does not match the explicitly named acceptance directory.'
    }
    if ((Get-VHD -Path $disk).Size -ne $diskCapacity) {
        throw 'Resume disk capacity differs from the requested capacity; preserving it.'
    }
} else {
    if (Get-VM -Name $Name -ErrorAction SilentlyContinue) { throw 'A VM with this name exists; preserving it.' }
    if (Test-Path -LiteralPath $resolved) { throw 'The VM directory exists; choose a new directory.' }
    New-Item -ItemType Directory -Path $resolved | Out-Null
    $vm = New-VM -Name $Name -Generation 2 -MemoryStartupBytes 8GB -NewVHDPath $disk -NewVHDSizeBytes $diskCapacity -Path $resolved
}
Set-VMProcessor -VM $vm -Count 4
Set-VMMemory -VM $vm -DynamicMemoryEnabled $false
if (-not $ResumeVmId) {
    Set-VMKeyProtector -VM $vm -NewLocalKeyProtector
    Enable-VMTPM -VM $vm
}
Set-VMFirmware -VM $vm -EnableSecureBoot On -SecureBootTemplate MicrosoftWindows
$drives = @(Get-VMDvdDrive -VM $vm)
if ($drives.Count -gt 1) { throw 'Unexpected multiple DVD drives; preserving existing attachments.' }
if ($drives.Count -eq 1) {
    if ($drives[0].Path -ne $iso) { throw 'Existing DVD media differs; preserving it.' }
    $dvd = $drives[0]
} else { $dvd = Add-VMDvdDrive -VM $vm -Path $iso -Passthru }
Set-VMFirmware -VM $vm -FirstBootDevice $dvd
# No virtual switch is attached. Networking stays disconnected before installation.
Disconnect-VMNetworkAdapter -VMName $Name
Enable-VMIntegrationService -VM $vm -Name 'Guest Service Interface'
[ordered]@{
    name = $Name; vmId = $vm.Id.ToString(); generation = 2
    memoryBytes = 8GB; processors = 4; diskCapacityBytes = $diskCapacity
    media = $iso; mediaSha256 = $hash
    mediaSource = 'https://www.microsoft.com/en-us/evalcenter/evaluate-windows-11-enterprise'
    networkDisconnected = $true; resumed = [bool]$ResumeVmId; recordedAt = [DateTime]::UtcNow.ToString('o')
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $resolved 'vm-creation.json') -Encoding utf8
if (-not $LeaveOff) { Start-VM -VM $vm }
Write-Output "Created $Name. Open its console (start it if powered off), then install Windows and record the guest baseline."
