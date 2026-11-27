[CmdletBinding()]
param(
    [string]$Version = $(if ($env:SUPER_VERSION) { $env:SUPER_VERSION } else { "0.1.0" }),
    [string]$InstallDir = $(if ($env:SUPER_HOME) { $env:SUPER_HOME } else { Join-Path $HOME ".super" }),
    [string]$ReleaseBase = $(if ($env:SUPER_BOOTSTRAP_BASE_URL) { $env:SUPER_BOOTSTRAP_BASE_URL } else { "https://github.com/theHamdiz/super-lang/releases/latest/download" }),
    [switch]$NoPathUpdate
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

if ([Environment]::Is64BitOperatingSystem -ne $true) {
    throw "Super 0.1.0 provides a Windows x86_64 bundle only."
}

if ($PSVersionTable.PSVersion.Major -le 5) {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
}

$target = "x86_64-pc-windows-gnu"
$archive = "super-$Version-$target.zip"
$work = Join-Path ([IO.Path]::GetTempPath()) ("super-install-" + [Guid]::NewGuid().ToString("N"))
$archivePath = Join-Path $work $archive
$sumsPath = Join-Path $work "SHA256SUMS"
$extractPath = Join-Path $work "bundle"

try {
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    Write-Host "Downloading $archive"
    Invoke-WebRequest -UseBasicParsing -Uri "$ReleaseBase/$archive" -OutFile $archivePath
    Invoke-WebRequest -UseBasicParsing -Uri "$ReleaseBase/SHA256SUMS" -OutFile $sumsPath

    $escaped = [Regex]::Escape($archive)
    $line = Get-Content -LiteralPath $sumsPath |
        Where-Object { $_ -match "^(?<hash>[0-9A-Fa-f]{64})\s+\*?$escaped$" } |
        Select-Object -First 1
    if (-not $line) {
        throw "SHA256SUMS has no entry for $archive"
    }
    $expected = ([Regex]::Match($line, "^([0-9A-Fa-f]{64})")).Groups[1].Value.ToLowerInvariant()
    $actual = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "checksum mismatch for $archive"
    }
    Write-Host "Checksum OK"

    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractPath -Force
    $root = Get-ChildItem -LiteralPath $extractPath -Directory | Select-Object -First 1
    if (-not $root) { $rootPath = $extractPath } else { $rootPath = $root.FullName }
    $sourceExe = Join-Path $rootPath "bin\super.exe"
    $sourceStd = Join-Path $rootPath "stdlib"
    if (-not (Test-Path -LiteralPath $sourceExe -PathType Leaf)) {
        throw "bundle missing bin\super.exe"
    }
    if (-not (Test-Path -LiteralPath $sourceStd -PathType Container)) {
        throw "bundle missing stdlib"
    }

    $bin = Join-Path $InstallDir "bin"
    $stdlib = Join-Path $InstallDir "lib\stdlib"
    New-Item -ItemType Directory -Force -Path $bin | Out-Null
    New-Item -ItemType Directory -Force -Path (Split-Path $stdlib) | Out-Null
    Copy-Item -LiteralPath $sourceExe -Destination (Join-Path $bin "super.exe") -Force
    if (Test-Path -LiteralPath $stdlib) {
        Remove-Item -LiteralPath $stdlib -Recurse -Force
    }
    Copy-Item -LiteralPath $sourceStd -Destination $stdlib -Recurse -Force

    if (-not $NoPathUpdate) {
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $parts = @($userPath -split ";" | Where-Object { $_ })
        if ($parts -notcontains $bin) {
            $next = (@($bin) + $parts) -join ";"
            [Environment]::SetEnvironmentVariable("Path", $next, "User")
            Write-Host "Added $bin to the user PATH (open a new terminal)."
        }
    }
    Write-Host "Installed Super $Version to $InstallDir"
}
finally {
    if (Test-Path -LiteralPath $work) {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}
