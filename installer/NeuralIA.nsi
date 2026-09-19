!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"

!ifndef APP_VERSION
  !error "APP_VERSION is required"
!endif
!ifndef SOURCE_EXE
  !error "SOURCE_EXE is required"
!endif
!ifndef OUTPUT_DIR
  !error "OUTPUT_DIR is required"
!endif
!ifndef ICON_PATH
  !error "ICON_PATH is required"
!endif

Unicode True
Name "NeuralIA"
OutFile "${OUTPUT_DIR}\NeuralIA-Setup-${APP_VERSION}-x64.exe"
InstallDir "$LOCALAPPDATA\Programs\NeuralIA"
RequestExecutionLevel user
SetCompressor /SOLID lzma
Icon "${ICON_PATH}"
UninstallIcon "${ICON_PATH}"

!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "NeuralIA requires 64-bit Windows."
    Abort
  ${EndIf}
FunctionEnd

Section "NeuralIA" SEC_MAIN
  SetShellVarContext current
  SetRegView 64
  SetOutPath "$INSTDIR"

  File /oname=NeuralIA.exe "${SOURCE_EXE}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\NeuralIA.lnk" "$INSTDIR\NeuralIA.exe"

  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\NeuralIA" "DisplayName" "NeuralIA"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\NeuralIA" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\NeuralIA" "Publisher" "Jose Ribamar Ferreira Junior"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\NeuralIA" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\NeuralIA" "DisplayIcon" "$INSTDIR\NeuralIA.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\NeuralIA" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\NeuralIA" "QuietUninstallString" "$\"$INSTDIR\Uninstall.exe$\" /S"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\NeuralIA" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\NeuralIA" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  SetRegView 64

  Delete "$SMPROGRAMS\NeuralIA.lnk"
  Delete "$INSTDIR\NeuralIA.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\NeuralIA"
SectionEnd
