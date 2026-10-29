# Validate silent installation, installed app startup, and uninstallation.
$ErrorActionPreference = 'Stop'
$Installer = @(Get-ChildItem dist/*-setup.exe)
if ($Installer.Count -ne 1) { throw 'Expected exactly one installer' }
$Destination = Join-Path $env:TEMP ('Speech2Text install test ' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force dist/windows-installer-smoke | Out-Null
$Log = Join-Path (Resolve-Path dist/windows-installer-smoke).Path 'install.log'
try {
    $Process = Start-Process -FilePath $Installer[0].FullName -Wait -PassThru -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-',
        "/DIR=`"$Destination`"", "/LOG=`"$Log`""
    )
    if ($Process.ExitCode -ne 0) { throw "Installer failed: $($Process.ExitCode)" }
    if (-not (Test-Path "$Destination/bin/speech2text.exe")) { throw 'Executable was not installed' }
    & "$PSScriptRoot/smoke-test.ps1" -BundleDirectory $Destination -LogDirectory dist/windows-installer-smoke
} finally {
    $Uninstaller = Join-Path $Destination 'unins000.exe'
    if (Test-Path $Uninstaller) {
        $Process = Start-Process -FilePath $Uninstaller -Wait -PassThru -ArgumentList '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART'
        if ($Process.ExitCode -ne 0) { throw "Uninstaller failed: $($Process.ExitCode)" }
        if (Test-Path "$Destination/bin/speech2text.exe") { throw 'Uninstall left the app executable behind' }
    }
    if (Test-Path $Destination) { Remove-Item -LiteralPath $Destination -Recurse -Force }
}
