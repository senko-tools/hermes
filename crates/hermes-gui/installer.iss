; Windows installer for the desktop client, built with Inno Setup 6.6 or newer (6.7.3 in the release
; workflow): `ISCC /DVersion=<version> /DFileVersion=<x.y.z> crates\hermes-gui\installer.iss`.
;
; It installs the files of the portable folder (`stage\portable`, made by the release workflow's
; Package step: hermes.exe, hermes-tui.exe, the licence and FFmpeg), so the installer, the portable
; zip and the in-app updates always carry the same files. Per user, without an admin prompt.
;
; Two kinds of installation (the first page; `/PORTABLE` and `/SIGNINSINFOLDER` when silent):
;   - Normal: Start menu shortcut, uninstaller, Apps & features entry, hermes:// links. Settings
;     live in %APPDATA%\Hermes, sign-ins in Windows Credential Manager.
;   - Portable: nothing outside the folder. A `config.toml` next to hermes.exe makes the app keep
;     everything in the folder (ConfigPaths::detect); sign-ins stay in Credential Manager unless
;     "Keep sign-ins in this folder" is ticked (`sign_ins_in_folder`). No uninstaller: deleting
;     the folder removes it.
;
; `/DTest` builds a test installer under other names ("Hermes Installer Test", hermes-test://, its
; own registry keys and data folders), so trying it never touches a real Hermes install on the same
; PC; its uninstaller never deletes real data or secrets.
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
; The folder page always shows, also when updating: where it goes is never a guess.
DisableDirPage=no
; Browse adds the app's folder itself (BrowseDir), and typed paths are used as they are.
AppendDefaultDirName=no
; Replaced by the own check in NextButtonClick (only for folders with other files in them).
DirExistsWarning=no
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
; A portable install leaves nothing behind: no uninstaller, no Apps & features entry.
Uninstallable=IsNormal
CreateUninstallRegKey=IsNormal
UninstallDisplayIcon={app}\hermes.exe
UninstallDisplayName={#AppName}
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

[Messages]
SelectDirLabel3=[name] and its files are installed directly into the folder below. Any folders in this path that don't exist yet are created.
SelectDirBrowseLabel=To continue, click Next. Browse adds a [name] folder to the folder you pick; the path can still be edited by hand.
; The licence is accepted with this button (the note above it says so); there is no licence page.
ButtonInstall=&Accept and install
ReadyLabel2a=Click Accept and install to continue, or click Back to review or change any settings.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; The licence shown by "Read the licence" on the last page, from inside the installer (no internet).
Source: "..\..\LICENSE-GPL-3.0"; DestName: "licence-shown.txt"; Flags: dontcopy
Source: "{#Source}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\hermes.exe"; AppUserModelID: "{#AppUserModel}"; Check: IsNormal
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\hermes.exe"; AppUserModelID: "{#AppUserModel}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Classes\{#Protocol}"; ValueType: string; ValueName: ""; ValueData: "URL:{#AppName} link"; Flags: uninsdeletekey; Check: IsNormal
Root: HKCU; Subkey: "Software\Classes\{#Protocol}"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Check: IsNormal
Root: HKCU; Subkey: "Software\Classes\{#Protocol}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\hermes.exe"",0"; Check: IsNormal
Root: HKCU; Subkey: "Software\Classes\{#Protocol}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\hermes.exe"" ""%1"""; Check: IsNormal

[Run]
Filename: "{app}\hermes.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Left by the in-app updater until the new version has started once.
Type: files; Name: "{app}\*.old"
Type: files; Name: "{app}\.update-pending"
#ifdef Test
Type: files; Name: "{app}\hermes-installer-test.marker"
#endif

[Code]
var
  TypePage: TWizardPage;
  NormalRadio, PortableRadio: TNewRadioButton;
  SignInsBox: TNewCheckBox;
  SignInsNote: TNewStaticText;

{ ---- Kind of installation ---- }

function IsPortable(): Boolean;
begin
  Result := (PortableRadio <> nil) and PortableRadio.Checked;
end;

function IsNormal(): Boolean;
begin
  Result := not IsPortable();
end;

{ A wrapping text on `Page` at `Left`, `Top`; returns its bottom. }
function AddText(Page: TWizardPage; Left, Top: Integer; Text: String): Integer;
var
  Label1: TNewStaticText;
begin
  Label1 := TNewStaticText.Create(Page);
  Label1.Parent := Page.Surface;
  Label1.Left := Left;
  Label1.Top := Top;
  Label1.Width := Page.SurfaceWidth - Left;
  Label1.AutoSize := True;
  Label1.WordWrap := True;
  Label1.Caption := Text;
  Result := Label1.Top + Label1.Height;
end;

procedure TypeChanged(Sender: TObject);
begin
  SignInsBox.Enabled := PortableRadio.Checked;
  SignInsNote.Enabled := PortableRadio.Checked;
end;

procedure CreateTypePage();
var
  Y, Indent: Integer;
begin
  TypePage := CreateCustomPage(wpWelcome, 'Installation type', 'Choose how {#AppName} is installed.');
  Indent := ScaleX(18);

  NormalRadio := TNewRadioButton.Create(TypePage);
  NormalRadio.Parent := TypePage.Surface;
  NormalRadio.Width := TypePage.SurfaceWidth;
  NormalRadio.Caption := 'Normal installation';
  NormalRadio.Checked := True;
  NormalRadio.OnClick := @TypeChanged;
  Y := AddText(TypePage, Indent, NormalRadio.Top + NormalRadio.Height + ScaleY(2),
    'A Start menu shortcut, an uninstaller, and invite links in your browser open {#AppName}. ' +
    'Settings and sign-ins are kept for your Windows account.');

  PortableRadio := TNewRadioButton.Create(TypePage);
  PortableRadio.Parent := TypePage.Surface;
  PortableRadio.Top := Y + ScaleY(14);
  PortableRadio.Width := TypePage.SurfaceWidth;
  PortableRadio.Caption := 'Portable installation';
  PortableRadio.OnClick := @TypeChanged;
  Y := AddText(TypePage, Indent, PortableRadio.Top + PortableRadio.Height + ScaleY(2),
    'Everything stays in one folder, for example on a USB stick: no Start menu entry, no ' +
    'uninstaller (delete the folder to remove it), and invite links in your browser don''t ' +
    'open this copy.');

  SignInsBox := TNewCheckBox.Create(TypePage);
  SignInsBox.Parent := TypePage.Surface;
  SignInsBox.Left := Indent;
  SignInsBox.Top := Y + ScaleY(10);
  SignInsBox.Width := TypePage.SurfaceWidth - Indent;
  SignInsBox.Caption := 'Keep sign-ins in this folder (tokens.toml)';

  SignInsNote := TNewStaticText.Create(TypePage);
  SignInsNote.Parent := TypePage.Surface;
  SignInsNote.Left := Indent * 2;
  SignInsNote.Top := SignInsBox.Top + SignInsBox.Height + ScaleY(2);
  SignInsNote.Width := TypePage.SurfaceWidth - Indent * 2;
  SignInsNote.AutoSize := True;
  SignInsNote.WordWrap := True;
  SignInsNote.Caption :=
    'Otherwise they stay in Windows Credential Manager on this computer. In the folder, the ' +
    'folder works on another computer too, but anyone who gets a copy of it is signed in as ' +
    'you and holds your encryption keys. On a USB stick (FAT32/exFAT) the file has no ' +
    'protection at all. It can be changed later in Settings.';

  PortableRadio.Checked := ExpandConstant('{param:PORTABLE|0}') = '1';
  NormalRadio.Checked := not PortableRadio.Checked;
  SignInsBox.Checked := ExpandConstant('{param:SIGNINSINFOLDER|0}') = '1';
  TypeChanged(nil);
end;

{ ---- Folder ---- }

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

{ Browse: the picked folder with the app's folder added, unless it already is one. }
procedure BrowseDir(Sender: TObject);
var
  Dir: String;
begin
  Dir := WizardForm.DirEdit.Text;
  if not BrowseForFolder(SetupMessage(msgBrowseDialogLabel), Dir, True) then
    exit;
  if not (SameText(ExtractFileName(RemoveBackslash(Dir)), '{#AppName}') or
    FileExists(AddBackslash(Dir) + 'hermes.exe')) then
    Dir := AddBackslash(Dir) + '{#AppName}';
  WizardForm.DirEdit.Text := Dir;
end;

{ Whether `Dir` has anything in it. }
function HasFiles(Dir: String): Boolean;
var
  Found: TFindRec;
begin
  Result := False;
  if FindFirst(AddBackslash(Dir) + '*', Found) then
  try
    repeat
      if (Found.Name <> '.') and (Found.Name <> '..') then
      begin
        Result := True;
        exit;
      end;
    until not FindNext(Found);
  finally
    FindClose(Found);
  end;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  Dir: String;
begin
  Result := True;
  if CurPageID <> wpSelectDir then
    exit;
  Dir := RemoveBackslashUnlessRoot(WizardDirValue());
  { A normal install there would make the app use it as a portable one. }
  if IsNormal() and FileExists(AddBackslash(Dir) + 'config.toml') and
    FileExists(AddBackslash(Dir) + 'hermes.exe') then
  begin
    SuppressibleMsgBox('This folder holds a portable {#AppName}. Choose Portable installation ' +
      'to update it, or another folder.', mbError, MB_OK, IDOK);
    Result := False;
    exit;
  end;
  if DirExists(Dir) and HasFiles(Dir) and not FileExists(AddBackslash(Dir) + 'hermes.exe') then
    Result := SuppressibleMsgBox('This folder already contains other files.' + #13#10#13#10 +
      '{#AppName} puts its files directly into it, and any file there with the same name as one ' +
      'of {#AppName}''s files will be replaced.' + #13#10#13#10 + 'Install here anyway?',
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDYES) = IDYES;
end;

#ifdef Test
{ A test build installs only into a new or empty folder, or one it made itself (with this
  marker), never over a real install, whatever the command line or the defaults say. }
const
  TestMarker = 'hermes-installer-test.marker';

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Dir: String;
begin
  Result := '';
  Dir := ExpandConstant('{app}');
  Log('Test build: installing into ' + Dir);
  if DirExists(Dir) and HasFiles(Dir) and not FileExists(AddBackslash(Dir) + TestMarker) then
    Result := 'Test build: refusing to install into "' + Dir + '", which is not a test folder.';
end;
#endif

{ ---- Licence, on the last page ---- }

{ The GPL in a window of its own, from the copy inside the installer. }
procedure OpenLicense(Sender: TObject);
var
  Form: TSetupForm;
  Memo: TNewMemo;
  CloseButton: TNewButton;
  Text: AnsiString;
  Shown: String;
begin
  ExtractTemporaryFile('licence-shown.txt');
  if not LoadStringFromFile(ExpandConstant('{tmp}\licence-shown.txt'), Text) then
    exit;
  Shown := String(Text);
  StringChangeEx(Shown, #13#10, #10, True);
  StringChangeEx(Shown, #10, #13#10, True);
  Form := CreateCustomForm(ScaleX(600), ScaleY(480), False, True);
  try
    Form.Caption := 'GNU General Public License v3';
    Memo := TNewMemo.Create(Form);
    Memo.Parent := Form;
    Memo.Left := ScaleX(10);
    Memo.Top := ScaleY(10);
    Memo.Width := Form.ClientWidth - ScaleX(20);
    Memo.Height := Form.ClientHeight - ScaleY(60);
    Memo.ReadOnly := True;
    Memo.ScrollBars := ssVertical;
    Memo.WordWrap := True;
    Memo.Text := Shown;
    CloseButton := TNewButton.Create(Form);
    CloseButton.Parent := Form;
    CloseButton.Caption := 'Close';
    CloseButton.Width := ScaleX(90);
    CloseButton.Height := ScaleY(28);
    CloseButton.Left := Form.ClientWidth - CloseButton.Width - ScaleX(10);
    CloseButton.Top := Form.ClientHeight - CloseButton.Height - ScaleY(12);
    CloseButton.ModalResult := mrOk;
    CloseButton.Default := True;
    CloseButton.Cancel := True;
    Form.ActiveControl := CloseButton;
    Form.ShowModal();
  finally
    Form.Free();
  end;
end;

procedure AddLicenseNote();
var
  Note, Link: TNewStaticText;
begin
  WizardForm.ReadyMemo.Height := WizardForm.ReadyMemo.Height - ScaleY(64);
  Note := TNewStaticText.Create(WizardForm);
  Note.Parent := WizardForm.ReadyPage;
  Note.Left := WizardForm.ReadyMemo.Left;
  Note.Top := WizardForm.ReadyMemo.Top + WizardForm.ReadyMemo.Height + ScaleY(10);
  Note.Width := WizardForm.ReadyMemo.Width;
  Note.AutoSize := True;
  Note.WordWrap := True;
  Note.Caption := '{#AppName} is free software under the GNU General Public License v3. ' +
    'Installing it means you accept that licence.';
  Link := TNewStaticText.Create(WizardForm);
  Link.Parent := WizardForm.ReadyPage;
  Link.Left := Note.Left;
  Link.Top := Note.Top + Note.Height + ScaleY(2);
  Link.Caption := 'Read the licence';
  Link.Font.Style := [fsUnderline];
  Link.Cursor := crHand;
  Link.OnClick := @OpenLicense;
end;

{ The summary on the last page starts with the kind of installation. }
function UpdateReadyMemo(Space, NewLine, MemoUserInfoInfo, MemoDirInfo, MemoTypeInfo,
  MemoComponentsInfo, MemoGroupInfo, MemoTasksInfo: String): String;
begin
  if IsPortable() then
  begin
    Result := 'Installation type:' + NewLine + Space + 'Portable (everything stays in the folder)';
    if SignInsBox.Checked then
      Result := Result + NewLine + Space + 'Sign-ins kept in the folder (tokens.toml)';
  end
  else
    Result := 'Installation type:' + NewLine + Space + 'Normal';
  Result := Result + NewLine + NewLine + MemoDirInfo;
  if MemoTasksInfo <> '' then
    Result := Result + NewLine + NewLine + MemoTasksInfo;
end;

procedure InitializeWizard();
begin
  CreateTypePage();
  WizardForm.DirBrowseButton.OnClick := @BrowseDir;
  AddLicenseNote();
end;

{ ---- After installing ---- }

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

{ Portable: the `config.toml` that makes the app keep everything in its folder. An existing one
  (updating a portable copy) is left as it is. }
procedure WritePortableMarker();
var
  Path, Text: String;
begin
  Path := ExpandConstant('{app}\config.toml');
  if FileExists(Path) then
  begin
    Log('Portable: config.toml is already there');
    exit;
  end;
  Text := '# This file makes Hermes portable: with it next to hermes.exe, Hermes keeps its settings,' + #13#10 +
    '# data and logs in this folder. Settings are added here as you change them.' + #13#10;
  if SignInsBox.Checked then
    Text := Text + #13#10 + '[general]' + #13#10 +
      '# Sign-ins and encryption keys in tokens.toml in this folder (Settings > Window & startup).' + #13#10 +
      'sign_ins_in_folder = true' + #13#10;
  if not SaveStringToFile(Path, Text, False) then
    Log('Portable: could not write ' + Path);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
#ifdef Test
  if (CurStep = ssInstall) and ForceDirectories(ExpandConstant('{app}')) then
    SaveStringToFile(ExpandConstant('{app}\') + TestMarker, 'made by a test installer', False);
#endif
  if CurStep = ssPostInstall then
  begin
    if IsPortable() then
      WritePortableMarker()
    else
      LeaveNsis();
  end;
end;

{ ---- Uninstalling ---- }

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

{ Asks whether the user's own data goes too; never when silent. }
function WantsDataDeleted(): Boolean;
begin
  Result := False;
  if UninstallSilent() then
    exit;
  Result := MsgBox('Also delete your {#AppName} settings and data on this computer?' + #13#10#13#10 +
    'This cannot be undone. You would lose:' + #13#10 +
    '- This computer''s encryption keys: encrypted messages and files you received before can ' +
    'no longer be read here, even after signing in again.' + #13#10 +
    '- Your sign-ins, of every {#AppName} on this computer that keeps them in Windows ' +
    'Credential Manager.' + #13#10 +
    '- Your settings, themes, read marks and per-person volumes.' + #13#10 +
    '- The search index of encrypted messages, and the logs.' + #13#10#13#10 +
    'Your account, servers and messages on the server are not affected.' + #13#10#13#10 +
    'Yes deletes them; No keeps them for a later install.',
    mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
end;

{ The user's data: settings, encryption state, logs (%APPDATA%\Hermes), downloaded updates, and
  the secrets in Credential Manager (removed by hermes.exe itself, which knows their names). }
procedure DeleteUserData();
var
  Code: Integer;
begin
#ifdef Test
  Log('Test build: no real data or secrets are deleted');
  DelTree(ExpandConstant('{userappdata}\{#AppName}'), True, True, True);
#else
  if Exec(ExpandConstant('{app}\hermes.exe'), '--forget-secrets', '', SW_HIDE,
    ewWaitUntilTerminated, Code) then
    Log(Format('Secrets deleted (exit code %d)', [Code]))
  else
    Log('Could not run hermes.exe to delete the secrets');
  DelTree(ExpandConstant('{userappdata}\Hermes'), True, True, True);
  DelTree(ExpandConstant('{localappdata}\senko\Hermes'), True, True, True);
  RemoveDir(ExpandConstant('{localappdata}\senko'));
#endif
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  I: Integer;
begin
  { A Hermes that was just closed may still hold the folder for a moment: the empty folder is
    removed once it lets go (RemoveDir never removes a folder with anything in it). }
  if CurUninstallStep = usPostUninstall then
    for I := 1 to 30 do
    begin
      if not DirExists(ExpandConstant('{app}')) or RemoveDir(ExpandConstant('{app}')) then
        break;
      Sleep(100);
    end;
  if CurUninstallStep = usUninstall then
  begin
    CloseRunningApp();
    { Before the files go: deleting the secrets needs hermes.exe. }
    if WantsDataDeleted() then
      DeleteUserData();
  end;
end;
