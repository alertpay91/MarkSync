; MarkSync installer (Inno Setup 6). Build with build.ps1 in the project root.

#define AppName "MarkSync"
#ifndef AppVer
  #define AppVer "1.0.0"
#endif
#ifndef ExeDir
  #define ExeDir "..\target\release"
#endif

[Setup]
AppId={{8C5E2F7A-3B1D-4E9A-9F21-6D4C7B0A1E55}
AppName={#AppName}
AppVersion={#AppVer}
AppVerName={#AppName} {#AppVer}
AppPublisher={#AppName}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=MarkSync-Setup-{#AppVer}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\MarkSync.exe
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ShowLanguageDialog=yes
LanguageDetectionMethod=none
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes

[Languages]
Name: "srlatn"; MessagesFile: "compiler:Default.isl,SerbianLatin.isl"
Name: "srcyrl"; MessagesFile: "compiler:Default.isl,SerbianCyrillic.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#ExeDir}\marksync.exe"; DestDir: "{app}"; DestName: "MarkSync.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\MarkSync.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\MarkSync.exe"; Tasks: desktopicon

[Registry]
; the program reads this on start and uses the language picked here
Root: HKCU; Subkey: "Software\MarkSync"; ValueType: string; ValueName: "Language"; ValueData: "{language}"; Flags: uninsdeletekey

[Run]
Filename: "{app}\MarkSync.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
