<#
.SYNOPSIS
Creates a local command-link layout for testing Coreutils for Windows.

.DESCRIPTION
Creates bin\<utility>.exe and cmd\<utility>.cmd entries that point at a built
coreutils.exe. Hard links are used first because they do not require elevation
on NTFS. If hard links fail, the default fallback is to copy the binary.
#>

param(
    [string]$Coreutils = (Join-Path $PSScriptRoot '..\target\debug\coreutils.exe'),
    [string]$OutputDir = (Join-Path $PSScriptRoot '..\artifacts\dev-layout'),
    [ValidateSet('Copy', 'Symlink', 'None')]
    [string]$Fallback = 'Copy',
    [switch]$Force
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

function Resolve-FullPath([string]$Path) {
    return [System.IO.Path]::GetFullPath($Path)
}

function Remove-ExistingFile([string]$Path) {
    if (Test-Path -LiteralPath $Path) {
        if (-not $Force) {
            throw "Target already exists: $Path. Pass -Force to recreate it."
        }
        Remove-Item -LiteralPath $Path -Force
    }
}

function New-CommandLink {
    param(
        [string]$LinkPath,
        [string]$TargetPath
    )

    Remove-ExistingFile $LinkPath

    try {
        New-Item -ItemType HardLink -Path $LinkPath -Target $TargetPath | Out-Null
        return 'HardLink'
    }
    catch {
        if ($Fallback -eq 'None') {
            throw
        }
    }

    if ($Fallback -eq 'Symlink') {
        New-Item -ItemType SymbolicLink -Path $LinkPath -Target $TargetPath | Out-Null
        return 'Symlink'
    }

    Copy-Item -LiteralPath $TargetPath -Destination $LinkPath
    return 'Copy'
}

$coreutilsPath = Resolve-FullPath $Coreutils
if (-not (Test-Path -LiteralPath $coreutilsPath)) {
    throw "coreutils.exe not found: $coreutilsPath"
}

$outputPath = Resolve-FullPath $OutputDir
$binDir = Join-Path $outputPath 'bin'
$cmdDir = Join-Path $outputPath 'cmd'
New-Item -ItemType Directory -Force -Path $binDir, $cmdDir | Out-Null

$utilities = & $coreutilsPath --list
if ($LASTEXITCODE -ne 0) {
    throw "Failed to run '$coreutilsPath --list'."
}

$created = @{}
$skipped = @()
foreach ($utility in $utilities) {
    $name = $utility.Trim()
    if ($name.Length -eq 0) {
        continue
    }

    # Keep the development layout aligned with the installer. The binary maps
    # "[" to test, but the installer does not currently create [.exe/[.cmd.
    if ($name -eq '[') {
        $skipped += $name
        continue
    }

    foreach ($entry in @(
        @{ Dir = $binDir; Extension = '.exe' },
        @{ Dir = $cmdDir; Extension = '.cmd' }
    )) {
        $linkPath = Join-Path $entry.Dir "$name$($entry.Extension)"
        $kind = New-CommandLink -LinkPath $linkPath -TargetPath $coreutilsPath
        $created[$kind] = 1 + ($created[$kind] ?? 0)
    }
}

[PSCustomObject]@{
    Coreutils = $coreutilsPath
    OutputDir = $outputPath
    BinDir = $binDir
    CmdDir = $cmdDir
    Created = $created
    Skipped = $skipped
}
