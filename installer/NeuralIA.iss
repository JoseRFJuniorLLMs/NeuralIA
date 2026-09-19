#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef SourceExe
  #error SourceExe is required
#endif
#ifndef OutputDir
  #error OutputDir is required
#endif

[Setup]
AppId={{8B2A98F4-7D55-4C43-ABF0-0D7D1A02C4B9}
AppName=NeuralIA
AppVersion={#AppVersion}
AppPublisher=Jose Ribamar Ferreira Junior
DefaultDirName={localappdata}\Programs\NeuralIA
DefaultGroupName=NeuralIA
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=NeuralIA-Setup-{#AppVersion}-x64
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupIconFile=..\assets\logo.ico
UninstallDisplayIcon={app}\NeuralIA.exe
UninstallDisplayName=NeuralIA
Uninstallable=yes
CreateUninstallRegKey=yes
DisableProgramGroupPage=yes
ChangesAssociations=no
ChangesEnvironment=no
CloseApplications=yes
RestartApplications=no

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "NeuralIA.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\NeuralIA"; Filename: "{app}\NeuralIA.exe"; WorkingDir: "{app}"
