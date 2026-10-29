# Builds the release binary, then packages it into a Windows installer via
# Inno Setup. Needs Inno Setup 6 installed (winget install --id
# JRSoftware.InnoSetup -e). Output: installer\output\rustlight-setup-<version>.exe

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

& "$root\scripts\build-windows.ps1"

$iscc = Get-ChildItem -Path "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $iscc) {
    Write-Error "Inno Setup not found. Install it with: winget install --id JRSoftware.InnoSetup -e"
    exit 1
}

& $iscc.FullName "$root\installer\windows.iss"
