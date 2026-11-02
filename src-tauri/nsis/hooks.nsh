; MapsOffline NSIS hooks (bundle.windows.nsis.installerHooks).
; The binary and install folder keep the Latin name "MapsOffline"; the shortcuts the user sees
; on the desktop and in the Start menu are called "מפות אופליין".

!define HEBREW_SHORTCUT "מפות אופליין"

!macro NSIS_HOOK_POSTINSTALL
  ; Start menu: replace the shortcut the template just created
  !if "${STARTMENUFOLDER}" != ""
    Delete "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
    CreateShortcut "$SMPROGRAMS\$AppStartMenuFolder\${HEBREW_SHORTCUT}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\$AppStartMenuFolder\${HEBREW_SHORTCUT}.lnk"
  !else
    Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    CreateShortcut "$SMPROGRAMS\${HEBREW_SHORTCUT}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${HEBREW_SHORTCUT}.lnk"
  !endif
  ; Desktop: create ours now and keep the finish-page checkbox from adding a Latin-named one
  Delete "$DESKTOP\${PRODUCTNAME}.lnk"
  CreateShortcut "$DESKTOP\${HEBREW_SHORTCUT}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${HEBREW_SHORTCUT}.lnk"
  StrCpy $NoShortcutMode 1
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro UnpinShortcut "$DESKTOP\${HEBREW_SHORTCUT}.lnk"
  Delete "$DESKTOP\${HEBREW_SHORTCUT}.lnk"
  !insertmacro UnpinShortcut "$SMPROGRAMS\$AppStartMenuFolder\${HEBREW_SHORTCUT}.lnk"
  Delete "$SMPROGRAMS\$AppStartMenuFolder\${HEBREW_SHORTCUT}.lnk"
  Delete "$SMPROGRAMS\${HEBREW_SHORTCUT}.lnk"
!macroend
