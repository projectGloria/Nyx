<#
.SYNOPSIS
    Install Nyx for the current user.

.DESCRIPTION
    Copies nyx.exe to %LOCALAPPDATA%\Programs\Nyx, puts a shortcut in the Start menu, optionally
    adds "Open Nyx here" to Explorer's context menu, and optionally puts the folder on PATH.

    Per user, and deliberately. Nothing here needs administrator rights: %LOCALAPPDATA% and HKCU
    are both writable by the person running this, which means no UAC prompt, no shared state, and
    an uninstall that cannot leave something behind for another account.

    Nyx keeps its settings in %APPDATA%\Nyx and its generated shell scripts in %LOCALAPPDATA%\Nyx.
    Neither is touched here, so reinstalling over an existing copy keeps your config.

.PARAMETER Source
    The nyx.exe to install. Defaults to target\release\nyx.exe beside this script.

.PARAMETER NoContextMenu
    Skip the Explorer "Open Nyx here" entry.

.PARAMETER AddToPath
    Add the install folder to the user's PATH, so `nyx` works from any shell.

.EXAMPLE
    cargo build --release
    .\install.ps1
#>
[CmdletBinding()]
param(
    [string] $Source,
    [switch] $NoContextMenu,
    [switch] $AddToPath
)

$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $Source) { $Source = Join-Path $here 'target\release\nyx.exe' }

if (-not (Test-Path $Source)) {
    Write-Error "No executable at $Source. Run ``cargo build --release`` first, or pass -Source."
}

$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Nyx'
$target     = Join-Path $installDir 'nyx.exe'

Write-Host "Installing Nyx"
Write-Host "  from $Source"
Write-Host "  to   $installDir"

if (-not (Test-Path $installDir)) {
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
}

Start-Process -FilePath $Source -ArgumentList '--stop-launcher' -Wait -WindowStyle Hidden
Start-Sleep -Milliseconds 200

# A running Nyx holds its own executable open, and the copy fails with a message about the file
# being in use rather than about what to do. Say what to do.
$running = Get-Process -Name 'nyx' -ErrorAction SilentlyContinue |
           Where-Object { $_.Path -eq $target }
if ($running) {
    Write-Error "Nyx is running from $target. Close it and run this again."
}

Copy-Item -Path $Source -Destination $target -Force
Write-Host "  copied nyx.exe"
foreach ($asset in @('icons', 'cursors', 'sounds')) {
    $assetSource = Join-Path (Split-Path -Parent $Source) $asset
    if (Test-Path -LiteralPath $assetSource) {
        Copy-Item -LiteralPath $assetSource -Destination $installDir -Recurse -Force
    }
}

Start-Process -FilePath $target -ArgumentList '--refresh-launcher' -Wait -WindowStyle Hidden

# --- Start menu ------------------------------------------------------------------------------
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$shortcut  = Join-Path $startMenu 'Nyx.lnk'
$shell     = New-Object -ComObject WScript.Shell
$link      = $shell.CreateShortcut($shortcut)
$link.TargetPath       = $target
$link.WorkingDirectory = $env:USERPROFILE
$link.Description      = 'Nyx terminal'
$link.Save()
Write-Host "  Start menu shortcut"

# --- Explorer context menu -------------------------------------------------------------------
# Done by nyx.exe itself rather than by writing the keys here. Two implementations of the same
# registry layout means one of them is quietly wrong, and the one that matters is the one the
# settings window uses.
#
# `Start-Process -Wait`, not `& $target`: nyx.exe is a GUI-subsystem binary, so a shell does not
# wait for it and $LASTEXITCODE is never set. Calling it the obvious way reports success no matter
# what happened.
if (-not $NoContextMenu) {
    $proc = Start-Process -FilePath $target -ArgumentList '--install-context-menu' -Wait -PassThru -NoNewWindow
    if ($proc.ExitCode -eq 0) {
        Write-Host '  "Open Nyx here" added to Explorer (under "Show more options" on Windows 11)'
    } else {
        Write-Warning "  could not add the Explorer context menu (exit $($proc.ExitCode))"
    }
}

# --- PATH ------------------------------------------------------------------------------------
if ($AddToPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath -split ';' -contains $installDir) {
        Write-Host "  already on PATH"
    } else {
        $new = if ([string]::IsNullOrEmpty($userPath)) { $installDir } else { "$userPath;$installDir" }
        [Environment]::SetEnvironmentVariable('Path', $new, 'User')
        Write-Host "  added to PATH (open a new shell to pick it up)"
    }
}

Write-Host ""
Write-Host "Done. Launch it from the Start menu, or:"
Write-Host "    $target"
Write-Host ""
Write-Host "Settings live in $env:APPDATA\Nyx\config.toml, or press Ctrl+, in the terminal."
Write-Host "To remove it again: .\uninstall.ps1"
