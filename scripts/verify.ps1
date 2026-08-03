[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repositoryRoot

function Invoke-VerificationStep {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [scriptblock] $Action
    )

    Write-Host "==> $Name"
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE."
    }
}

Invoke-VerificationStep 'npm ci' { npm ci }
Invoke-VerificationStep 'npm test' { npm test }
Invoke-VerificationStep 'npm run check' { npm run check }
Invoke-VerificationStep 'npm run build' { npm run build }

Push-Location (Join-Path $repositoryRoot 'src-tauri')
try {
    Invoke-VerificationStep 'cargo fmt -- --check' { cargo fmt -- --check }
    Invoke-VerificationStep 'cargo clippy --all-targets -- -D warnings' { cargo clippy --all-targets -- -D warnings }
    Invoke-VerificationStep 'cargo test --all-targets' { cargo test --all-targets }
}
finally {
    Pop-Location
}

Invoke-VerificationStep 'npm run tauri build -- --bundles nsis' {
    npm run tauri build -- --bundles nsis
}

$installerDirectory = Join-Path $repositoryRoot 'src-tauri\target\release\bundle\nsis'
$installers = @(Get-ChildItem -LiteralPath $installerDirectory -Filter '*-setup.exe' -File)
if ($installers.Count -ne 1) {
    throw "Expected exactly one NSIS installer matching *-setup.exe in '$installerDirectory'; found $($installers.Count)."
}

Write-Host "Verified installer: $($installers[0].FullName)"
