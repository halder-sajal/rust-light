; Inno Setup script for rustlight. Builds a standalone Windows installer
; from the release binary - run `..\scripts\build-windows.ps1` first, then
; compile this with ISCC.exe (Inno Setup's compiler).
;
;   & "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" installer\windows.iss
;
; Output lands in installer\output\rustlight-setup-<version>.exe.
;
; MyAppVersion defaults to 0.1.0 below but can be overridden without editing
; this file, e.g. `ISCC.exe /DMyAppVersion=0.2.0 installer\windows.iss` - the
; release GitHub Action does this so the installer version always matches
; the pushed tag.

#define MyAppName "rustlight"
#ifndef MyAppVersion
#define MyAppVersion "0.1.0"
#endif
#define MyAppPublisher "halder-sajal"
#define MyAppURL "https://github.com/halder-sajal/rust-light"
#define MyAppExeName "app.exe"

[Setup]
AppId={{B6E2B8B0-6C3C-4E6E-9B9B-6B6B6B6B6B6B}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={localappdata}\Programs\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=output
OutputBaseFilename=rustlight-setup-{#MyAppVersion}
SetupIconFile=..\app\ui\app-icon.ico
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\{#MyAppExeName}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked
Name: "autostart"; Description: "Start {#MyAppName} automatically when Windows starts (minimized to tray)"; GroupDescription: "Additional options:"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#MyAppName}"; ValueData: """{app}\{#MyAppExeName}"" --minimized"; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName} now"; Flags: nowait postinstall skipifsilent
