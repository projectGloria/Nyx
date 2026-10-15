<#
.SYNOPSIS
    Remove a Nyx installed by install.ps1.

.DESCRIPTION
    Takes out the Explorer context menu, the Start menu shortcut, the PATH entry and the installed
    executable.

    Your settings are left alone. %APPDATA%\Nyx\config.toml is yours — an uninstaller that deletes
    a hand-edited config is an uninstaller that makes reinstalling expensive. Pass -Purge if you
    really do want it gone.

.PARAMETER Purge
    Also delete %APPDATA%\Nyx (your config) and %LOCALAPPDATA%\Nyx (generated shell scripts).

.EXAMPLE
    .\uninstall.ps1
#>
[CmdletBinding()]
param(
    [switch] $Purge
)

$ErrorActionPreference = 'Stop'

$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Nyx'
$target     = Join-Path $installDir 'nyx.exe'

Write-Host "Removing Nyx from $installDir"

# The context menu first, and through the executable while it is still there: it is the only thing
# that knows which registry keys it wrote.
#
# `Start-Process -Wait`, not `& $target`: nyx.exe is GUI-subsystem, so a shell does not wait for it
# and $LASTEXITCODE is never set — and here the very next thing is deleting the file it is running.
if (Test-Path $target) {
    Start-Process -FilePath $target -ArgumentList '--remove-launcher' -Wait -WindowStyle Hidden
    Start-Sleep -Milliseconds 150
    $proc = Start-Process -FilePath $target -ArgumentList '--remove-context-menu' -Wait -PassThru -NoNewWindow
    if ($proc.ExitCode -eq 0) { Write-Host '  Explorer context menu removed' }
    else { Write-Warning "  could not remove the Explorer context menu (exit $($proc.ExitCode))" }
} else {
    Write-Warning "  no nyx.exe at $target; the Explorer entry may need removing by hand"
}

$running = Get-Process -Name 'nyx' -ErrorAction SilentlyContinue |
           Where-Object { $_.Path -eq $target }
if ($running) {
    Write-Error "Nyx is running from $target. Close it and run this again."
}

$shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Nyx.lnk'
if (Test-Path $shortcut) {
    Remove-Item $shortcut -Force
    Write-Host "  Start menu shortcut removed"
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -and ($userPath -split ';' -contains $installDir)) {
    $kept = ($userPath -split ';' | Where-Object { $_ -and $_ -ne $installDir }) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $kept, 'User')
    Write-Host "  removed from PATH"
}

if (Test-Path $installDir) {
    Remove-Item $installDir -Recurse -Force
    Write-Host "  executable removed"
}

if ($Purge) {
    foreach ($dir in @((Join-Path $env:APPDATA 'Nyx'), (Join-Path $env:LOCALAPPDATA 'Nyx'))) {
        if (Test-Path $dir) {
            Remove-Item $dir -Recurse -Force
            Write-Host "  deleted $dir"
        }
    }
} else {
    Write-Host ""
    Write-Host "Your settings are still in $env:APPDATA\Nyx. Pass -Purge to delete them too."
}

Write-Host ""
Write-Host "Done."
