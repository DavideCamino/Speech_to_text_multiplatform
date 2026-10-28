# Test the actual ZIP after relocation, with no MSYS2 directory in PATH.
param(
    [string]$Archive = "dist/Speech2Text-windows-x86_64.zip",
    [string]$LogDirectory = "dist/windows-smoke"
)
$ErrorActionPreference = "Stop"
$Archive = (Resolve-Path $Archive).Path
New-Item -ItemType Directory -Force -Path $LogDirectory | Out-Null
$LogDirectory = (Resolve-Path $LogDirectory).Path
$Destination = Join-Path ([IO.Path]::GetTempPath()) ("Speech2Text portable test " + [guid]::NewGuid().ToString("N"))
$LauncherProcess = $null
$App = $null
try {
    Expand-Archive -LiteralPath $Archive -DestinationPath $Destination
    $Bundle = Join-Path $Destination "Speech2Text"
    $Executable = Join-Path $Bundle "bin/speech2text.exe"
    $Launcher = Join-Path $Bundle "Start-Speech2Text.cmd"
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    $env:SPEECH2TEXT_NO_PAUSE = "1"
    $Stdout = Join-Path $LogDirectory "stdout.log"
    $Stderr = Join-Path $LogDirectory "stderr.log"
    $LauncherProcess = Start-Process -FilePath "$env:SystemRoot\System32\cmd.exe" `
        -ArgumentList "/d /v:off /c `"`"$Launcher`"`"" `
        -WorkingDirectory ([IO.Path]::GetTempPath()) `
        -RedirectStandardOutput $Stdout -RedirectStandardError $Stderr -PassThru
    $Deadline = (Get-Date).AddSeconds(45)
    do {
        $App = Get-Process -Name speech2text -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -eq $Executable } | Select-Object -First 1
        if ($null -ne $App) {
            $App.Refresh()
            if ($App.MainWindowHandle -ne 0 -and $App.MainWindowTitle -eq "Speech2Text") { break }
        }
        $LauncherProcess.Refresh()
        if ($LauncherProcess.HasExited) { throw "Launcher exited before a window appeared (code $($LauncherProcess.ExitCode))" }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $Deadline)
    if ($null -eq $App -or $App.MainWindowHandle -eq 0 -or $App.MainWindowTitle -ne "Speech2Text") {
        throw "No application window appeared within 45 seconds; inspect Windows smoke-test logs"
    }
    Start-Sleep -Seconds 3
    $App.Refresh()
    if ($App.HasExited -or $App.MainWindowTitle -ne "Speech2Text") {
        throw "Application exited or lost its main window immediately after startup"
    }
    $LoaderCache = Join-Path $env:LOCALAPPDATA "io.github.davidecamino.Speech2Text/cache/loaders.cache"
    if (-not (Test-Path $LoaderCache) -or -not (Select-String -LiteralPath $LoaderCache -Pattern '^"svg"\s' -Quiet)) {
        throw "The relocated runtime did not successfully load its SVG image plugin"
    }
    Write-Host "Portable ZIP opened a window and loaded SVG support after relocation, without MSYS2 in PATH."
    $null = $App.CloseMainWindow()
    if ($App.WaitForExit(5000)) {
        if ($App.ExitCode -ne 0) { throw "Application exited with code $($App.ExitCode)" }
    } else {
        $App.Kill(); $App.WaitForExit()
    }
    if (-not $LauncherProcess.WaitForExit(5000)) { $LauncherProcess.Kill() }
} finally {
    if ($null -ne $App) {
        try { if (-not $App.HasExited) { $App.Kill(); $App.WaitForExit() } } catch { Write-Warning $_ }
    }
    if ($null -ne $LauncherProcess) {
        try { if (-not $LauncherProcess.HasExited) { $LauncherProcess.Kill(); $LauncherProcess.WaitForExit() } } catch { Write-Warning $_ }
    }
    if (Test-Path $Destination) { Remove-Item -LiteralPath $Destination -Recurse -Force }
    foreach ($Name in @("stdout.log", "stderr.log")) {
        $Log = Join-Path $LogDirectory $Name
        if (Test-Path $Log) { Write-Host "--- $Name ---"; Get-Content -LiteralPath $Log }
    }
}
