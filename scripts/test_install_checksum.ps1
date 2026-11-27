$ErrorActionPreference = "Stop"
$root = Join-Path ([IO.Path]::GetTempPath()) ("super-checksum-test-" + [Guid]::NewGuid().ToString("N"))
$archive = "super-0.1.0-x86_64-pc-windows-gnu.zip"

try {
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    [IO.File]::WriteAllBytes((Join-Path $root $archive), [byte[]](1, 2, 3, 4))
    ("{0}  {1}" -f ("0" * 64), $archive) |
        Set-Content -LiteralPath (Join-Path $root "SHA256SUMS") -Encoding Ascii
    $global:SuperChecksumFixtureRoot = $root
    $global:SuperChecksumArchive = $archive

    function global:Invoke-WebRequest {
        param(
            [switch]$UseBasicParsing,
            [string]$Uri,
            [string]$OutFile
        )
        $name = if ($Uri.EndsWith("SHA256SUMS")) { "SHA256SUMS" } else { $global:SuperChecksumArchive }
        Copy-Item -LiteralPath (Join-Path $global:SuperChecksumFixtureRoot $name) -Destination $OutFile
    }

    $rejected = $false
    try {
        & (Join-Path $PSScriptRoot "..\install.ps1") `
            -Version "0.1.0" `
            -InstallDir (Join-Path $root "install") `
            -ReleaseBase "https://checksum.test" `
            -NoPathUpdate
    }
    catch {
        if ($_.Exception.Message -like "*checksum mismatch*") {
            $rejected = $true
        }
        else {
            throw
        }
    }
    if (-not $rejected) {
        throw "corrupt installer checksum was accepted"
    }
    Write-Host "OK: corrupt PowerShell installer checksum rejected"
}
finally {
    Remove-Item function:\Invoke-WebRequest -ErrorAction SilentlyContinue
    Remove-Variable SuperChecksumFixtureRoot -Scope Global -ErrorAction SilentlyContinue
    Remove-Variable SuperChecksumArchive -Scope Global -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
