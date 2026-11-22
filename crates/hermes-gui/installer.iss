; Windows installer for the desktop client, built with Inno Setup 6.6 or newer (6.7.3 in the release
; workflow): `ISCC /DVersion=<version> /DFileVersion=<x.y.z> crates\hermes-gui\installer.iss`.
;
; It installs the files of the portable folder (`stage\portable`, made by the release workflow's
; Package step: hermes.exe, hermes-tui.exe, the licence and FFmpeg), so the installer, the portable
; zip and the in-app updates always carry the same files. Per user, without an admin prompt, with a
; Start menu shortcut, an uninstaller and the hermes:// links.
;
; `/DTest` builds a test installer under other names ("Hermes Installer Test", hermes-test://, its
; own registry keys), so trying it never touches a real Hermes install on the same PC.
;
; Before 1.5 the installer was NSIS (cargo-packager). This one installs over such an install in the
; same folder and removes the old installer's entries itself. It does not run the old uninstaller:
; that one ends every process named hermes.exe, also other copies of Hermes.

#ifdef Test
  #define AppName "Hermes Installer Test"
  #define AppGuid "FD57D6C1-49DE-45C1-A4E0-BBDF3B1D319A"
  #define Protocol "hermes-test"
  #define AppUserModel "tools.senko.hermes.installer-test"
#else
  #define AppName "Hermes"
  #define AppGuid "C0B42D8A-130E-4477-98A0-917F64B0F00D"
  #define Protocol "hermes"
  #define AppUserModel "tools.senko.hermes"
#endif
#ifndef Version
  #define Version "0.0.0-dev"
#endif
; Windows wants numbers only here (a dry run's version is `dev-<commit>`).
#ifndef FileVersion
  #define FileVersion "0.0.0"
#endif
#ifndef Source
  #define Source "..\..\stage\portable"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

; Where the NSIS installer kept its install (cargo-packager's template).
#define NsisUninstallKey "Software\Microsoft\Windows\CurrentVersion\Uninstall\" + AppName
#define NsisProductKey "Software\senko-tools\" + AppName

[Setup]
AppId={{{#AppGuid}}
AppName={#AppName}
AppVersion={#Version}
AppVerName={#AppName} {#Version}
AppPublisher=senko-tools
AppPublisherURL=https://hermes.senko.tools
AppSupportURL=https://github.com/senko-tools/hermes
VersionInfoVersion={#FileVersion}
VersionInfoDescription={#AppName} Setup
; An NSIS install's folder, else %LOCALAPPDATA%\Hermes as before. A later run of this installer
; uses the folder of the last install instead (UsePreviousAppDir).
DefaultDirName={code:DefaultDir}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; Light or dark with Windows.
WizardStyle=modern dynamic
SetupIconFile=assets\icons\hermes.ico
; The Hermes icon in the corner of each page instead of Inno's box (it picks a size for the DPI).
WizardSmallImageFile=assets\icons\icon-64.png,assets\icons\icon-128.png
WizardSmallImageFileDynamicDark=assets\icons\icon-64.png,assets\icons\icon-128.png
UninstallDisplayIcon={app}\hermes.exe
UninstallDisplayName={#AppName}
LicenseFile=..\..\LICENSE-GPL-3.0
OutputDir={#OutputDir}
OutputBaseFilename=Hermes-Setup-{#Version}
Compression=lzma2/max
SolidCompression=yes
; The hermes:// links.
ChangesAssociations=yes
; Only programs that use files in the install folder are closed (Windows' Restart Manager), so
; other copies of Hermes keep running.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Source}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\hermes.exe"; AppUserModelID: "{#AppUserModel}"
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\hermes.exe"; AppUserModelID: "{#AppUserModel}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Classes\{#Protocol}"; ValueType: string; ValueName: ""; ValueData: "URL:{#AppName} link"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\{#Protocol}"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\{#Protocol}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\hermes.exe"",0"
Root: HKCU; Subkey: "Software\Classes\{#Protocol}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\hermes.exe"" ""%1"""

[Run]
Filename: "{app}\hermes.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Left by the in-app updater until the new version has started once.
Type: files; Name: "{app}\*.old"
Type: files; Name: "{app}\.update-pending"

[Code]
{ The folder of an install made by the NSIS installer, or '' if there is none. }
function NsisDir(): String;
begin
  Result := '';
  if RegQueryStringValue(HKCU, '{#NsisProductKey}', '', Result) then
    Result := RemoveBackslashUnlessRoot(RemoveQuotes(Result));
  if (Result <> '') and not DirExists(Result) then
    Result := '';
end;

function DefaultDir(Param: String): String;
begin
  Result := NsisDir();
  if Result = '' then
    Result := ExpandConstant('{localappdata}\{#AppName}');
end;

{ After installing: the NSIS installer's entries go, so Apps & features lists Hermes once. In the
  same folder its files were just replaced, and only its uninstaller is left to delete. }
procedure LeaveNsis();
var
  Old: String;
begin
  if not RegKeyExists(HKCU, '{#NsisUninstallKey}') then
    exit;
  Old := '';
  if RegQueryStringValue(HKCU, '{#NsisUninstallKey}', 'InstallLocation', Old) then
    Old := RemoveBackslashUnlessRoot(RemoveQuotes(Old));
  if (Old <> '') and SameText(AddBackslash(Old), AddBackslash(ExpandConstant('{app}'))) then
  begin
    DeleteFile(AddBackslash(Old) + 'uninstall.exe');
    Log('Took over the NSIS install in ' + Old);
  end
  else if Old <> '' then
  begin
    Log('An NSIS install stays in ' + Old);
    if not WizardSilent() then
      MsgBox('The older {#AppName} in "' + Old + '" was left in place, but no longer has an ' +
        'uninstaller: once you no longer need it, delete that folder. Your settings and sign-ins ' +
        'are not in it.', mbInformation, MB_OK);
  end;
  RegDeleteKeyIncludingSubkeys(HKCU, '{#NsisUninstallKey}');
  RegDeleteKeyIncludingSubkeys(HKCU, '{#NsisProductKey}');
  RegDeleteKeyIfEmpty(HKCU, 'Software\senko-tools');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    LeaveNsis();
end;

{ Windows' Restart Manager, which Inno Setup uses only when installing. }
function RmStartSession(out Session: DWORD; Flags: DWORD; Key: String): DWORD;
  external 'RmStartSession@rstrtmgr.dll stdcall delayload uninstallonly';
function RmRegisterResources(Session: DWORD; FileCount: UINT; Files: TArrayOfString;
  AppCount: UINT; Apps: Cardinal; ServiceCount: UINT; Services: Cardinal): DWORD;
  external 'RmRegisterResources@rstrtmgr.dll stdcall delayload uninstallonly';
function RmShutdown(Session: DWORD; Flags: Cardinal; Callback: Cardinal): DWORD;
  external 'RmShutdown@rstrtmgr.dll stdcall delayload uninstallonly';
function RmEndSession(Session: DWORD): DWORD;
  external 'RmEndSession@rstrtmgr.dll stdcall delayload uninstallonly';

{ Closes the programs that use this install's executables, as installing does, so they can be
  deleted. Only those: other copies of Hermes keep running. }
procedure CloseRunningApp();
var
  Session: DWORD;
  Key: String;
  Files: TArrayOfString;
  Err: DWORD;
begin
  Key := StringOfChar(' ', 64);
  if RmStartSession(Session, 0, Key) <> 0 then
  begin
    Log('Restart Manager did not start');
    exit;
  end;
  SetArrayLength(Files, 2);
  Files[0] := ExpandConstant('{app}\hermes.exe');
  Files[1] := ExpandConstant('{app}\hermes-tui.exe');
  Err := RmRegisterResources(Session, 2, Files, 0, 0, 0, 0);
  if Err = 0 then
    Err := RmShutdown(Session, 0, 0);
  Log(Format('Closing the running app: %d', [Err]));
  RmEndSession(Session);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    CloseRunningApp();
end;
