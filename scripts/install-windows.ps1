#requires -Version 5.1

[CmdletBinding()]
param(
    [ValidateNotNullOrEmpty()]
    [string]$Version = 'latest',

    [ValidateNotNullOrEmpty()]
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\DTTN\bin",

    [ValidatePattern('^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$')]
    [string]$Repository = 'yichensunjoe/DTTN-CLI',

    [string]$ArchivePath,

    [string]$ChecksumPath,

    [switch]$NoPathUpdate
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$assetName = 'dttn-windows-x86_64.zip'
$checksumAssetName = "$assetName.sha256"
$tempRoot = $null

function Resolve-AbsolutePath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    return (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
}

function Get-ReleaseAssets {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Repo,

        [Parameter(Mandatory = $true)]
        [string]$RequestedVersion,

        [Parameter(Mandatory = $true)]
        [string]$Destination
    )

    $headers = @{
        Accept = 'application/vnd.github+json'
        'User-Agent' = 'DTTN-Windows-Installer'
        'X-GitHub-Api-Version' = '2022-11-28'
    }

    $releaseUri = if ($RequestedVersion -eq 'latest') {
        "https://api.github.com/repos/$Repo/releases/latest"
    }
    else {
        $encodedVersion = [Uri]::EscapeDataString($RequestedVersion)
        "https://api.github.com/repos/$Repo/releases/tags/$encodedVersion"
    }

    Write-Host "Resolving DTTN release '$RequestedVersion'..."
    $release = Invoke-RestMethod -Uri $releaseUri -Headers $headers -Method Get

    $archiveAsset = @($release.assets | Where-Object { $_.name -eq $assetName }) | Select-Object -First 1
    $checksumAsset = @($release.assets | Where-Object { $_.name -eq $checksumAssetName }) | Select-Object -First 1

    if ($null -eq $archiveAsset) {
        throw "Release '$($release.tag_name)' does not contain $assetName."
    }
    if ($null -eq $checksumAsset) {
        throw "Release '$($release.tag_name)' does not contain $checksumAssetName."
    }

    $downloadedArchive = Join-Path $Destination $assetName
    $downloadedChecksum = Join-Path $Destination $checksumAssetName

    Write-Host "Downloading $assetName..."
    Invoke-WebRequest -Uri $archiveAsset.browser_download_url -Headers $headers -OutFile $downloadedArchive
    Invoke-WebRequest -Uri $checksumAsset.browser_download_url -Headers $headers -OutFile $downloadedChecksum

    return @{
        Archive = $downloadedArchive
        Checksum = $downloadedChecksum
        Tag = [string]$release.tag_name
    }
}

function Assert-ArchiveChecksum {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Archive,

        [Parameter(Mandatory = $true)]
        [string]$ChecksumFile
    )

    $checksumText = Get-Content -LiteralPath $ChecksumFile -Raw
    $checksumMatch = [regex]::Match($checksumText, '(?im)^\s*([0-9a-f]{64})\b')
    if (-not $checksumMatch.Success) {
        throw "Checksum file '$ChecksumFile' does not contain a SHA-256 digest."
    }

    $expected = $checksumMatch.Groups[1].Value.ToLowerInvariant()
    $actual = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "SHA-256 verification failed for '$Archive'. Expected $expected, got $actual."
    }

    Write-Host 'SHA-256 verification passed.'
}

function Add-InstallDirToUserPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Directory
    )

    $normalizedDirectory = [IO.Path]::GetFullPath($Directory).TrimEnd('\')
    $currentUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $segments = @()
    if (-not [string]::IsNullOrWhiteSpace($currentUserPath)) {
        $segments = @($currentUserPath.Split(';') | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    }

    $alreadyPresent = $false
    foreach ($segment in $segments) {
        try {
            $normalizedSegment = [IO.Path]::GetFullPath($segment.Trim()).TrimEnd('\')
        }
        catch {
            $normalizedSegment = $segment.Trim().TrimEnd('\')
        }

        if ([string]::Equals($normalizedSegment, $normalizedDirectory, [StringComparison]::OrdinalIgnoreCase)) {
            $alreadyPresent = $true
            break
        }
    }

    if (-not $alreadyPresent) {
        $updatedUserPath = if ([string]::IsNullOrWhiteSpace($currentUserPath)) {
            $normalizedDirectory
        }
        else {
            "$currentUserPath;$normalizedDirectory"
        }
        [Environment]::SetEnvironmentVariable('Path', $updatedUserPath, 'User')
        Write-Host "Added '$normalizedDirectory' to the user PATH."
    }
    else {
        Write-Host "'$normalizedDirectory' is already in the user PATH."
    }

    $processSegments = @($env:Path.Split(';'))
    if (-not ($processSegments | Where-Object { [string]::Equals($_.TrimEnd('\'), $normalizedDirectory, [StringComparison]::OrdinalIgnoreCase) })) {
        $env:Path = "$normalizedDirectory;$env:Path"
    }
}

try {
    if ($env:OS -ne 'Windows_NT') {
        throw 'This installer only supports Windows.'
    }
    if (-not [Environment]::Is64BitOperatingSystem) {
        throw 'This release currently supports Windows x64 only.'
    }

    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $resolvedArchive = $null
    $resolvedChecksum = $null
    $resolvedVersion = $Version

    if ([string]::IsNullOrWhiteSpace($ArchivePath)) {
        $tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("dttn-install-" + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null
        $assets = Get-ReleaseAssets -Repo $Repository -RequestedVersion $Version -Destination $tempRoot
        $resolvedArchive = $assets.Archive
        $resolvedChecksum = $assets.Checksum
        $resolvedVersion = $assets.Tag
    }
    else {
        $resolvedArchive = Resolve-AbsolutePath -Path $ArchivePath
        if ([string]::IsNullOrWhiteSpace($ChecksumPath)) {
            $ChecksumPath = "$resolvedArchive.sha256"
        }
        $resolvedChecksum = Resolve-AbsolutePath -Path $ChecksumPath
    }

    Assert-ArchiveChecksum -Archive $resolvedArchive -ChecksumFile $resolvedChecksum

    $extractRoot = Join-Path ([IO.Path]::GetTempPath()) ("dttn-extract-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $extractRoot -Force | Out-Null
    try {
        Expand-Archive -LiteralPath $resolvedArchive -DestinationPath $extractRoot -Force
        $binary = Get-ChildItem -LiteralPath $extractRoot -Filter 'dttn.exe' -File -Recurse | Select-Object -First 1
        if ($null -eq $binary) {
            throw "Archive '$resolvedArchive' does not contain dttn.exe."
        }

        $absoluteInstallDir = [IO.Path]::GetFullPath($InstallDir)
        New-Item -ItemType Directory -Path $absoluteInstallDir -Force | Out-Null

        $destination = Join-Path $absoluteInstallDir 'dttn.exe'
        $stagedDestination = "$destination.new-$PID"
        Copy-Item -LiteralPath $binary.FullName -Destination $stagedDestination -Force

        & $stagedDestination --help | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "Downloaded dttn.exe failed its startup check with exit code $LASTEXITCODE."
        }

        Move-Item -LiteralPath $stagedDestination -Destination $destination -Force

        if (-not $NoPathUpdate) {
            Add-InstallDirToUserPath -Directory $absoluteInstallDir
        }

        Write-Host ''
        Write-Host "DTTN $resolvedVersion installed successfully."
        Write-Host "Executable: $destination"
        if (-not $NoPathUpdate) {
            Write-Host 'Open a new terminal, then run: dttn --help'
        }
    }
    finally {
        Remove-Item -LiteralPath $extractRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
finally {
    if ($null -ne $tempRoot) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
