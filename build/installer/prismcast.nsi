; PrismCast Studio — Instalador NSIS Nativo (100% Rust)
!include "MUI2.nsh"

!ifndef APP_VERSION
  !define APP_VERSION "3.1.0"
!endif

Name "PrismCast Studio ${APP_VERSION}"
OutFile "..\..\release-windows\PrismCast-Studio-${APP_VERSION}-x64-setup.exe"
Unicode True
InstallDir "$LOCALAPPDATA\Programs\PrismCast Studio"
InstallDirRegKey HKCU "Software\PrismCast Studio" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!define MUI_ICON "..\icon.ico"
!define MUI_UNICON "..\icon.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEFINISHPAGE_BITMAP "nsis-sidebar.bmp"
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_BITMAP "nsis-header.bmp"
!define MUI_FINISHPAGE_RUN "$INSTDIR\prismcast.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Abrir PrismCast Studio"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "PortugueseBR"

Section "PrismCast Studio" SecMain
  SetOutPath "$INSTDIR"
  File "..\..\target\release\prismcast.exe"
  File "..\..\target\release\prismcast-audio.exe"
  File "..\icon.ico"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "Software\PrismCast Studio" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismCastStudio" "DisplayName" "PrismCast Studio"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismCastStudio" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismCastStudio" "Publisher" "PrismCast"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismCastStudio" "DisplayIcon" "$INSTDIR\icon.ico"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismCastStudio" "UninstallString" '"$INSTDIR\uninstall.exe"'

  CreateDirectory "$SMPROGRAMS\PrismCast Studio"
  CreateShortcut "$SMPROGRAMS\PrismCast Studio\PrismCast Studio.lnk" "$INSTDIR\prismcast.exe" "" "$INSTDIR\icon.ico"
  CreateShortcut "$SMPROGRAMS\PrismCast Studio\Desinstalar PrismCast Studio.lnk" "$INSTDIR\uninstall.exe"
  CreateShortcut "$DESKTOP\PrismCast Studio.lnk" "$INSTDIR\prismcast.exe" "" "$INSTDIR\icon.ico"
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\PrismCast Studio.lnk"
  Delete "$SMPROGRAMS\PrismCast Studio\PrismCast Studio.lnk"
  Delete "$SMPROGRAMS\PrismCast Studio\Desinstalar PrismCast Studio.lnk"
  RMDir "$SMPROGRAMS\PrismCast Studio"

  Delete "$INSTDIR\prismcast.exe"
  Delete "$INSTDIR\prismcast-audio.exe"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "Software\PrismCast Studio"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismCastStudio"
SectionEnd
