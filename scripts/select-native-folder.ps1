<# Acceptance helper for the real Windows folder picker in an owned test app.
   Selects a folder by observed dialog control IDs; never sends global keystrokes. #>
param([Parameter(Mandatory)][int]$AppProcessId, [Parameter(Mandatory)][string]$Folder)
$ErrorActionPreference = 'Stop'
$selectedFolder = (Resolve-Path -LiteralPath $Folder).Path
if (-not (Test-Path -LiteralPath $selectedFolder -PathType Container)) { throw 'Fixture must be an existing folder' }
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class RevenantFolderDialog {
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr window,uint message,IntPtr parameter,string text);
 [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr window,uint message,IntPtr parameter,IntPtr value);
}
'@
$condition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ProcessIdProperty, $AppProcessId)
$windows = [System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $condition)
$pathControl = $null
$selectControl = $null
foreach ($window in $windows) {
    $nodes = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($node in $nodes) {
        if ($node.Current.AutomationId -eq '1152' -and $node.Current.NativeWindowHandle) { $pathControl = $node }
        if ($node.Current.AutomationId -eq '1' -and $node.Current.Name -eq 'Select Folder' -and $node.Current.NativeWindowHandle) { $selectControl = $node }
    }
}
if (-not $pathControl -or -not $selectControl) { throw 'Expected folder dialog controls were not observed in the specified process' }
[void][RevenantFolderDialog]::SendMessage([IntPtr]$pathControl.Current.NativeWindowHandle, 12, [IntPtr]::Zero, $selectedFolder)
[void][RevenantFolderDialog]::SendMessage([IntPtr]$selectControl.Current.NativeWindowHandle, 245, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output 'Selected fixture through the native folder dialog.'
