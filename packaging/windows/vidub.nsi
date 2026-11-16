Unicode True
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "MUI2.nsh"

!define APPNAME "ViDub"
!define APPEXE "vidub_app.exe"
!ifndef VERSION
  !define VERSION "0.1.0"
!endif

!define ROOT "..\.."

Name "${APPNAME} ${VERSION}"
OutFile "${ROOT}\output\ViDubSetup-${VERSION}.exe"
InstallDir "$LOCALAPPDATA\ViDub"
InstallDirRegKey HKCU "Software\ViDub" "InstallDir"
BrandingText "${APPNAME}"

!if "${VERSION}" == "$V"
  !error "VERSION was not expanded; pass it as one quoted argument: -DVERSION=1.2.3"
!endif
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APPNAME}"
VIAddVersionKey "FileVersion" "${VERSION}.0"
VIAddVersionKey "ProductVersion" "${VERSION}.0"
VIAddVersionKey "CompanyName" "ViDub"
VIAddVersionKey "LegalCopyright" "ViDub contributors"

!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APPEXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Launch ViDub"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${ROOT}\THIRD_PARTY_NOTICES.md"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

ShowInstDetails show
ShowUnInstDetails show

Section "ViDub (required)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${ROOT}\target\release\vidub_app.exe"
  File "${ROOT}\dist\versions.json"
  File "${ROOT}\models.toml"
  File "${ROOT}\THIRD_PARTY_NOTICES.md"

  SetOutPath "$INSTDIR\bin"
  File /r "${ROOT}\dist\bin\*"

  SetOutPath "$INSTDIR\models"
  File /r "${ROOT}\dist\models\*"

  WriteUninstaller "$INSTDIR\Uninstall.exe"

  WriteRegStr HKCU "Software\ViDub" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ViDub" "DisplayName" "${APPNAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ViDub" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ViDub" "Publisher" "ViDub contributors"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ViDub" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ViDub" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ViDub" "DisplayIcon" "$INSTDIR\${APPEXE}"
SectionEnd

Section "Start Menu Shortcut" SecStartMenu
  CreateDirectory "$SMPROGRAMS\ViDub"
  CreateShortcut "$SMPROGRAMS\ViDub\ViDub.lnk" "$INSTDIR\${APPEXE}"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\${APPEXE}"
  Delete "$INSTDIR\versions.json"
  Delete "$INSTDIR\models.toml"
  Delete "$INSTDIR\THIRD_PARTY_NOTICES.md"
  RMDir /r "$INSTDIR\bin"
  RMDir /r "$INSTDIR\models"
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$SMPROGRAMS\ViDub\ViDub.lnk"
  RMDir "$SMPROGRAMS\ViDub"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ViDub"
  DeleteRegKey HKCU "Software\ViDub"
  RMDir "$INSTDIR"
SectionEnd
