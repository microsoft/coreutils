@{
    Rules = @{
        PSUseCompatibleCommands                     = @{
            Enable         = $true
            TargetProfiles = @('win-8_x64_10.0.17763.0_5.1.17763.316_x64_4.0.30319.42000_framework')
        }
        PSUseCompatibleSyntax                       = @{
            Enable         = $true
            TargetVersions = @('5.1', '7.0')
        }
        PSAvoidOverwritingBuiltInCmdlets            = @{ Enable = $false }
        PSUseShouldProcessForStateChangingFunctions = @{ Enable = $false }
        PSUseSingularNouns                          = @{ Enable = $false }
    }
}
