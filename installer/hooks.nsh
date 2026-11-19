; atmosphere · hooks.nsh · Uninstaller hooks: offer a backup before removing data, and remove Atmosphere's own data folders only when asked

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
  ${AndIf} $DeleteAppDataCheckboxState = 1
    SetShellVarContext current
    MessageBox MB_YESNO|MB_ICONQUESTION "Atmosphere will remove your settings, listening stats, liked songs and artist photos.$\r$\n$\r$\nSave a backup to your Documents folder first? You can restore it later from Settings, Storage.$\r$\n$\r$\nYour music files are never touched." /SD IDNO IDNO atmo_skip_backup
      StrCpy $R0 "$DOCUMENTS\Atmosphere backup.zip"
      StrCpy $R1 1
      atmo_name_loop:
        IfFileExists "$R0" 0 atmo_name_done
        IntOp $R1 $R1 + 1
        StrCpy $R0 "$DOCUMENTS\Atmosphere backup ($R1).zip"
        Goto atmo_name_loop
      atmo_name_done:
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --backup "$R0"' $R2
      ${If} $R2 = 0
      ${AndIf} ${FileExists} "$R0"
        MessageBox MB_OK|MB_ICONINFORMATION "Backup saved to:$\r$\n$R0" /SD IDOK
      ${Else}
        MessageBox MB_YESNO|MB_ICONEXCLAMATION "The backup couldn't be saved. Keep your Atmosphere data instead of removing it?" /SD IDYES IDNO atmo_skip_backup
          StrCpy $DeleteAppDataCheckboxState 0
      ${EndIf}
    atmo_skip_backup:
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
  ${AndIf} $DeleteAppDataCheckboxState = 1
    SetShellVarContext current
    RmDir /r "$APPDATA\atmosphere"
    RmDir /r "$LOCALAPPDATA\atmosphere"
  ${EndIf}
!macroend
