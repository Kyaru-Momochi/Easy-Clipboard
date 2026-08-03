[CmdletBinding()]
param(
    [string] $InstallerPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ([string]::IsNullOrWhiteSpace($InstallerPath)) {
    $InstallerPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'release\Easy-Clipboard_0.1.0_x64-setup.exe'
}

function Test-ExistingEasyClipboardInstall {
    $installDirectory = Join-Path $env:LOCALAPPDATA 'Easy Clipboard'
    if (Test-Path -LiteralPath $installDirectory) {
        return $true
    }

    $uninstallRoots = @(
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall'
    )
    foreach ($root in $uninstallRoots) {
        if (-not (Test-Path -LiteralPath $root)) { continue }
        foreach ($entry in Get-ChildItem -LiteralPath $root) {
            $properties = Get-ItemProperty -LiteralPath $entry.PSPath -ErrorAction SilentlyContinue
            $displayNameProperty = $properties.PSObject.Properties['DisplayName']
            $displayName = if ($null -eq $displayNameProperty) { $null } else { $displayNameProperty.Value }
            if ($displayName -eq 'Easy Clipboard') { return $true }
        }
    }

    return $false
}

function Wait-ForPath {
    param(
        [Parameter(Mandatory)] [string] $Path,
        [int] $TimeoutSeconds = 30
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path -LiteralPath $Path) { return }
        Start-Sleep -Milliseconds 250
    }
    throw "Timed out waiting for '$Path'."
}

$installer = Get-Item -LiteralPath $InstallerPath
if ($installer.Extension -ne '.exe') {
    throw "InstallerPath must point to an .exe installer: '$InstallerPath'."
}
if (Test-ExistingEasyClipboardInstall) {
    throw 'Refusing installer smoke test: an Easy Clipboard installation may already exist. Remove or relocate it first; this script will not risk overwriting a user installation.'
}

$scratchDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ("easy-clipboard-installer-smoke-" + [guid]::NewGuid())
$copiedInstaller = Join-Path $scratchDirectory $installer.Name
$isolatedLocalAppData = Join-Path $scratchDirectory 'localappdata'
$isolatedAppData = Join-Path $scratchDirectory 'appdata'
$isolatedRepository = Join-Path $scratchDirectory 'isolated-app-data'
$installDirectory = Join-Path $env:LOCALAPPDATA 'Easy Clipboard'
$applicationPath = Join-Path $installDirectory 'easy-clipboard.exe'
$uninstallerPath = Join-Path $installDirectory 'uninstall.exe'
$installedBySmoke = $false
$application = $null

try {
    New-Item -ItemType Directory -Path $scratchDirectory | Out-Null
    Copy-Item -LiteralPath $installer.FullName -Destination $copiedInstaller

    if (Test-ExistingEasyClipboardInstall) {
        throw 'Refusing installer smoke test: an Easy Clipboard installation appeared before install. No installer was run.'
    }

    $install = Start-Process -FilePath $copiedInstaller -ArgumentList '/S' -Wait -PassThru
    if ($install.ExitCode -ne 0) { throw "Silent installer exited with code $($install.ExitCode)." }
    $installedBySmoke = $true
    Wait-ForPath -Path $applicationPath

    New-Item -ItemType Directory -Path $isolatedLocalAppData | Out-Null
    New-Item -ItemType Directory -Path $isolatedAppData | Out-Null
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new($applicationPath)
    $startInfo.UseShellExecute = $false
    $startInfo.EnvironmentVariables['LOCALAPPDATA'] = $isolatedLocalAppData
    $startInfo.EnvironmentVariables['APPDATA'] = $isolatedAppData
    $startInfo.EnvironmentVariables['EASY_CLIPBOARD_DATA_DIR'] = $isolatedRepository
    $application = [System.Diagnostics.Process]::Start($startInfo)
    Start-Sleep -Seconds 5
    if ($application.HasExited) {
        throw "Installed application exited early with code $($application.ExitCode)."
    }
    Wait-ForPath -Path (Join-Path $isolatedRepository 'history.sqlite3')
    $application.Kill()
    $application.WaitForExit()
}
finally {
    $cleanupErrors = @()

    try {
        if ($null -ne $application) {
            try {
                if (-not $application.HasExited) {
                    $application.Kill()
                    $application.WaitForExit()
                }
            }
            finally {
                $application.Dispose()
            }
        }
    }
    catch {
        $cleanupErrors += $_
    }

    try {
        if ($installedBySmoke) {
            if (-not (Test-Path -LiteralPath $uninstallerPath)) {
                throw "Smoke-installed application has no uninstaller at '$uninstallerPath'; refusing to continue."
            }
            $uninstall = Start-Process -FilePath $uninstallerPath -ArgumentList '/S' -Wait -PassThru
            if ($uninstall.ExitCode -ne 0) { throw "Silent uninstaller exited with code $($uninstall.ExitCode)." }
            if (Test-Path -LiteralPath $applicationPath) {
                throw "Uninstall did not remove '$applicationPath'."
            }
        }
    }
    catch {
        $cleanupErrors += $_
    }

    try {
        if (Test-Path -LiteralPath $scratchDirectory) {
            Remove-Item -LiteralPath $scratchDirectory -Recurse -Force
        }
    }
    catch {
        $cleanupErrors += $_
    }

    if ($cleanupErrors.Count -gt 0) {
        throw ('Smoke test cleanup failed: ' + (($cleanupErrors | ForEach-Object { $_.Exception.Message }) -join '; '))
    }
}

Write-Host 'Installer smoke test passed'
