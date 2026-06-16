# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

# A typical test invocation looks like this (as admin):
#  .\src\pwsh-install.ps1 -Action Install -Scope CurrentUser -CmdDir 'C:\Program Files\coreutils\cmd'

param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Install', 'Uninstall', 'Refresh')]
    [string]$Action,
    [ValidateSet('AllUsers', 'CurrentUser')]
    [string]$Scope = 'AllUsers',
    [string]$CmdDir = ''
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

if ([Console]::IsOutputRedirected) {
    $PSStyle.OutputRendering = 'PlainText'
}

$SectionMarker = '60b36fc6-2d59-49df-be51-28dd2f4c3c9a'
$MarkerLine = "# DO NOT MODIFY -- coreutils -- $SectionMarker"
# Earliest PowerShell that supports PSNativeCommandPreserveBytePipe.
$MinPwshVersion = [version]'7.4.0'
$CoreutilsRegPath = 'HKLM:\SOFTWARE\Microsoft\coreutils'
# A REG_MULTI_SZ list of utility names (e.g. "ls") that the user has disabled.
$DisabledUtilitiesRegName = 'DisabledUtilities'
# Contains SID --> Microsoft.PowerShell_profile.ps1 mappings,
# such that we can clean them up on uninstall.
$ProfilesRegPath = 'HKLM:\SOFTWARE\Microsoft\coreutils\PowerShellProfiles'

function Remove-FileIfExists([string]$Path) {
    Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue -ErrorVariable removeErrors
    foreach ($e in $removeErrors) {
        if ($e.CategoryInfo.Category -ne 'ObjectNotFound') {
            throw $e
        }
    }
}

function Get-EnabledCoreutilsAliases([string]$CmdDir) {
    [string[]]$disabled = @()
    if ($props = Get-ItemProperty -LiteralPath $CoreutilsRegPath -Name $DisabledUtilitiesRegName -ErrorAction Ignore) {
        $disabled = $props.$DisabledUtilitiesRegName
    }
    $disabled = [System.Collections.Generic.HashSet[string]]::new($disabled, [System.StringComparer]::OrdinalIgnoreCase)

    $aliases = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($file in Get-ChildItem -LiteralPath $CmdDir -Filter '*.cmd' -File -ErrorAction Ignore) {
        $alias = [System.IO.Path]::GetFileNameWithoutExtension($file.Name)
        if ($alias -and !$disabled.Contains($alias)) {
            [void]$aliases.Add($alias)
        }
    }

    # coreutils-manager is a hidden utility. Theoretically it should not be in the cmd
    # directory at all (it's only exposed as coreutils-manager.exe), but just in case...
    [void]$aliases.Remove('coreutils-manager')

    if ($aliases.Contains('ls')) {
        [void]$aliases.Add('la')
    }

    return @($aliases | Sort-Object)
}

function Get-InjectedSection([string]$CmdDir) {
    $templatePath = Join-Path $PSScriptRoot 'pwsh-install-template.ps1'
    $template = Get-Content -LiteralPath $templatePath -Raw
    $cmdDir = [System.IO.Path]::GetFullPath($CmdDir).TrimEnd('\') + '\'
    $aliases = (Get-EnabledCoreutilsAliases $cmdDir | ForEach-Object { "'$_'" }) -join ','
    $template = $template.Replace("'!!COREUTILS!!'", $aliases)
    $template = $template.Replace("!!CMDDIR!!", $cmdDir.Replace("'", "''"))
    $body = $template.TrimEnd("`r", "`n")
    return "$MarkerLine`r`n$body`r`n$MarkerLine"
}

function Update-PowerShellProfile([string]$Path, [bool] $Install, [bool] $UseBom, [string]$Section, [bool]$RefreshOnly) {
    $parent = Split-Path -LiteralPath $Path
    if ($Install -and !$RefreshOnly) {
        [void](New-Item -Path $parent -ItemType Directory -Force)
    }
    elseif (!(Test-Path -LiteralPath $Path)) {
        return
    }

    # Get-Content uses .NET's StreamReader, so it auto-detects UTF-8/UTF-16 with BOM.
    $text = Get-Content -LiteralPath $Path -Raw -ErrorAction Ignore
    if (!$text) {
        $text = ''
    }

    # Validate marker count: must be 0 (no existing section) or 2 (a complete section).
    $marker = [regex]::Escape($SectionMarker)
    $markerCount = ([regex]::Matches($text, $marker)).Count
    if ($markerCount -ne 0 -and $markerCount -ne 2) {
        throw "Invalid coreutils section markers in PowerShell profile: $Path"
    }
    if ($RefreshOnly -and $markerCount -eq 0) {
        return
    }

    # Strip the existing section (markers + content + any surrounding blank lines) in one shot.
    if ($markerCount -eq 2) {
        $blockRegex = "(?s)(\r?\n)*[^\r\n]*$marker[^\r\n]*\r?\n.*?\r?\n[^\r\n]*$marker[^\r\n]*(\r?\n)*"
        $text = [regex]::Replace($text, $blockRegex, "`r`n`r`n", 1)
    }
    $text = $text.Trim("`r", "`n")

    if ($Install) {
        if ($text) {
            $text += "`r`n`r`n"
        }
        $text += $Section
    }

    if (!$text) {
        Remove-FileIfExists $Path
        return
    }

    $text += "`r`n"
    $encoding = [System.Text.UTF8Encoding]::new($UseBom)

    # Atomic write: stage as .new, then replace.
    $newPath = "$Path.new"
    try {
        [System.IO.File]::WriteAllText($newPath, $text, $encoding)
        [System.IO.File]::Move($newPath, $Path, $true)
    }
    catch {
        Remove-Item -LiteralPath $newPath -Force -ErrorAction Ignore
        throw
    }
}

function Get-MsiPwshProfilePaths {
    Get-ChildItem -LiteralPath 'HKLM:\SOFTWARE\Microsoft\PowerShellCore\InstalledVersions' -ErrorAction Ignore | ForEach-Object {
        $props = Get-ItemProperty -LiteralPath $_.PSPath -ErrorAction Ignore
        if (!$props -or !$props.PSObject.Properties['InstallLocation'] -or !$props.PSObject.Properties['SemanticVersion']) {
            return
        }

        try {
            $version = [version](($props.SemanticVersion -split '[-+ ]', 2)[0])
        }
        catch {
            return
        }

        if ($version -lt $MinPwshVersion) {
            return
        }

        Join-Path $props.InstallLocation 'Microsoft.PowerShell_profile.ps1'
    }
}

function Get-CurrentSid {
    return [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
}

function Get-SidProfileRoot([string]$Sid) {
    $key = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\$Sid"
    $prop = Get-ItemProperty -LiteralPath $key -Name ProfileImagePath -ErrorAction Ignore
    if (!$prop) {
        return $null
    }
    return [Environment]::ExpandEnvironmentVariables($prop.ProfileImagePath)
}

function Get-RecordedProfiles {
    $key = Get-Item -LiteralPath $ProfilesRegPath -ErrorAction Ignore
    if (!$key) {
        return @()
    }

    $result = @()
    foreach ($name in $key.GetValueNames()) {
        if (!$name) {
            continue
        }

        $resolved = [string]$key.GetValue($name)
        if (!$resolved) {
            continue
        }

        # The user may have configured the Documents directory to point to outside the
        # $USERPROFILE and in that case we're storing an absolute path in the registry.
        if (![System.IO.Path]::IsPathRooted($resolved)) {
            $base = Get-SidProfileRoot $name
            if (!$base) {
                continue
            }
            $resolved = Join-Path $base $resolved
        }

        $result += [PSCustomObject]@{
            Sid  = $name
            Path = $resolved
        }
    }

    return $result
}

function Save-RecordedProfile([string]$Sid, [string]$Value) {
    [void](New-Item -Path $ProfilesRegPath -Force)
    [void](New-ItemProperty -LiteralPath $ProfilesRegPath -Name $Sid -Value $Value -PropertyType String -Force)
}

function Remove-RecordedProfile([string]$Sid) {
    Remove-ItemProperty -LiteralPath $ProfilesRegPath -Name $Sid -ErrorAction Ignore
}

function Get-ProfilePlan([bool] $Install, [string]$Scope) {
    $allUsersInstall = $Install -and $Scope -eq 'AllUsers'
    $currentUserInstall = $Install -and $Scope -eq 'CurrentUser'
    $plan = @{}

    function Add([string]$Path) {
        if (!$Path) {
            return $null
        }

        $existing = $plan[$Path]
        if ($existing) {
            return $existing
        }

        $obj = [PSCustomObject]@{
            Path        = $Path
            Install     = $false
            RecordSid   = $null
            RecordValue = $null
        }
        $plan[$Path] = $obj
        return $obj
    }

    foreach ($profilePath in Get-MsiPwshProfilePaths) {
        $entry = Add $profilePath
        if ($allUsersInstall) {
            $entry.Install = $true
        }
    }

    foreach ($r in Get-RecordedProfiles) {
        $entry = Add $r.Path
        # Tag the entry with its existing record SID so the main loop can drop
        # the record when the entry isn't being (re-)installed. CurrentUser
        # install below may overwrite this with the running user's SID.
        $entry.RecordSid = $r.Sid
    }

    $livePath = $PROFILE.CurrentUserCurrentHost
    $entry = Add $livePath

    if ($currentUserInstall) {
        $entry.Install = $true
        $entry.RecordSid = Get-CurrentSid
        $userProfile = $env:USERPROFILE.TrimEnd('\') + '\'
        $entry.RecordValue = if ($livePath.StartsWith($userProfile, [StringComparison]::OrdinalIgnoreCase)) {
            $livePath.Substring($userProfile.Length)
        }
        else {
            $livePath
        }
    }

    return $plan.Values
}

function Get-RefreshProfilePlan {
    $plan = @{}

    function Add([string]$Path) {
        if (!$Path) {
            return
        }
        if ($plan[$Path]) {
            return
        }

        $plan[$Path] = [PSCustomObject]@{
            Path        = $Path
            Install     = $true
            RecordSid   = $null
            RecordValue = $null
        }
    }

    foreach ($profilePath in Get-MsiPwshProfilePaths) {
        Add $profilePath
    }
    foreach ($r in Get-RecordedProfiles) {
        Add $r.Path
    }
    Add $PROFILE.CurrentUserCurrentHost

    return $plan.Values
}

$install = $Action -eq 'Install'
$refresh = $Action -eq 'Refresh'
$plan = if ($refresh) {
    @(Get-RefreshProfilePlan)
}
else {
    @(Get-ProfilePlan $install $Scope)
}
$section = if ($install -or $refresh) {
    Get-InjectedSection $CmdDir
}
else {
    ''
}

foreach ($entry in $plan) {
    Update-PowerShellProfile -Path $entry.Path -Install $entry.Install -UseBom $false -Section $section -RefreshOnly $refresh
}

# Only adjust records once every Update succeeded. A failure mid-loop leaves
# the old records intact so a re-run re-discovers the same paths and retries
# the cleanup/install.
if (!$refresh) {
    foreach ($entry in $plan) {
        if (!$entry.RecordSid) {
            continue
        }
        if ($entry.Install) {
            Save-RecordedProfile $entry.RecordSid $entry.RecordValue
        }
        else {
            Remove-RecordedProfile $entry.RecordSid
        }
    }
}
