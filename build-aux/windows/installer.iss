; Compile with ISCC /DAppVersion=0.1.0 build-aux/windows/installer.iss
#ifndef AppVersion
  #error AppVersion must be supplied by the build workflow
#endif

[Setup]
AppId={{4AC91456-10AD-4FCE-94A7-7726C5881DD5}
AppName=Speech2Text
AppVersion={#AppVersion}
AppPublisher=Davide Camino
AppPublisherURL=https://github.com/DavideCamino/Speech_to_text_multiplatform
DefaultDirName={localappdata}\Programs\Speech2Text
DefaultGroupName=Speech2Text
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile=..\..\LICENSE
OutputDir=..\..\dist
OutputBaseFilename=Speech2Text-{#AppVersion}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\bin\speech2text.exe
CloseApplications=yes
SetupLogging=yes

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

; Remove obsolete runtime files on upgrade, never user preferences or models.
[InstallDelete]
Type: filesandordirs; Name: "{app}\bin"
Type: filesandordirs; Name: "{app}\lib"
Type: filesandordirs; Name: "{app}\share"
Type: filesandordirs; Name: "{app}\etc"
Type: filesandordirs; Name: "{app}\licenses"

[Files]
Source: "..\..\dist\Speech2Text\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Speech2Text"; Filename: "{app}\Start-Speech2Text.cmd"; WorkingDir: "{app}"; IconFilename: "{app}\bin\speech2text.exe"
Name: "{autodesktop}\Speech2Text"; Filename: "{app}\Start-Speech2Text.cmd"; WorkingDir: "{app}"; IconFilename: "{app}\bin\speech2text.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Start-Speech2Text.cmd"; Description: "Launch Speech2Text"; Flags: postinstall shellexec skipifsilent nowait
