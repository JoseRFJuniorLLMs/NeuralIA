#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef SourceExe
  #error SourceExe is required
#endif
#ifndef OutputDir
  #error OutputDir is required
#endif
#ifndef IconPath
  #error IconPath is required
#endif

[Setup]
AppId={{8B2A98F4-7D55-4C43-ABF0-0D7D1A02C4B9}
AppName=NeuralIA
AppVersion={#AppVersion}
AppPublisher=Jose Ribamar Ferreira Junior
DefaultDirName={localappdata}\Programs\NeuralIA
DefaultGroupName=NeuralIA
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=NeuralIA-Setup-{#AppVersion}-x64
SetupIconFile={#IconPath}
UninstallDisplayIcon={app}\NeuralIA.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
UsePreviousAppDir=yes
UsePreviousGroup=yes

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "NeuralIA.exe"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\NeuralIA"; Filename: "{app}\NeuralIA.exe"; WorkingDir: "{app}"

[Run]
Filename: "{app}\NeuralIA.exe"; Description: "Abrir NeuralIA"; Flags: nowait postinstall skipifsilent
