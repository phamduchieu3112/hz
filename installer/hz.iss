; Inno Setup 6 script for Hz (Hiếu's Zalo)
; Compiles to: dist\Hz-Setup-v<MyAppVersion>.exe
; Optional preprocessor overrides:
;   ISCC /DMyAppVersion=0.1.4 /DFfmpegPath="C:\path\to\ffmpeg.exe" installer\hz.iss

#ifndef MyAppVersion
  #define MyAppVersion "0.1.4"
#endif

#define MyAppName "Hz"
#define MyAppFullName "Hz (Hiếu's Zalo)"
#define MyAppPublisher "Phạm Đức Hiếu"
#define MyAppURL "https://github.com/phamduchieu3112/hz"
#define MyAppExeName "hz.exe"

[Setup]
AppId={{8F9E3B2A-6C4D-4E1F-9A7B-5D2C8E4F1A3B}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppFullName} v{#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}/issues
AppUpdatesURL={#MyAppURL}/releases
DefaultDirName={localappdata}\Programs\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\dist
OutputBaseFilename=Hz-Setup-v{#MyAppVersion}
SetupIconFile=..\src\assets\app_icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppFullName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
CloseApplicationsFilter=hz.exe,ffmpeg.exe
RestartApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
#ifdef FfmpegPath
Source: "{#FfmpegPath}"; DestDir: "{app}"; DestName: "ffmpeg.exe"; Flags: ignoreversion
#endif

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Parameters: "--gui"; WorkingDir: "{app}"; IconFilename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Parameters: "--gui"; WorkingDir: "{app}"; IconFilename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Parameters: "--gui"; WorkingDir: "{app}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent
