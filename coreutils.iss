#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{84814416-E113-451B-B34C-95A3B4A259A1}
AppName=Coreutils for Windows
DefaultGroupName=Coreutils for Windows
AppVersion={#AppVersion}
AppPublisher=Microsoft Corporation
AppPublisherURL=https://github.com/microsoft/coreutils
AppSupportURL=https://github.com/microsoft/coreutils
AppUpdatesURL=https://github.com/microsoft/coreutils
SetupMutex=coreutils-windows-setup
DefaultDirName={autopf}\coreutils
DisableDirPage=yes
DisableProgramGroupPage=yes
SetupIconFile=src\coreutils.ico
UninstallDisplayIcon={app}\coreutils.exe
MinVersion=10.0
ArchitecturesAllowed={#ArchitecturesAllowed}
ArchitecturesInstallIn64BitMode={#ArchitecturesAllowed}
PrivilegesRequired=admin
ChangesEnvironment=yes
SolidCompression=yes
WizardStyle=modern dynamic
OutputBaseFilename=coreutils

#ifdef SignedUninstallerDir
SignedUninstaller=yes
SignedUninstallerDir={#SignedUninstallerDir}
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "addtopath"; Description: "Add to system &PATH"
Name: "corefind"; Description: "find (may break Batch scripts)"; GroupDescription: "Prefer Coreutils- over DOS-style behavior when the invocation is ambiguous for:"
Name: "coresort"; Description: "sort (may break non-UTF8 Batch scripts)"; GroupDescription: "Prefer Coreutils- over DOS-style behavior when the invocation is ambiguous for:"

[Files]
Source: "src\pwsh-install.ps1"; DestDir: "{app}"; Flags: notimestamp ignoreversion
Source: "src\pwsh-install-template.ps1"; DestDir: "{app}"; Flags: notimestamp ignoreversion
Source: "src\_why_is_this_700MB_.txt"; DestDir: "{app}\bin"; Flags: notimestamp ignoreversion
Source: "src\_why_is_this_700MB_.txt"; DestDir: "{app}\cmd"; Flags: notimestamp ignoreversion
Source: {#Source}; DestDir: "{app}"; DestName: "coreutils.exe"; Flags: notimestamp ignoreversion

; Just in case, ensure that the install dir is in a clean state.
; This also ensures that coreutils is not currently being used. :)
[InstallDelete]
Type: filesandordirs; Name: "{app}\*"

[UninstallDelete]
Type: filesandordirs; Name: "{app}"

[Registry]
Root: HKLM; Subkey: "SOFTWARE\Microsoft\coreutils"; ValueType: none; ValueName: ""; Flags: uninsdeletekey
Root: HKLM; Subkey: "SOFTWARE\Microsoft\coreutils"; ValueType: dword; ValueName: "DefaultFind"; ValueData: "0"; Tasks: not corefind; Flags: uninsdeletekey
Root: HKLM; Subkey: "SOFTWARE\Microsoft\coreutils"; ValueType: dword; ValueName: "DefaultFind"; ValueData: "1"; Tasks: corefind; Flags: uninsdeletekey
Root: HKLM; Subkey: "SOFTWARE\Microsoft\coreutils"; ValueType: dword; ValueName: "DefaultSort"; ValueData: "0"; Tasks: not coresort; Flags: uninsdeletekey
Root: HKLM; Subkey: "SOFTWARE\Microsoft\coreutils"; ValueType: dword; ValueName: "DefaultSort"; ValueData: "1"; Tasks: coresort; Flags: uninsdeletekey

[Code]
function CreateHardLink(lpFileName, lpExistingFileName: String; lpSecurityAttributes: LongWord): Boolean;
external 'CreateHardLinkW@kernel32.dll stdcall';

const
    ENV_KEY = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
    PWSH_SCOPE_NONE = 0;
    PWSH_SCOPE_CURRENTUSER = 1;
    PWSH_SCOPE_ALLUSERS = 2;

var
    g_AppDirPath: String;
    g_AppBinDirPath: String;
    g_AppCmdDirPath: String;
    g_CoreutilsExePath: String;
    g_PowerShellPage: TInputOptionWizardPage;
    g_PowerShellCanInstallForAllUsers: Boolean;
    g_HasUsablePowerShell: Boolean;
    g_PowerShellExecutionPolicy: String;
    g_HasSupportedPowerShellExecutionPolicy: Boolean;

procedure InitializeGlobals;
begin
    g_AppDirPath := ExpandConstant('{app}\');
    g_AppBinDirPath := ExpandConstant('{app}\bin\');
    g_AppCmdDirPath := ExpandConstant('{app}\cmd\');
    g_CoreutilsExePath := ExpandConstant('{app}\coreutils.exe');
end;

procedure CreateHardlinks;
var
    Output: TExecOutput;
    Name: String;
    ResultCode, I: Integer;
begin
    ForceDirectories(g_AppBinDirPath);
    ForceDirectories(g_AppCmdDirPath);

    if (not ExecAndCaptureOutput(g_CoreutilsExePath, '--list', '', SW_SHOWNORMAL, ewWaitUntilTerminated, ResultCode, Output)) or (ResultCode <> 0) then
        RaiseException('Failed to execute coreutils.exe --list');

    for I := 0 to GetArrayLength(Output.StdOut) - 1 do
    begin
        Name := Trim(Output.StdOut[I]);
        if (Name <> '') and (Name <> '[') then
        begin
            if not CreateHardLink(g_AppBinDirPath + Name + '.exe', g_CoreutilsExePath, 0) then
                RaiseException('Failed to create hardlink for ' + Name);
            if not CreateHardLink(g_AppCmdDirPath + Name + '.cmd', g_CoreutilsExePath, 0) then
                RaiseException('Failed to create hardlink for ' + Name);
        end;
    end;
end;

procedure ModifyPath(Install: Boolean);
var
    PathsBefore, PathsAfter: TArrayOfString;
    PathsStringBefore: String;
    I, Count, System32Index, AppPathIndex: Integer;
begin
    if not RegQueryStringValue(HKLM, ENV_KEY, 'Path', PathsStringBefore) then
        RaiseException('Failed to read system PATH');

    PathsBefore := StringSplit(PathsStringBefore, [';'], stExcludeEmpty);
    if GetArrayLength(PathsBefore) = 0 then
        RaiseException('Failed to parse system PATH');

    System32Index := -1;
    AppPathIndex := -1;
    if Install then
    begin
        // Find the index of System32.
        System32Index := 0;
        for I := 0 to GetArrayLength(PathsBefore) - 1 do
        begin
            if PathStartsWith(PathsBefore[I], '%SystemRoot%\System32', True) or
            PathStartsWith(PathsBefore[I], ExpandConstant('{sysnative}'), True) then
            begin
                System32Index := I;
                Break;
            end;
        end;

        // Find the index of our app path, if any. We want to retain the same
        // index between installations. Unless it was previously past System32.
        // We want it to be always before System32.
        AppPathIndex := 0;
        for I := 0 to System32Index - 1 do
        begin
            if PathStartsWith(PathsBefore[I], g_AppDirPath, True) then
            begin
                AppPathIndex := I;
                Break;
            end;
        end;
    end;

    // Remove any and all paths pointing to our app.
    // This doubles as an uninstall path.
    SetArrayLength(PathsAfter, GetArrayLength(PathsBefore) + 1);
    Count := 0;
    for I := 0 to GetArrayLength(PathsBefore) - 1 do
    begin
        if I = AppPathIndex then
        begin
            PathsAfter[Count] := RemoveBackslashUnlessRoot(g_AppBinDirPath);
            Count := Count + 1;
        end;
        if not PathStartsWith(PathsBefore[I], g_AppDirPath, True) then
        begin
            PathsAfter[Count] := PathsBefore[I];
            Count := Count + 1;
        end;
    end;
    SetArrayLength(PathsAfter, Count);

    if not RegWriteExpandStringValue(HKLM, ENV_KEY, 'Path', StringJoin(';', PathsAfter)) then
        RaiseException('Failed to write system PATH');
end;

function HasMsiPowerShell: Boolean;
var
    Names: TArrayOfString;
begin
    Result := RegGetSubkeyNames(HKLM, 'SOFTWARE\Microsoft\PowerShellCore\InstalledVersions', Names) and (GetArrayLength(Names) > 0)
end;

function HasMsixPowerShell: Boolean;
var
    Names: TArrayOfString;
    I: Integer;
begin
    Result := False;
    if not RegGetSubkeyNames(HKCU, 'Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages', Names) then
        Exit;
    for I := 0 to GetArrayLength(Names) - 1 do
    begin
        if PathStartsWith(Names[I], 'Microsoft.PowerShell', False) then
        begin
            Result := True;
            Exit;
        end;
    end;
end;

procedure DetectPowerShell;
var
    VerParts: TArrayOfString;
    Params: String;
    ResultCode, Major, Minor: Integer;
    Output: TExecOutput;
    Version: String;
begin
    g_PowerShellCanInstallForAllUsers := HasMsiPowerShell and (not HasMsixPowerShell);
    g_HasUsablePowerShell := False;
    g_PowerShellExecutionPolicy := '';
    g_HasSupportedPowerShellExecutionPolicy := False;

    Params := '-NoProfile -NonInteractive -Command "$PSVersionTable.PSVersion.ToString(); Get-ExecutionPolicy"';
    if (not ExecAndCaptureOutput('pwsh.exe', Params, '', SW_SHOWNORMAL, ewWaitUntilTerminated, ResultCode, Output)) or
       (ResultCode <> 0) or
       (GetArrayLength(Output.StdOut) < 2)
    then
        Exit;

    Version := Trim(Output.StdOut[0]);
    VerParts := StringSplit(Version, ['.'], stExcludeEmpty);
    if GetArrayLength(VerParts) < 2 then
        Exit;
    Major := StrToIntDef(VerParts[0], 0);
    Minor := StrToIntDef(VerParts[1], 0);
    g_HasUsablePowerShell := (Major > 7) or ((Major = 7) and (Minor >= 4));

    g_PowerShellExecutionPolicy := Trim(Output.StdOut[1]);
    g_HasSupportedPowerShellExecutionPolicy := (g_PowerShellExecutionPolicy = 'Unrestricted') or (g_PowerShellExecutionPolicy = 'RemoteSigned') or (g_PowerShellExecutionPolicy = 'Bypass');
    g_HasUsablePowerShell := g_HasUsablePowerShell and (g_PowerShellExecutionPolicy <> '');
end;

procedure RunPwshScript(const ExtraParams: String);
var
    Output: TExecOutput;
    Params, Detail: String;
    ResultCode: Integer;
begin
    Params := '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File ' + AddQuotes(g_AppDirPath + 'pwsh-install.ps1') + ' ' + ExtraParams;
    if not ExecAndCaptureOutput('pwsh.exe', Params, '', SW_SHOWNORMAL, ewWaitUntilTerminated, ResultCode, Output) then
        Exit;

    if ResultCode <> 0 then
    begin
        Detail := '';
        if GetArrayLength(Output.StdErr) > 0 then
            Detail := Output.StdErr[0]
        else if GetArrayLength(Output.StdOut) > 0 then
            Detail := Output.StdOut[0];

        if Detail <> '' then
            RaiseException('Failed to update PowerShell profiles: ' + Detail)
        else
            RaiseException('Failed to update PowerShell profiles');
    end;
end;

procedure InstallPowerShellProfiles(Scope: Integer);
var
    Params: String;
begin
    Params := '-Action Uninstall';

    if Scope <> PWSH_SCOPE_NONE then
    begin
        if Scope = PWSH_SCOPE_ALLUSERS then
            Params := '-Action Install -Scope AllUsers'
        else
            Params := '-Action Install -Scope CurrentUser';
        Params := Params + ' -CmdDir ' + AddQuotes(RemoveBackslashUnlessRoot(g_AppCmdDirPath));
    end;

    RunPwshScript(Params);
end;

// #### Event Handlers ####

procedure InitializeWizard;
var
    Description, Policy: String;
begin
    DetectPowerShell;

    Description :=
        'Coreutils needs a small profile snippet to work inside PowerShell. ' +
        'Without it, PowerShell mangles argument quoting and globbing, so most Coreutils commands wouldn''t behave correctly. ' +
        'The snippet also overrides PowerShell''s built-in aliases (cat, cp, ls, ...) so they resolve to the Coreutils versions.' + #13#10#13#10 +
        'PowerShell 7.4 or newer is required (it requires PSNativeCommandPreserveBytePipe).';

    if not g_HasSupportedPowerShellExecutionPolicy then
    begin
        Policy := g_PowerShellExecutionPolicy;
        if Policy = '' then
            Policy := 'unknown';

        Description := Description + #13#10#13#10 +
            'Warning: Your execution policy is set to ' + Policy + ' and will prevent the integration from working.';
    end;

    Description := Description + #13#10 + #13#10 + 'Choose where to install the snippet:';

    g_PowerShellPage := CreateInputOptionPage(
        wpSelectTasks,
        'PowerShell Integration',
        'Integrate Coreutils with PowerShell?',
        Description,
        True,  // exclusive radio buttons
        False
    );
    g_PowerShellPage.Add('&Do not integrate with PowerShell');
    g_PowerShellPage.Add('Install for the &current user only');
    g_PowerShellPage.Add('Install for &all users (modifies the system-wide PowerShell profile)');

    if not g_PowerShellCanInstallForAllUsers then
        g_PowerShellPage.CheckListBox.ItemEnabled[PWSH_SCOPE_ALLUSERS] := False;

    if g_HasSupportedPowerShellExecutionPolicy then
        g_PowerShellPage.SelectedValueIndex := PWSH_SCOPE_CURRENTUSER
    else
        g_PowerShellPage.SelectedValueIndex := PWSH_SCOPE_NONE;
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
    Result := (PageID = g_PowerShellPage.ID) and (not g_HasUsablePowerShell);
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
    Msg: String;
begin
    Result := True;
    if (CurPageID = g_PowerShellPage.ID)
        and (g_PowerShellPage.SelectedValueIndex = PWSH_SCOPE_CURRENTUSER)
        and g_PowerShellCanInstallForAllUsers then
    begin
        Msg :=
            'Only the profile of the user running this installer will be modified. ' +
            'Other accounts on this machine will each need to re-run the installer (or pick the all-users option) to get it.' + #13#10#13#10 +
            'Continue with the current-user installation?';
        if MsgBox(Msg, mbConfirmation, MB_YESNO) <> IDYES then
            Result := False;
    end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
    Scope: Integer;
begin
    if CurStep = ssPostInstall then
    begin
        InitializeGlobals;
        CreateHardlinks;
        ModifyPath(WizardIsTaskSelected('addtopath'));

        if g_HasUsablePowerShell then
            Scope := g_PowerShellPage.SelectedValueIndex
        else
            Scope := PWSH_SCOPE_NONE;

        InstallPowerShellProfiles(Scope);
    end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
    if CurUninstallStep = usUninstall then
    begin
        InitializeGlobals;
        InstallPowerShellProfiles(PWSH_SCOPE_NONE);
        ModifyPath(False);
        DelTree(g_AppDirPath, True, True, True);
    end;
end;
