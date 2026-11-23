; Inno Setup script for Star Citizen Rich Presence.
; Build: ISCC /DAppVersion=3.0.0 installer\scrp.iss
; Optional: /DExePath=path\to\star-citizen-rich-presence.exe  /DOutputDir=path

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef ExePath
  #define ExePath "..\target\release\star-citizen-rich-presence.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\target\installer"
#endif
#define AppName "Star Citizen Rich Presence"
#define ExeName "star-citizen-rich-presence.exe"

[Setup]
; Keep this AppId forever so upgrades replace the existing install.
AppId={{6C1E4F2A-8B37-4D5E-9A61-3F0C2B7D9E14}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Star Citizen Rich Presence contributors
AppPublisherURL=https://github.com/pawrenity/star-citizen-rich-presence
AppSupportURL=https://github.com/pawrenity/star-citizen-rich-presence/issues
AppUpdatesURL=https://github.com/pawrenity/star-citizen-rich-presence/releases
; Per-user install: no administrator prompt, like the portable app.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
SetupIconFile=..\assets\scrp.ico
UninstallDisplayIcon={app}\{#ExeName}
UninstallDisplayName={#AppName}
OutputDir={#OutputDir}
OutputBaseFilename=star-citizen-rich-presence-setup-x64
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Close a running copy (it locks the exe) and reopen it afterwards.
CloseApplications=yes
RestartApplications=no
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "startup"; Description: "Start with Windows (hidden in the tray)"; GroupDescription: "Startup:"

[Files]
Source: "{#ExePath}"; DestDir: "{app}"; DestName: "{#ExeName}"; Flags: ignoreversion
Source: "..\assets\artwork-gallery.html"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\scrp.example.toml"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\RELEASE_NOTES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#ExeName}"
Name: "{group}\Artwork picker"; Filename: "{app}\artwork-gallery.html"
Name: "{group}\Settings example (scrp.example.toml)"; Filename: "{app}\scrp.example.toml"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#ExeName}"; Tasks: desktopicon

[Registry]
; Same value the app's own "Start with Windows" toggle writes, so either can turn it off.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "StarCitizenRichPresence"; ValueData: """{app}\{#ExeName}"" --tray"; Tasks: startup

[Run]
Filename: "{app}\{#ExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
; The in-app updater runs setup silently with /UPDATE; reopen the app afterwards.
Filename: "{app}\{#ExeName}"; Flags: nowait; Check: IsUpdate

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM {#ExeName}"; Flags: runhidden; RunOnceId: "StopApp"

[Code]
function IsUpdate: Boolean;
begin
  Result := Pos('/UPDATE', Uppercase(GetCmdTail)) > 0;
end;

const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  RunName = 'StarCitizenRichPresence';

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Command: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    { The app's own toggle can add this value too. Remove it only when it starts this
      installed copy, so a separate portable copy keeps starting with Windows. }
    if RegQueryStringValue(HKEY_CURRENT_USER, RunKey, RunName, Command) and
       (Pos(Lowercase(ExpandConstant('{app}\')), Lowercase(Command)) > 0) then
      RegDeleteValue(HKEY_CURRENT_USER, RunKey, RunName);
    if (not UninstallSilent) and DirExists(ExpandConstant('{localappdata}\StarCitizenRichPresence')) then
      if MsgBox('Also delete your saved control-panel preferences?', mbConfirmation, MB_YESNO) = IDYES then
        DelTree(ExpandConstant('{localappdata}\StarCitizenRichPresence'), True, True, True);
  end;
end;
