; Inno Setup script for APFSReader. Build it with scripts\release.ps1, or by hand:
;   ISCC.exe /DAppVersion=0.9.0 /DStageDir=<folder with the files> installer\APFSReader.iss
;
; The installer works without administrator rights (it installs for the current user
; unless the user chooses all users in the dialog). WinFsp, which APFSReader needs, is
; not bundled: if it is missing the installer offers to install it with winget, and
; the app itself explains how to get it.

#ifndef AppVersion
  #define AppVersion "0.9.0"
#endif
#ifndef StageDir
  #define StageDir "..\dist\stage"
#endif
#define AppName "APFSReader"
#define AppExe "apfsreader-gui.exe"

[Setup]
AppId={{6F1B7C2E-3A54-4B8D-9E0A-5C2D81F7A4B3}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=niceokiraku
AppCopyright=Copyright (c) 2026 niceokiraku
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
SetupIconFile={#StageDir}\apfsreader.ico
OutputDir=..\dist
OutputBaseFilename=APFSReader-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile={#StageDir}\LICENSE.md
CloseApplications=yes
RestartApplications=no
DisableProgramGroupPage=yes

[Languages]
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "installwinfsp"; Description: "WinFsp (ファイルシステムドライバー) をインストールする / Install WinFsp (file system driver)"; GroupDescription: "WinFsp:"; Check: NeedWinFsp

[Files]
Source: "{#StageDir}\apfsreader-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\apfsreader-broker.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\apfsreader.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\apfsreader-mount.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSE.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\COPYING"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\THIRD_PARTY_LICENSES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\USER_GUIDE.ja.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\USER_GUIDE.en.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\{#AppName} 使い方 (User guide)"; Filename: "{app}\USER_GUIDE.ja.md"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "winget.exe"; Parameters: "install --id WinFsp.WinFsp -e --accept-package-agreements --accept-source-agreements"; StatusMsg: "WinFsp ...  (UAC)"; Flags: runhidden waituntilterminated; Tasks: installwinfsp
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "taskkill.exe"; Parameters: "/F /IM {#AppExe}"; Flags: runhidden; RunOnceId: "StopApp"

[Code]
{ WinFsp registers its install directory here (a 32-bit installer, so WOW6432Node). }
function WinFspInstalled: Boolean;
var
  Dir: String;
begin
  Result := RegQueryStringValue(HKLM32, 'SOFTWARE\WinFsp', 'InstallDir', Dir) and (Dir <> '');
end;

function WingetAvailable: Boolean;
var
  Code: Integer;
begin
  Result := Exec(ExpandConstant('{cmd}'), '/C winget --version', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
end;

{ The WinFsp task is offered only when WinFsp is missing and winget can install it. }
function NeedWinFsp: Boolean;
begin
  Result := (not WinFspInstalled) and WingetAvailable;
end;
